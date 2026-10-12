//! Display recorder: ScreenCaptureKit → frame repeater → ffmpeg HLS/fMP4 writer.
//!
//! Output per session `S`: `channel-1-display-S.m3u8`, `channel-1-display-S-0000.mp4`
//! (init segment) and `channel-1-display-S-NNNN.m4s` (~2 s segments, H.264 High, yuv420p,
//! bt709), i.e. the layout polyrecorder's `AVAssetWriter` produces.
//!
//! ScreenCaptureKit only delivers a frame when the screen changes, so a repeater thread
//! keeps the last frame and feeds ffmpeg at a constant frame rate; if the encoder falls
//! behind, missed ticks are caught up with duplicate frames so the media duration always
//! equals wall time (and therefore the input event timeline).

use sf_project::schema::common::Rect;

use crate::config::{CaptureTarget, RecordingConfig};
use crate::discover::Discovery;
use crate::{Error, Result};

/// Screen Studio caps the output width at 4096 px.
pub const MAX_WIDTH_PX: u32 = 4096;
/// Average bitrate polyrecorder chose for 2400×1800 at quality 0.85.
const REFERENCE_BITRATE: f64 = 16_943_358.0;
const REFERENCE_PIXELS: f64 = 2400.0 * 1800.0;

/// Geometry of the display capture, derived from the config and the display list.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayPlan {
    pub display_id: u32,
    /// Display-relative crop in points (`None` = whole display).
    pub crop: Option<Rect>,
    /// Recorded region in global points.
    pub bounds: Rect,
    /// Backing scale of the display.
    pub scale: f64,
    pub width_px: u32,
    pub height_px: u32,
    /// Points per output pixel (`bounds.width / width_px`).
    pub recording_scale: f64,
    pub refresh_rate: f64,
}

fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// Work out what to capture and at which pixel size.
pub fn plan(cfg: &RecordingConfig, discovery: &Discovery) -> Result<DisplayPlan> {
    let (display, region): (_, Option<Rect>) = match &cfg.target {
        CaptureTarget::Display { display_id } => {
            (discovery.display(*display_id).ok_or_else(|| Error::InvalidConfig(format!("unknown display {display_id}")))?, None)
        }
        CaptureTarget::Area { display_id, rect_points } => (
            discovery.display(*display_id).ok_or_else(|| Error::InvalidConfig(format!("unknown display {display_id}")))?,
            Some(*rect_points),
        ),
        CaptureTarget::Window { window_id } => {
            let w = discovery.window(*window_id).ok_or_else(|| Error::InvalidConfig(format!("unknown window {window_id}")))?;
            let cx = w.bounds.x + w.bounds.width / 2.0;
            let cy = w.bounds.y + w.bounds.height / 2.0;
            let d = discovery
                .displays
                .iter()
                .find(|d| {
                    let b = &d.bounds_points;
                    cx >= b.x && cx < b.x + b.width && cy >= b.y && cy < b.y + b.height
                })
                .or_else(|| discovery.main_display())
                .ok_or_else(|| Error::InvalidConfig("no display".into()))?;
            (d, Some(w.bounds))
        }
    };
    let db = display.bounds_points;
    let (bounds, crop) = match region {
        None => (db, None),
        Some(r) => {
            let b = intersect(&r, &db).ok_or_else(|| Error::InvalidConfig("capture area is outside the display".into()))?;
            // integral points like Screen Studio's picker produces
            let b = Rect { x: b.x.floor(), y: b.y.floor(), width: b.width.round().max(2.0), height: b.height.round().max(2.0) };
            (b, Some(Rect { x: b.x - db.x, y: b.y - db.y, width: b.width, height: b.height }))
        }
    };
    let scale = display.scale.max(1.0);
    let mut width_px = (bounds.width * scale).round() as u32;
    let mut height_px = (bounds.height * scale).round() as u32;
    if width_px > MAX_WIDTH_PX {
        let f = MAX_WIDTH_PX as f64 / width_px as f64;
        width_px = MAX_WIDTH_PX;
        height_px = (height_px as f64 * f).round() as u32;
    }
    width_px = (width_px & !1).max(2);
    height_px = (height_px & !1).max(2);
    Ok(DisplayPlan {
        display_id: display.id,
        crop,
        bounds,
        scale,
        width_px,
        height_px,
        recording_scale: bounds.width / width_px as f64,
        refresh_rate: display.refresh_rate,
    })
}

/// Target H.264 bitrate: Screen Studio's ~17 Mb/s for 2400×1800 scaled by pixel count and
/// quality, clamped to a sane range.
pub fn target_bitrate(width: u32, height: u32, quality: f64) -> u64 {
    let px = width as f64 * height as f64;
    let q = (quality / 0.85).clamp(0.3, 1.5);
    (REFERENCE_BITRATE * px / REFERENCE_PIXELS * q).clamp(1.5e6, 80e6) as u64
}

/// ffmpeg arguments for one HLS session (relative file names; run with cwd = output dir).
pub fn hls_args(width: u32, height: u32, fps: u32, bitrate: u64, encoder: &str, session: usize) -> Vec<String> {
    let base = format!("channel-1-display-{session}");
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-video_size".into(),
        format!("{width}x{height}"),
        "-framerate".into(),
        fps.to_string(),
        "-i".into(),
        "pipe:0".into(),
        "-c:v".into(),
        encoder.into(),
    ];
    if encoder == "libx264" {
        args.extend(["-preset", "veryfast", "-tune", "zerolatency"].map(String::from));
    } else {
        args.extend(["-allow_sw", "1", "-realtime", "1"].map(String::from));
    }
    args.extend(
        [
            "-profile:v", "high", "-b:v", &bitrate.to_string(), "-g", "120", "-pix_fmt", "yuv420p",
            "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv",
            // videotoolbox does not always put the colour description into the SPS VUI; rewrite it.
            "-bsf:v", "h264_metadata=video_full_range_flag=0:colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1",
            "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
            "-hls_flags", "independent_segments", "-hls_list_size", "0", "-start_number", "1",
            "-hls_fmp4_init_filename", &format!("{base}-0000.mp4"),
            "-hls_segment_filename", &format!("{base}-%04d.m4s"),
            &format!("{base}.m3u8"),
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args
}

#[cfg(target_os = "macos")]
pub use imp::DisplayRecorder;

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
    use parking_lot::Mutex;
    use screencapturekit::cg::CGRect;
    use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus};
    use screencapturekit::cv::CVPixelBufferLockFlags;
    use screencapturekit::shareable_content::SCShareableContent;
    use screencapturekit::stream::configuration::pixel_format::PixelFormat;
    use screencapturekit::stream::configuration::SCStreamConfiguration;
    use screencapturekit::stream::content_filter::SCContentFilter;
    use screencapturekit::stream::delegate_trait::StreamCallbacks;
    use screencapturekit::stream::output_trait::SCStreamOutputTrait;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use screencapturekit::stream::sc_stream::SCStream;

    use super::{hls_args, target_bitrate, DisplayPlan};
    use crate::audio::SystemAudioHandler;
    use crate::config::RecordingConfig;
    use crate::ffmpeg::{has_encoder, FfmpegProc};
    use crate::log::RecLog;
    use crate::permissions;
    use crate::{Error, Result};

    const COMPONENT: &str = "DisplayRecorder";

    /// Latest captured frame (tightly packed BGRA, `width*4` bytes per row).
    struct FrameStore {
        latest: Mutex<Option<Arc<Vec<u8>>>>,
        width: usize,
        height: usize,
        received: AtomicU64,
    }

    struct FrameHandler {
        store: Arc<FrameStore>,
    }

    impl SCStreamOutputTrait for FrameHandler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }
            if matches!(sample.frame_status(), Some(SCFrameStatus::Idle | SCFrameStatus::Blank | SCFrameStatus::Suspended | SCFrameStatus::Stopped)) {
                return;
            }
            let Some(pb) = sample.pixel_buffer() else { return };
            let Ok(guard) = pb.lock(CVPixelBufferLockFlags::READ_ONLY) else { return };
            let (w, h, stride) = (guard.width(), guard.height(), guard.bytes_per_row());
            if w != self.store.width || h != self.store.height {
                return;
            }
            let Some(src) = (unsafe { guard.as_slice() }) else { return };
            let row = w * 4;
            let mut buf = Vec::with_capacity(row * h);
            for y in 0..h {
                let start = y * stride;
                if start + row > src.len() {
                    return;
                }
                buf.extend_from_slice(&src[start..start + row]);
            }
            *self.store.latest.lock() = Some(Arc::new(buf));
            self.store.received.fetch_add(1, Ordering::Relaxed);
        }
    }

    enum Cmd {
        Begin { index: usize, dir: PathBuf, reply: Sender<Result<()>> },
        End { reply: Sender<Result<u64>> },
        Stop,
    }

    /// ScreenCaptureKit stream plus the HLS writer.
    pub struct DisplayRecorder {
        stream: SCStream,
        store: Arc<FrameStore>,
        tx: Sender<Cmd>,
        repeater: Option<JoinHandle<()>>,
        error: Arc<Mutex<Option<String>>>,
        pub plan: DisplayPlan,
        pub encoder: &'static str,
    }

    impl DisplayRecorder {
        /// Build the content filter and stream and start capturing (frames are kept, not
        /// written, until [`begin_session`](Self::begin_session)). `system_audio` attaches a
        /// system-audio handler to the same stream.
        pub fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, system_audio: Option<SystemAudioHandler>) -> Result<Self> {
            if !permissions::screen_recording().is_granted() {
                return Err(Error::PermissionDenied("screenRecording"));
            }
            let content = SCShareableContent::get().map_err(|e| Error::Capture(format!("shareable content: {e}")))?;
            let display = content
                .displays()
                .into_iter()
                .find(|d| d.display_id() == plan.display_id)
                .ok_or_else(|| Error::InvalidConfig(format!("display {} not capturable", plan.display_id)))?;
            let windows = content.windows();
            let excluded: Vec<_> = windows
                .iter()
                .filter(|w| {
                    cfg.exclude_window_ids.contains(&w.window_id())
                        || (cfg.hide_desktop_icons
                            && w.window_layer() < 0
                            && w.owning_application().map(|a| a.bundle_identifier() == "com.apple.finder").unwrap_or(false))
                })
                .collect();
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&excluded)
                .build()
                .map_err(|e| Error::Capture(format!("content filter: {e}")))?;

            let mut config = SCStreamConfiguration::new()
                .with_width(plan.width_px)
                .with_height(plan.height_px)
                .with_pixel_format(PixelFormat::BGRA)
                .with_fps(cfg.fps.max(1))
                .with_queue_depth(8)
                .with_shows_cursor(false);
            if let Some(c) = &plan.crop {
                config = config.with_source_rect(CGRect::new(c.x, c.y, c.width, c.height));
            }
            if system_audio.is_some() {
                config = config
                    .with_captures_audio(true)
                    .with_sample_rate(48_000)
                    .with_channel_count(2)
                    .with_excludes_current_process_audio(true);
            }

            let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let err_slot = error.clone();
            let err_log = log.clone();
            let callbacks = StreamCallbacks::new().on_error(move |e| {
                err_log.error(COMPONENT, format!("Stream error: {e}"));
                *err_slot.lock() = Some(e.to_string());
            });
            let mut stream = SCStream::new_with_delegate(&filter, &config, callbacks).map_err(|e| Error::Capture(format!("SCStream: {e}")))?;
            let store = Arc::new(FrameStore {
                latest: Mutex::new(None),
                width: plan.width_px as usize,
                height: plan.height_px as usize,
                received: AtomicU64::new(0),
            });
            stream
                .add_output_handler(FrameHandler { store: store.clone() }, SCStreamOutputType::Screen)
                .map_err(|e| Error::Capture(format!("output handler: {e}")))?;
            if let Some(h) = system_audio {
                stream.add_output_handler(h, SCStreamOutputType::Audio).map_err(|e| Error::Capture(format!("audio handler: {e}")))?;
            }
            log.debug(
                COMPONENT,
                format!(
                    "Creating SCStream sourceRect={:?} width={} height={} queueDepth=8 fps={} excludedWindows={}",
                    plan.crop, plan.width_px, plan.height_px, cfg.fps, excluded.len()
                ),
            );
            stream.start_capture().map_err(|e| Error::Capture(format!("start capture: {e}")))?;

            // Wait for the first frame so a session never starts without content.
            let t = Instant::now();
            while store.received.load(Ordering::Relaxed) == 0 && t.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if store.received.load(Ordering::Relaxed) == 0 {
                let _ = stream.stop_capture();
                return Err(Error::Capture("no frames received from ScreenCaptureKit".into()));
            }

            let encoder: &'static str = if has_encoder("h264_videotoolbox") { "h264_videotoolbox" } else { "libx264" };
            let bitrate = target_bitrate(plan.width_px, plan.height_px, cfg.video_quality);
            log.notice(COMPONENT, format!("Capture started. encoder={encoder} bitrate={bitrate} size={}x{} recordingScale={}", plan.width_px, plan.height_px, plan.recording_scale));

            let (tx, rx) = unbounded();
            let rep = Repeater {
                rx,
                store: store.clone(),
                log: log.clone(),
                fps: cfg.fps.max(1),
                width: plan.width_px,
                height: plan.height_px,
                bitrate,
                encoder,
            };
            let repeater = std::thread::Builder::new()
                .name("sf-frame-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            Ok(DisplayRecorder { stream, store, tx, repeater: Some(repeater), error, plan, encoder })
        }

        /// Spawn ffmpeg for session `index` and start feeding frames.
        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::Begin { index, dir: dir.into(), reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
        }

        /// Stop feeding, finish the playlist. Returns the number of frames written.
        pub fn end_session(&self) -> Result<u64> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::End { reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
        }

        /// Frames delivered by ScreenCaptureKit so far.
        pub fn frames_received(&self) -> u64 {
            self.store.received.load(Ordering::Relaxed)
        }

        /// Last stream error reported by ScreenCaptureKit, if any.
        pub fn error(&self) -> Option<String> {
            self.error.lock().clone()
        }

        /// Stop the stream and the repeater.
        pub fn stop(&mut self) {
            let _ = self.tx.send(Cmd::Stop);
            if let Some(t) = self.repeater.take() {
                let _ = t.join();
            }
            let _ = self.stream.stop_capture();
        }
    }

    impl Drop for DisplayRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct Repeater {
        rx: Receiver<Cmd>,
        store: Arc<FrameStore>,
        log: RecLog,
        fps: u32,
        width: u32,
        height: u32,
        bitrate: u64,
        encoder: &'static str,
    }

    struct Active {
        proc: FfmpegProc,
        t0: Instant,
        written: u64,
        failed: AtomicBool,
    }

    impl Repeater {
        fn run(self) {
            let mut active: Option<Active> = None;
            let frame_dur = Duration::from_secs_f64(1.0 / self.fps as f64);
            loop {
                let cmd = match &active {
                    None => self.rx.recv().ok(),
                    Some(a) => {
                        let due = a.t0 + frame_dur * (a.written as u32);
                        let now = Instant::now();
                        if due > now {
                            match self.rx.recv_timeout(due - now) {
                                Ok(c) => Some(c),
                                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                                Err(_) => break,
                            }
                        } else {
                            match self.rx.try_recv() {
                                Ok(c) => Some(c),
                                Err(crossbeam_channel::TryRecvError::Empty) => None,
                                Err(_) => break,
                            }
                        }
                    }
                };
                match cmd {
                    Some(Cmd::Begin { index, dir, reply }) => {
                        if let Some(a) = active.take() {
                            let _ = a.proc.finish();
                        }
                        let r = self.spawn(index, &dir).map(|p| {
                            active = Some(Active { proc: p, t0: Instant::now(), written: 0, failed: AtomicBool::new(false) });
                        });
                        let _ = reply.send(r);
                    }
                    Some(Cmd::End { reply }) => {
                        let r = match active.take() {
                            Some(a) => {
                                let n = a.written;
                                a.proc.finish().map(|_| n)
                            }
                            None => Ok(0),
                        };
                        let _ = reply.send(r);
                    }
                    Some(Cmd::Stop) => break,
                    None => {
                        if let Some(a) = active.as_mut() {
                            if a.failed.load(Ordering::Relaxed) {
                                // ffmpeg died; keep counting ticks so End returns promptly
                                a.written += 1;
                                continue;
                            }
                            let frame = self.store.latest.lock().clone();
                            if let Some(f) = frame {
                                if let Err(e) = a.proc.write(&f) {
                                    self.log.error(COMPONENT, format!("Frame write failed: {e}"));
                                    a.failed.store(true, Ordering::Relaxed);
                                }
                            }
                            a.written += 1;
                        }
                    }
                }
            }
            if let Some(a) = active.take() {
                let _ = a.proc.finish();
            }
        }

        fn spawn(&self, index: usize, dir: &Path) -> Result<FfmpegProc> {
            let args = hls_args(self.width, self.height, self.fps, self.bitrate, self.encoder, index);
            self.log.notice(COMPONENT, format!("Starting video session {index}. file=channel-1-display-{index}.m3u8"));
            FfmpegProc::spawn(&args, dir, self.log.clone(), COMPONENT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{DisplayInfo, WindowInfo};

    fn disc() -> Discovery {
        Discovery {
            displays: vec![DisplayInfo {
                id: 3,
                bounds_points: Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1440.0 },
                scale: 2.0,
                refresh_rate: 60.0,
                is_main: true,
                is_builtin: false,
                name: "Studio Display".into(),
            }],
            windows: vec![WindowInfo {
                id: 7,
                title: "w".into(),
                app_name: "a".into(),
                bundle_id: "b".into(),
                pid: 1,
                is_on_screen: true,
                bounds: Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 },
            }],
            microphones: vec![],
            cameras: vec![],
        }
    }

    #[test]
    fn full_display_is_capped_like_screen_studio() {
        let cfg = RecordingConfig::new("/tmp/x", CaptureTarget::Display { display_id: 3 });
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!((p.width_px, p.height_px), (4096, 2304));
        assert!((p.recording_scale - 0.625).abs() < 1e-9);
        assert!(p.crop.is_none());
    }

    #[test]
    fn window_becomes_display_crop() {
        let cfg = RecordingConfig::new("/tmp/x", CaptureTarget::Window { window_id: 7 });
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!((p.width_px, p.height_px), (2400, 1800));
        assert_eq!(p.recording_scale, 0.5);
        assert_eq!(p.crop.unwrap(), Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 });
        assert_eq!(p.bounds, Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 });
    }

    #[test]
    fn area_is_clamped_to_display() {
        let cfg = RecordingConfig::new(
            "/tmp/x",
            CaptureTarget::Area { display_id: 3, rect_points: Rect { x: -10.0, y: 0.0, width: 100.0, height: 50.4 } },
        );
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!(p.crop.unwrap(), Rect { x: 0.0, y: 0.0, width: 90.0, height: 50.0 });
        assert_eq!((p.width_px, p.height_px), (180, 100));
    }

    #[test]
    fn bitrate_scales_with_pixels() {
        assert_eq!(target_bitrate(2400, 1800, 0.85), 16_943_358);
        assert!(target_bitrate(1200, 900, 0.85) < 5_000_000);
        let args = hls_args(2400, 1800, 60, 1, "h264_videotoolbox", 2);
        assert!(args.iter().any(|a| a == "channel-1-display-2-0000.mp4"));
        assert!(args.last().unwrap() == "channel-1-display-2.m3u8");
    }
}
