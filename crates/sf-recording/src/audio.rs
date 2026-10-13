//! Audio recorders: microphone (`cpal`) and system audio (ScreenCaptureKit), both written
//! as AAC-LC 48 kHz mono ≈64 kb/s `channel-N-<kind>-S.m4a` through ffmpeg.
//!
//! [`PcmSink`] is the shared writer: float PCM chunks go through a channel to a thread that
//! owns the ffmpeg process for the current session. Every chunk carries the process time at
//! which its first frame was captured; the first chunk of a session is aligned to the
//! session start (silence inserted or leading samples trimmed) so audio and video sessions
//! stay in sync regardless of capture latency.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crate::clock;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::ffmpeg::FfmpegProc;
use crate::log::RecLog;
use crate::{Error, Result};

enum Cmd {
    Begin { path: PathBuf, reply: Sender<Result<()>> },
    Samples { data: Vec<f32>, capture_start_ms: f64 },
    End { end_ms: f64, reply: Sender<Result<()>> },
    Stop,
}

/// Float PCM → AAC `.m4a` writer with per-session ffmpeg processes.
#[derive(Clone)]
pub struct PcmSink {
    tx: Sender<Cmd>,
}

impl std::fmt::Debug for PcmSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PcmSink")
    }
}

/// Owns the writer thread; dropping it stops the sink.
pub struct PcmSinkHandle {
    sink: PcmSink,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PcmSinkHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PcmSinkHandle")
    }
}

struct Writer {
    rx: Receiver<Cmd>,
    log: RecLog,
    label: String,
    rate: u32,
    channels: u16,
    proc: Option<FfmpegProc>,
    /// Process time (ms) at which the current session began.
    session_start_ms: Option<f64>,
    aligned: bool,
    /// Frames written to the current session's ffmpeg.
    frames_written: u64,
}

impl Writer {
    fn run(mut self) {
        while let Ok(cmd) = self.rx.recv() {
            match cmd {
                Cmd::Begin { path, reply } => {
                    let r = self.begin(&path);
                    let _ = reply.send(r);
                }
                Cmd::Samples { data, capture_start_ms } => self.samples(&data, capture_start_ms),
                Cmd::End { end_ms, reply } => {
                    let r = self.end(Some(end_ms));
                    let _ = reply.send(r);
                }
                Cmd::Stop => break,
            }
        }
        if let Err(e) = self.end(None) {
            self.log.warning(&self.label, format!("Could not finish audio file: {e}"));
        }
    }

    fn begin(&mut self, path: &Path) -> Result<()> {
        self.end(None)?;
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = path.parent().unwrap_or(Path::new("."));
        let layout = if self.channels == 1 { "mono" } else { "stereo" };
        let args: Vec<String> = [
            "-y", "-f", "f32le", "-ar", &self.rate.to_string(), "-ac", &self.channels.to_string(), "-ch_layout", layout, "-i", "pipe:0",
            "-c:a", "aac", "-b:a", "64k", "-ac", "1", "-ar", "48000", "-f", "ipod", &file,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        self.proc = Some(FfmpegProc::spawn(&args, dir, self.log.clone(), &self.label)?);
        self.session_start_ms = Some(clock::process_time_ms());
        self.aligned = false;
        self.frames_written = 0;
        self.log.notice(&self.label, format!("Started audio session. file={file} rate={} channels={}", self.rate, self.channels));
        Ok(())
    }

    /// Finish the session. With `end_ms` the file is padded with silence up to the session's
    /// exact length (the last chunk in flight when the session ends is never delivered).
    fn end(&mut self, end_ms: Option<f64>) -> Result<()> {
        if let (Some(start_ms), Some(end_ms), Some(p)) = (self.session_start_ms, end_ms, self.proc.as_mut()) {
            let expected = ((end_ms - start_ms) / 1000.0 * self.rate as f64).round().max(0.0) as u64;
            let missing = expected.saturating_sub(self.frames_written).min(self.rate as u64 * 2);
            if missing > 0 {
                let _ = p.write(&vec![0u8; missing as usize * self.channels.max(1) as usize * 4]);
            }
        }
        self.session_start_ms = None;
        match self.proc.take() {
            Some(p) => p.finish(),
            None => Ok(()),
        }
    }

    fn samples(&mut self, data: &[f32], capture_start_ms: f64) {
        let Some(start_ms) = self.session_start_ms else { return };
        let ch = self.channels.max(1) as usize;
        let mut data = data;
        let mut lead_silence_frames = 0usize;
        if !self.aligned {
            let frames = data.len() / ch;
            let chunk_secs = frames as f64 / self.rate as f64;
            let offset = (capture_start_ms - start_ms) / 1000.0;
            if offset + chunk_secs <= 0.0 {
                // whole chunk predates the session
                return;
            }
            self.aligned = true;
            self.log.debug(&self.label, format!("Aligned first chunk. offsetMs={:.1}", offset * 1000.0));
            if offset > 0.0 {
                lead_silence_frames = (offset * self.rate as f64).round() as usize;
            } else {
                let skip = ((-offset) * self.rate as f64).round() as usize;
                data = &data[(skip * ch).min(data.len())..];
            }
        }
        let mut bytes = Vec::with_capacity((lead_silence_frames * ch + data.len()) * 4);
        bytes.resize(lead_silence_frames * ch * 4, 0);
        for s in data {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.frames_written += (lead_silence_frames + data.len() / ch) as u64;
        if let Some(p) = self.proc.as_mut() {
            if let Err(e) = p.write(&bytes) {
                self.log.error(&self.label, format!("Audio write failed, stopping session: {e}"));
                if let Some(p) = self.proc.take() {
                    p.kill();
                }
            }
        }
    }
}

impl PcmSink {
    /// Start a writer for `channels`-interleaved float PCM at `rate` Hz.
    pub fn spawn(rate: u32, channels: u16, log: RecLog, label: &str) -> Result<PcmSinkHandle> {
        let (tx, rx) = unbounded();
        let writer = Writer { rx, log, label: label.into(), rate, channels, proc: None, session_start_ms: None, aligned: false, frames_written: 0 };
        let thread = std::thread::Builder::new()
            .name(format!("sf-audio-{label}"))
            .spawn(move || writer.run())
            .map_err(|source| Error::Io { path: "thread".into(), source })?;
        Ok(PcmSinkHandle { sink: PcmSink { tx }, thread: Some(thread) })
    }

    /// Queue interleaved samples whose first frame was captured at process time
    /// `capture_start_ms` (see [`clock::process_time_ms`]).
    pub fn push(&self, data: Vec<f32>, capture_start_ms: f64) {
        let _ = self.tx.send(Cmd::Samples { data, capture_start_ms });
    }

    fn call(&self, cmd: impl FnOnce(Sender<Result<()>>) -> Cmd) -> Result<()> {
        let (reply, rx) = bounded(1);
        self.tx.send(cmd(reply)).map_err(|_| Error::Capture("audio writer stopped".into()))?;
        rx.recv().map_err(|_| Error::Capture("audio writer stopped".into()))?
    }

    /// Start writing `path` (`.m4a`).
    pub fn begin_session(&self, path: &Path) -> Result<()> {
        self.call(|reply| Cmd::Begin { path: path.into(), reply })
    }

    /// Finish the current file, padding it with silence up to `end_ms` (process time) so its
    /// duration equals the session's.
    pub fn end_session(&self, end_ms: f64) -> Result<()> {
        self.call(|reply| Cmd::End { end_ms, reply })
    }
}

impl PcmSinkHandle {
    pub fn sink(&self) -> PcmSink {
        self.sink.clone()
    }
    pub fn stop(&mut self) {
        let _ = self.sink.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PcmSinkHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Convert native-endian float bytes to samples.
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Interleave planar channel buffers.
pub fn interleave(planes: &[Vec<f32>]) -> Vec<f32> {
    let n = planes.iter().map(|p| p.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n * planes.len());
    for i in 0..n {
        for p in planes {
            out.push(p[i]);
        }
    }
    out
}

#[cfg(target_os = "macos")]
pub use imp::SystemAudioHandler;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use mic::MicRecorder;


/// Microphone capture through `cpal` (CoreAudio on macOS, ALSA/PulseAudio/PipeWire on Linux).
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod mic {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use crossbeam_channel::bounded;

    use super::{PcmSink, PcmSinkHandle};
    use crate::clock;
    use crate::log::RecLog;
    use crate::{Error, Result};

    const COMPONENT: &str = "MicrophoneRecorder";

    /// Microphone capture through CoreAudio (`cpal`).
    pub struct MicRecorder {
        sink_handle: PcmSinkHandle,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
        /// CoreAudio UID when known (what `metadata.json` stores as `deviceId`), else the name.
        pub device_id: String,
        pub device_name: String,
        pub sample_rate: u32,
        pub channels: u16,
    }

    /// `(uid, name, is_default)` of the inputs the OS reports: CoreAudio UIDs on macOS; on
    /// Linux the device name doubles as the id.
    #[cfg(target_os = "macos")]
    fn known_inputs() -> Vec<crate::discover::AudioDevice> {
        screencapturekit::audio_devices::AudioInputDevice::list()
            .into_iter()
            .map(|d| crate::discover::AudioDevice { id: d.id, name: d.name, is_default: d.is_default })
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn known_inputs() -> Vec<crate::discover::AudioDevice> {
        crate::discover::microphones()
    }

    fn resolve_device(selector: &str) -> Result<(cpal::Device, String, String)> {
        let host = cpal::default_host();
        let known = known_inputs();
        let (want_name, uid): (Option<String>, Option<String>) = if selector == "default" || selector.is_empty() {
            (None, known.iter().find(|d| d.is_default).map(|d| d.id.clone()))
        } else if let Some(k) = known.iter().find(|d| d.id == selector) {
            (Some(k.name.clone()), Some(k.id.clone()))
        } else {
            (Some(selector.to_string()), known.iter().find(|d| d.name == selector).map(|d| d.id.clone()))
        };
        let device = match &want_name {
            None => host.default_input_device(),
            Some(name) => host
                .input_devices()
                .map_err(|e| Error::Audio(e.to_string()))?
                .find(|d| d.description().map(|d| d.name() == name).unwrap_or(false)),
        }
        .ok_or_else(|| Error::Audio(format!("microphone not found: {selector}")))?;
        let name = device.description().map_err(|e| Error::Audio(e.to_string()))?.name().to_string();
        let id = uid.or_else(|| device.id().ok().map(|i| i.id().to_string())).unwrap_or_else(|| name.clone());
        Ok((device, name, id))
    }

    impl MicRecorder {
        /// Open the microphone and start streaming into the sink (nothing is written until
        /// [`begin_session`](Self::begin_session)).
        pub fn start(selector: &str, log: RecLog) -> Result<Self> {
            let (device, device_name, device_id) = resolve_device(selector)?;
            let supported = device.default_input_config().map_err(|e| Error::Audio(e.to_string()))?;
            let sample_rate = supported.sample_rate();
            let channels = supported.channels().clamp(1, 2);
            let config = cpal::StreamConfig { channels, sample_rate, buffer_size: cpal::BufferSize::Default };
            let in_channels = supported.channels() as usize;
            let sink_handle = PcmSink::spawn(sample_rate, channels, log.clone(), COMPONENT)?;
            let sink = sink_handle.sink();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let (ready_tx, ready_rx) = bounded::<Result<()>>(1);
            let err_log = log.clone();
            let mut full_config = config;
            full_config.channels = supported.channels();
            let thread = std::thread::Builder::new()
                .name("sf-mic".into())
                .spawn(move || {
                    let stream = device.build_input_stream(
                        full_config,
                        move |data: &[f32], info: &cpal::InputCallbackInfo| {
                            let now_ms = clock::process_time_ms();
                            let ts = info.timestamp();
                            let latency_ms = ts.callback.duration_since(ts.capture).as_secs_f64() * 1000.0;
                            let chunk_ms = (data.len() / in_channels.max(1)) as f64 / sample_rate as f64 * 1000.0;
                            let capture_start_ms = now_ms - latency_ms - chunk_ms;
                            let samples = if in_channels > 2 {
                                // keep the first two channels of multi-channel interfaces
                                let mut v = Vec::with_capacity(data.len() / in_channels * 2);
                                for frame in data.chunks_exact(in_channels) {
                                    v.push(frame[0]);
                                    v.push(frame[1]);
                                }
                                v
                            } else {
                                data.to_vec()
                            };
                            sink.push(samples, capture_start_ms);
                        },
                        move |e| err_log.error(COMPONENT, format!("Microphone stream error: {e}")),
                        None,
                    );
                    let stream = match stream.and_then(|s| s.play().map(|_| s)) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = ready_tx.send(Err(Error::Audio(e.to_string())));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    while !thread_stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    drop(stream);
                })
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            match ready_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(Error::Audio("timeout opening microphone".into())),
            }
            log.notice(COMPONENT, format!("Microphone opened. name=\"{device_name}\" id={device_id} rate={sample_rate} channels={channels}"));
            Ok(MicRecorder { sink_handle, stop, thread: Some(thread), device_id, device_name, sample_rate, channels })
        }

        pub fn begin_session(&self, path: &std::path::Path) -> Result<()> {
            self.sink_handle.sink().begin_session(path)
        }

        pub fn end_session(&self, end_ms: f64) -> Result<()> {
            self.sink_handle.sink().end_session(end_ms)
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            self.sink_handle.stop();
        }
    }

    impl Drop for MicRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use screencapturekit::cm::CMSampleBuffer;
    use screencapturekit::stream::output_trait::SCStreamOutputTrait;
    use screencapturekit::stream::output_type::SCStreamOutputType;

    use super::{bytes_to_f32, interleave, PcmSink};
    use crate::clock;

    /// `SCStreamOutputType::Audio` handler forwarding system audio (48 kHz stereo float)
    /// into a [`PcmSink`].
    pub struct SystemAudioHandler {
        pub sink: PcmSink,
    }

    impl SCStreamOutputTrait for SystemAudioHandler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            let Ok(list) = sample.audio_buffer_list() else { return };
            let n = list.num_buffers();
            let samples = if n >= 2 {
                let planes: Vec<Vec<f32>> = (0..2).filter_map(|i| list.get(i)).map(|b| bytes_to_f32(b.data())).collect();
                interleave(&planes)
            } else if let Some(b) = list.get(0) {
                let data = bytes_to_f32(b.data());
                match b.number_channels {
                    1 => data.iter().flat_map(|s| [*s, *s]).collect(),
                    2 => data,
                    c => {
                        let c = c as usize;
                        data.chunks_exact(c).flat_map(|f| [f[0], f[1]]).collect()
                    }
                }
            } else {
                return;
            };
            if samples.is_empty() {
                return;
            }
            // SCK stamps buffers on the host clock (uptime ns), same base as CGEventTimestamp.
            let capture_start_ms = sample
                .presentation_timestamp()
                .as_seconds()
                .filter(|s| *s > 0.0)
                .map(|s| clock::process_time_ms_from_event_timestamp((s * 1e9) as u64))
                .unwrap_or_else(|| clock::process_time_ms() - (samples.len() / 2) as f64 / 48.0);
            self.sink.push(samples, capture_start_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_and_bytes() {
        let l = vec![1.0, 2.0, 3.0];
        let r = vec![-1.0, -2.0, -3.0];
        assert_eq!(interleave(&[l, r]), vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        let bytes: Vec<u8> = [0.5f32, -0.25].iter().flat_map(|s| s.to_ne_bytes()).collect();
        assert_eq!(bytes_to_f32(&bytes), vec![0.5, -0.25]);
    }
}
