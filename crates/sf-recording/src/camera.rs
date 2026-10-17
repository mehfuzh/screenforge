//! Camera recorder: ffmpeg reads the webcam (AVFoundation on macOS, V4L2 on Linux, DirectShow
//! on Windows) and writes
//! YUV4MPEG frames to a pipe; the latest frame feeds the shared frame repeater, which writes
//! `channel-N-camera-S.m3u8` (HLS/fMP4 like the display) at a constant frame rate. Sessions
//! begin and end together with the display's, so the editor plays both on the same clock.

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub use imp::{CameraCapture, CameraRecorder};

use std::sync::Arc;

/// Frame rate of the camera channel.
pub const CAMERA_FPS: u32 = 30;
/// Frames wider than this are scaled down (keeping the aspect ratio).
pub const MAX_WIDTH: u32 = 1280;

/// One camera frame: tightly packed yuv420p planes (Y, then U, then V).
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub yuv420p: Arc<Vec<u8>>,
}

impl CameraFrame {
    /// RGBA pixels for a preview, downsampled (nearest) so the longer side is at most
    /// `max_side`, optionally mirrored like a selfie view. Returns `(width, height, rgba)`.
    pub fn to_rgba(&self, max_side: u32, mirror: bool) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        let step = (w.max(h) as f64 / max_side.max(1) as f64).max(1.0);
        let (ow, oh) = (((w as f64 / step) as usize).max(1), ((h as f64 / step) as usize).max(1));
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let data = &self.yuv420p[..];
        if data.len() < w * h + 2 * cw * ch {
            return (ow as u32, oh as u32, vec![0; ow * oh * 4]);
        }
        let (y_plane, rest) = data.split_at(w * h);
        let (u_plane, v_plane) = rest.split_at(cw * ch);
        let mut out = Vec::with_capacity(ow * oh * 4);
        for oy in 0..oh {
            let sy = ((oy as f64 + 0.5) * step) as usize;
            let sy = sy.min(h - 1);
            for ox in 0..ow {
                let ox = if mirror { ow - 1 - ox } else { ox };
                let sx = (((ox as f64 + 0.5) * step) as usize).min(w - 1);
                // BT.601 limited range
                let y = (y_plane[sy * w + sx] as f32 - 16.0) * 1.164;
                let u = u_plane[(sy / 2) * cw + sx / 2] as f32 - 128.0;
                let v = v_plane[(sy / 2) * cw + sx / 2] as f32 - 128.0;
                let r = y + 1.596 * v;
                let g = y - 0.392 * u - 0.813 * v;
                let b = y + 2.017 * u;
                out.extend_from_slice(&[r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, 255]);
            }
        }
        (ow as u32, oh as u32, out)
    }
}

/// Size and chroma layout from a `YUV4MPEG2 W… H… C…` stream header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Y4mHeader {
    pub width: u32,
    pub height: u32,
}

impl Y4mHeader {
    /// Parse the header line (without the trailing newline). Only 4:2:0 streams are accepted.
    pub fn parse(line: &str) -> Option<Y4mHeader> {
        let mut it = line.split_ascii_whitespace();
        if it.next()? != "YUV4MPEG2" {
            return None;
        }
        let (mut width, mut height) = (None, None);
        for tok in it {
            let (tag, val) = tok.split_at(1);
            match tag {
                "W" => width = val.parse().ok(),
                "H" => height = val.parse().ok(),
                "C" if !val.starts_with("420") => return None,
                _ => {}
            }
        }
        Some(Y4mHeader { width: width.filter(|w| *w > 0)?, height: height.filter(|h| *h > 0)? })
    }

    /// Bytes of one yuv420p frame.
    pub fn frame_len(&self) -> usize {
        let (w, h) = (self.width as usize, self.height as usize);
        w * h + 2 * (w.div_ceil(2) * h.div_ceil(2))
    }
}

/// ffmpeg output arguments shared by every capture attempt: scale down to [`MAX_WIDTH`], keep
/// even dimensions, emit yuv420p frames on stdout. `-r` keeps the output rate sane whatever
/// timestamps the device reports (the repeater only ever keeps the latest frame).
pub fn capture_output_args() -> Vec<String> {
    let scale = format!("scale=w='trunc(min({MAX_WIDTH},iw)/2)*2':h=-2");
    ["-an", "-vf", &scale, "-r", &CAMERA_FPS.to_string(), "-pix_fmt", "yuv420p", "-f", "yuv4mpegpipe", "pipe:1"].map(String::from).to_vec()
}

/// Input arguments to try in order: some cameras refuse a frame rate or size they do not
/// offer, so the later attempts ask for less.
#[cfg(target_os = "macos")]
pub fn capture_input_attempts(device: &str) -> Vec<Vec<String>> {
    let input = format!("{device}:none");
    let fps = CAMERA_FPS.to_string();
    [
        vec!["-framerate", &fps, "-video_size", "1280x720", "-pixel_format", "nv12"],
        vec!["-framerate", &fps, "-pixel_format", "nv12"],
        vec![],
    ]
    .into_iter()
    .map(|opts| {
        let mut a: Vec<String> = ["-f", "avfoundation"].map(String::from).to_vec();
        a.extend(opts.into_iter().map(String::from));
        a.extend(["-i".into(), input.clone()]);
        a
    })
    .collect()
}

#[cfg(target_os = "linux")]
pub fn capture_input_attempts(device: &str) -> Vec<Vec<String>> {
    let fps = CAMERA_FPS.to_string();
    [vec!["-framerate", &fps, "-video_size", "1280x720"], vec![]]
        .into_iter()
        .map(|opts| {
            let mut a: Vec<String> = ["-f", "v4l2"].map(String::from).to_vec();
            a.extend(opts.into_iter().map(String::from));
            a.extend(["-i".into(), device.to_string()]);
            a
        })
        .collect()
}

/// DirectShow: `device` is `video=<name>` (see [`crate::discover::camera_input`]).
#[cfg(target_os = "windows")]
pub fn capture_input_attempts(device: &str) -> Vec<Vec<String>> {
    let fps = CAMERA_FPS.to_string();
    [vec!["-framerate", &fps, "-video_size", "1280x720"], vec!["-video_size", "1280x720"], vec![]]
        .into_iter()
        .map(|opts| {
            let mut a: Vec<String> = ["-f", "dshow", "-rtbufsize", "64M"].map(String::from).to_vec();
            a.extend(opts.into_iter().map(String::from));
            a.extend(["-i".into(), device.to_string()]);
            a
        })
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod imp {
    use std::io::{BufRead, BufReader, Read};
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use crossbeam_channel::{bounded, unbounded, Sender};
    use parking_lot::Mutex;

    use super::{capture_input_attempts, capture_output_args, CameraFrame, Y4mHeader, CAMERA_FPS};
    use crate::display::target_bitrate;
    use crate::ffmpeg::FfmpegProc;
    use crate::log::RecLog;
    use crate::repeater::{Cmd, FrameStore, RawInput, Repeater};
    use crate::{Error, Result};

    const COMPONENT: &str = "CameraRecorder";
    /// How long a camera may take to deliver its first frame (they can be slow to wake up).
    const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(8);

    /// A running webcam capture: ffmpeg plus a reader thread keeping the latest frame. Shared
    /// (`Arc`) between the live preview and the [`CameraRecorder`]; the camera turns off when
    /// the last handle is dropped.
    pub struct CameraCapture {
        proc: Mutex<Option<FfmpegProc>>,
        reader: Mutex<Option<JoinHandle<()>>>,
        store: Arc<FrameStore>,
        /// Device as stored in the metadata (`configuration.deviceId`).
        pub device_id: String,
        pub width: u32,
        pub height: u32,
    }

    impl CameraCapture {
        /// Open camera `device_id` (an id from `discover().cameras`) and wait for its first
        /// frame. Blocks for as long as the camera takes to wake up (up to a few seconds).
        pub fn open(device_id: &str, log: RecLog) -> Result<Arc<Self>> {
            let input = crate::discover::camera_input(device_id).ok_or_else(|| Error::InvalidConfig(format!("camera not found: {device_id}")))?;
            let mut last_err = String::from("no frames received");
            for (i, mut args) in capture_input_attempts(&input).into_iter().enumerate() {
                args.extend(capture_output_args());
                let (proc, stdout) = FfmpegProc::spawn_reading(&args, Path::new("."), log.clone(), COMPONENT)?;
                let (header_tx, header_rx) = bounded(1);
                let reader_log = log.clone();
                let reader = std::thread::Builder::new()
                    .name("sf-camera-reader".into())
                    .spawn(move || read_frames(stdout, header_tx, reader_log))
                    .map_err(|source| Error::Io { path: "thread".into(), source })?;
                match header_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
                    Ok((header, store)) if store.wait_first(FIRST_FRAME_TIMEOUT) => {
                        log.notice(COMPONENT, format!("Capture started. device={device_id} input={input} attempt={i} size={}x{}", header.width, header.height));
                        return Ok(Arc::new(CameraCapture {
                            proc: Mutex::new(Some(proc)),
                            reader: Mutex::new(Some(reader)),
                            store,
                            device_id: device_id.to_string(),
                            width: header.width,
                            height: header.height,
                        }));
                    }
                    Ok(_) => last_err = "the camera sent no frames".into(),
                    // ffmpeg could not open the device with these options (or timed out)
                    Err(_) => log.warning(COMPONENT, format!("Camera attempt {i} failed; trying the next settings.")),
                }
                proc.kill();
                let _ = reader.join();
            }
            last_err = format!("could not open camera {device_id}: {last_err}");
            log.error(COMPONENT, &last_err);
            Err(Error::Capture(last_err))
        }

        /// The most recent frame.
        pub fn latest(&self) -> Option<CameraFrame> {
            let data = self.store.latest.lock().clone()?;
            Some(CameraFrame { width: self.width, height: self.height, yuv420p: data })
        }

        /// Frames delivered by the camera so far.
        pub fn frames_received(&self) -> u64 {
            self.store.received.load(Ordering::Relaxed)
        }
    }

    impl Drop for CameraCapture {
        fn drop(&mut self) {
            if let Some(p) = self.proc.lock().take() {
                p.kill();
            }
            if let Some(t) = self.reader.lock().take() {
                let _ = t.join();
            }
        }
    }

    /// Encodes a [`CameraCapture`] into `channel-N-camera-S.m3u8`.
    pub struct CameraRecorder {
        capture: Arc<CameraCapture>,
        tx: Sender<Cmd>,
        repeater: Option<JoinHandle<()>>,
        /// Recorder id, e.g. `channel-4-camera`.
        pub channel: String,
    }

    impl CameraRecorder {
        /// Start the writer for `capture`. Frames are kept, not written, until
        /// [`begin_session`](Self::begin_session).
        pub fn start(capture: Arc<CameraCapture>, channel_index: usize, quality: f64, log: RecLog) -> Result<Self> {
            let channel = format!("channel-{channel_index}-camera");
            let bitrate = target_bitrate(capture.width, capture.height, quality);
            log.notice(COMPONENT, format!("Camera channel {channel}. device={} size={}x{} bitrate={bitrate}", capture.device_id, capture.width, capture.height));
            let (tx, rx) = unbounded();
            let rep = Repeater {
                rx,
                store: capture.store.clone(),
                log,
                component: COMPONENT,
                fps: CAMERA_FPS,
                input: RawInput { pix_fmt: "yuv420p", width: capture.width, height: capture.height, filter: None },
                bitrate,
                encoder: crate::display::video_encoder(),
                channel: channel.clone(),
            };
            let repeater = std::thread::Builder::new()
                .name("sf-camera-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            Ok(CameraRecorder { capture, tx, repeater: Some(repeater), channel })
        }

        pub fn device_id(&self) -> &str {
            &self.capture.device_id
        }

        /// Spawn the encoder for session `index` and start feeding frames.
        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::Begin { index, dir: dir.into(), reply }).map_err(|_| Error::Capture("camera repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("camera repeater stopped".into()))?
        }

        /// Finish the session's playlist. Returns the number of frames written.
        pub fn end_session(&self) -> Result<u64> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::End { reply }).map_err(|_| Error::Capture("camera repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("camera repeater stopped".into()))?
        }

        /// Frames delivered by the camera so far.
        pub fn frames_received(&self) -> u64 {
            self.capture.frames_received()
        }

        /// Stop the writer. The camera itself stays on while other handles to the capture
        /// (e.g. the preview) exist.
        pub fn stop(&mut self) {
            let _ = self.tx.send(Cmd::Stop);
            if let Some(t) = self.repeater.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for CameraRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// Read the Y4M stream: hand the header (with a frame store sized for it) to `header_tx`,
    /// then keep the latest frame in the store until ffmpeg goes away.
    fn read_frames(out: impl Read, header_tx: Sender<(Y4mHeader, Arc<FrameStore>)>, log: RecLog) {
        let mut r = BufReader::with_capacity(1 << 20, out);
        let mut line = Vec::new();
        if r.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
            return;
        }
        let Some(header) = Y4mHeader::parse(String::from_utf8_lossy(&line).trim_end()) else {
            log.error(COMPONENT, format!("Unexpected camera stream header: {}", String::from_utf8_lossy(&line).trim_end()));
            return;
        };
        let store = Arc::new(FrameStore::new(header.width as usize, header.height as usize));
        if header_tx.send((header, store.clone())).is_err() {
            return;
        }
        let len = header.frame_len();
        loop {
            // every frame starts with `FRAME[ params]\n`
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if !line.starts_with(b"FRAME") => {
                    log.error(COMPONENT, "Camera stream out of sync; stopping the camera.");
                    break;
                }
                Ok(_) => {}
            }
            let mut buf = vec![0u8; len];
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            store.put(buf);
        }
        log.notice(COMPONENT, format!("Camera stream ended after {} frames.", store.received.load(Ordering::Relaxed)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_y4m_header() {
        let h = Y4mHeader::parse("YUV4MPEG2 W1280 H720 F30:1 Ip A0:0 C420jpeg XYSCSS=420JPEG").unwrap();
        assert_eq!(h, Y4mHeader { width: 1280, height: 720 });
        assert_eq!(h.frame_len(), 1280 * 720 * 3 / 2);
        assert!(Y4mHeader::parse("YUV4MPEG2 W640 H480 C422").is_none());
        assert!(Y4mHeader::parse("RIFF W640 H480").is_none());
        assert!(Y4mHeader::parse("YUV4MPEG2 W640").is_none());
    }

    #[test]
    fn preview_is_downsampled_and_mirrored() {
        // 4×2 frame: left half black, right half white (grey chroma)
        let mut data = vec![16, 16, 235, 235, 16, 16, 235, 235];
        data.extend([128; 4]);
        let f = CameraFrame { width: 4, height: 2, yuv420p: Arc::new(data) };
        let (w, h, px) = f.to_rgba(2, false);
        assert_eq!((w, h), (2, 1));
        assert!(px[0] < 10 && px[4] > 245, "{px:?}");
        let (_, _, m) = f.to_rgba(2, true);
        assert!(m[0] > 245 && m[4] < 10, "{m:?}");
    }

    #[test]
    fn output_is_scaled_yuv420p_on_stdout() {
        let a = capture_output_args();
        assert_eq!(a.last().unwrap(), "pipe:1");
        assert!(a.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "yuv420p"));
        assert!(a.iter().any(|s| s.contains("min(1280,iw)")));
    }
}
