//! Screen/audio/input recorder producing a Screen Studio compatible `recording/` directory.
//!
//! This crate plays the role of Screen Studio's `polyrecorder` helper: it captures a
//! display region with ScreenCaptureKit, the microphone and system audio, mouse/keyboard
//! events and cursor images, and writes them in exactly the layout the editor (and Screen
//! Studio itself) expects:
//!
//! ```text
//! <output_dir>/
//!   metadata.json / metadata-raw.json          sf_project::recording::RecordingMetadata
//!   polyrecorder.log
//!   channel-1-display-S.m3u8 + -0000.mp4 + -NNNN.m4s   H.264 HLS/fMP4, ~2 s segments
//!   channel-2-microphone-S.m4a                 AAC-LC 48 kHz mono (when a mic is selected)
//!   channel-N-systemAudio-S.m4a                AAC-LC (when system audio is enabled)
//!   channel-N-camera-S.m3u8 + segments         H.264 HLS/fMP4 webcam, 30 fps (when a camera is selected)
//!   cursors.json + cursors/<id>.png
//!   keystrokes-S.json, mouseclicks-S.json, mousemoves-S.json
//! ```
//!
//! `S` is the session index: 0, then one more for every pause/resume. All recorders share
//! the same session clock so their `sessions[i]` align by index.
//!
//! ```no_run
//! use sf_recording::{discover, CaptureTarget, Recorder, RecordingConfig};
//!
//! let d = discover();
//! let display = d.main_display().unwrap();
//! let cfg = RecordingConfig::new("/tmp/rec", CaptureTarget::Display { display_id: display.id });
//! let rec = Recorder::start(cfg).unwrap();
//! std::thread::sleep(std::time::Duration::from_secs(5));
//! let metadata = rec.finish().unwrap();
//! println!("recorded {} ms", metadata.duration_ms());
//! ```
//!
//! Everything compiles on other platforms but [`Recorder::start`] returns
//! [`Error::Unsupported`] there.

pub mod audio;
pub mod camera;
pub mod caret;
pub mod clock;
pub mod config;
pub mod cursor;
pub mod discover;
pub mod display;
pub mod ffmpeg;
pub mod input;
pub mod log;
pub mod metadata;
pub mod permissions;
pub mod window_frame;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod repeater;

use std::path::PathBuf;

pub use config::{CaptureTarget, RecordingConfig};
pub use discover::{discover, windows_with, AudioDevice, CameraInfo, Discovery, DisplayInfo, WindowInfo};
pub use permissions::{Permission, PermissionStatus};
pub use sf_project::recording::RecordingMetadata;

/// Errors from the recorder.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("recording is only supported on macOS")]
    Unsupported,
    /// `"screenRecording"`, `"inputMonitoring"`, `"microphone"` or `"camera"` (same names Screen Studio
    /// reports in its `missingPermission` message).
    #[error("missing permission: {0}")]
    PermissionDenied(&'static str),
    #[error("ffmpeg not found. Install it (brew install ffmpeg) or set SF_FFMPEG=/path/to/ffmpeg")]
    FfmpegNotFound,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("output directory already exists and is not empty: {0}")]
    OutputExists(PathBuf),
    #[error("capture: {0}")]
    Capture(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Lifecycle state of a [`Recorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderState {
    Recording,
    Paused,
    Finished,
    Cancelled,
    /// A recorder reported an error; whatever was written so far stays on disk.
    Failed(String),
}

/// A running recording. Create with [`Recorder::start`], end with [`Recorder::finish`] or
/// [`Recorder::cancel`]. Dropping a recorder without finishing stops all capture and leaves
/// the files as they are (metadata stays `"partial"`).
pub struct Recorder {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    inner: imp::Inner,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder").field("state", &self.state()).finish()
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Recorder {
    /// Create the output directory, start every recorder and begin session 0.
    pub fn start(cfg: RecordingConfig) -> Result<Recorder> {
        imp::Inner::start(cfg, None).map(|inner| Recorder { inner })
    }
    /// [`start`](Self::start) recording an already open camera (e.g. the one behind a live
    /// preview) when it is the configured `camera`, so the recording does not wait for the
    /// camera to wake up.
    pub fn start_with_camera(cfg: RecordingConfig, camera: std::sync::Arc<camera::CameraCapture>) -> Result<Recorder> {
        imp::Inner::start(cfg, Some(camera)).map(|inner| Recorder { inner })
    }
    /// End the current session (media files are finalized). No-op unless recording.
    pub fn pause(&mut self) -> Result<()> {
        self.inner.pause()
    }
    /// Begin the next session. No-op unless paused.
    pub fn resume(&mut self) -> Result<()> {
        self.inner.resume()
    }
    /// Remember the current elapsed time (Screen Studio's recording markers; see [`Recorder::markers`]).
    pub fn add_marker(&mut self) {
        self.inner.add_marker()
    }
    /// Markers added so far, in recorded milliseconds.
    pub fn markers(&self) -> Vec<f64> {
        self.inner.markers.clone()
    }
    /// Stop everything, write `metadata.json` with `state: "complete"` and return it.
    pub fn finish(self) -> Result<RecordingMetadata> {
        self.inner.finish()
    }
    /// Stop everything and delete the output directory.
    pub fn cancel(self) {
        self.inner.cancel()
    }
    /// Recorded (non-paused) time in milliseconds.
    pub fn elapsed_ms(&self) -> f64 {
        self.inner.elapsed_ms()
    }
    /// Live microphone level (0..1, dB scale) for a meter; `None` when no microphone is recorded.
    pub fn microphone_level(&self) -> Option<f32> {
        self.inner.mic.as_ref().map(|m| m.level())
    }
    pub fn state(&self) -> RecorderState {
        self.inner.state()
    }
    /// The output directory.
    pub fn output_dir(&self) -> &std::path::Path {
        &self.inner.cfg.output_dir
    }
    /// Geometry of the display capture (pixel size, recording scale, bounds).
    pub fn display_plan(&self) -> &display::DisplayPlan {
        &self.inner.plan
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Recorder {
    pub fn start(_cfg: RecordingConfig) -> Result<Recorder> {
        Err(Error::Unsupported)
    }
    pub fn pause(&mut self) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn resume(&mut self) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn add_marker(&mut self) {}
    pub fn markers(&self) -> Vec<f64> {
        Vec::new()
    }
    pub fn finish(self) -> Result<RecordingMetadata> {
        Err(Error::Unsupported)
    }
    pub fn cancel(self) {}
    pub fn elapsed_ms(&self) -> f64 {
        0.0
    }
    pub fn microphone_level(&self) -> Option<f32> {
        None
    }
    pub fn state(&self) -> RecorderState {
        RecorderState::Failed("unsupported platform".into())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod imp {
    use std::path::Path;
    use std::sync::Arc;

    use super::{Error, RecorderState, Result};
    use crate::audio::{MicRecorder, PcmSink, PcmSinkHandle};
    #[cfg(target_os = "macos")]
    use crate::audio::SystemAudioHandler;
    use crate::camera::{CameraCapture, CameraRecorder};
    use crate::config::RecordingConfig;
    use crate::cursor::CursorRecorder;
    use crate::display::{self, DisplayPlan, DisplayRecorder};
    use crate::input::InputRecorder;
    use crate::log::RecLog;
    use crate::metadata::{AudioChannel, CameraChannel, DisplayChannel, InputChannel, MetadataBuilder, SessionTimes};
    use crate::{clock, discover, permissions, RecordingMetadata};

    const COMPONENT: &str = "Recorder";

    struct Open {
        index: usize,
        process_start_ms: f64,
        unix_start_ms: f64,
    }

    pub(super) struct Inner {
        pub cfg: RecordingConfig,
        pub plan: DisplayPlan,
        log: RecLog,
        meta: MetadataBuilder,
        display: DisplayRecorder,
        input: Option<InputRecorder>,
        cursor: Option<CursorRecorder>,
        pub mic: Option<MicRecorder>,
        system_audio: Option<PcmSinkHandle>,
        camera: Option<CameraRecorder>,
        current: Option<Open>,
        state: RecorderState,
        pub markers: Vec<f64>,
        finished: bool,
    }

    fn prepare_dir(dir: &Path) -> Result<()> {
        if dir.exists() {
            let non_empty = std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true);
            if non_empty {
                return Err(Error::OutputExists(dir.to_path_buf()));
            }
        }
        std::fs::create_dir_all(dir).map_err(|source| Error::Io { path: dir.to_path_buf(), source })
    }

    impl Inner {
        pub fn start(cfg: RecordingConfig, camera_capture: Option<Arc<CameraCapture>>) -> Result<Self> {
            clock::init();
            if cfg.fps == 0 {
                return Err(Error::InvalidConfig("fps must be > 0".into()));
            }
            crate::ffmpeg::ffmpeg_path()?;
            if !permissions::screen_recording().is_granted() {
                return Err(Error::PermissionDenied("screenRecording"));
            }
            prepare_dir(&cfg.output_dir)?;
            let log = RecLog::open(&cfg.output_dir).map_err(|source| Error::Io { path: cfg.output_dir.join("polyrecorder.log"), source })?;
            log.notice(COMPONENT, format!("Recorder started. version={} target={:?}", cfg.recorder_version, cfg.target));

            // Run the rest in a closure so any failure removes the half-created directory.
            let dir = cfg.output_dir.clone();
            let r = Self::start_inner(cfg, camera_capture, log.clone());
            if let Err(e) = &r {
                log.error(COMPONENT, format!("Start failed: {e}"));
                let _ = std::fs::remove_dir_all(&dir);
            }
            r
        }

        fn start_inner(cfg: RecordingConfig, camera_capture: Option<Arc<CameraCapture>>, log: RecLog) -> Result<Self> {
            let discovery = discover::Discovery { displays: discover::displays(), windows: discover::windows(), microphones: Vec::new(), cameras: Vec::new() };
            // a window picked in the screen-sharing dialog: its stream size gives the plan
            #[cfg(target_os = "linux")]
            let picked = match cfg.target {
                crate::CaptureTarget::PortalWindow => Some(crate::portal::pick_window(cfg.fps.max(1), &log)?),
                _ => None,
            };
            #[cfg(target_os = "linux")]
            let plan = match &picked {
                Some(w) => display::window_stream_plan(w.size().0, w.size().1, w.scale(), &discovery)?,
                None => display::plan(&cfg, &discovery)?,
            };
            #[cfg(not(target_os = "linux"))]
            let plan = display::plan(&cfg, &discovery)?;
            log.notice(COMPONENT, format!("Display plan: {plan:?}"));

            // Input + cursor
            let (input, cursor) = if cfg.capture_input {
                let input = InputRecorder::start(log.clone(), cfg.capture_keystrokes)?;
                let cursor = CursorRecorder::start(&cfg.output_dir, log.clone(), input.cursor_change_sink())?;
                (Some(input), Some(cursor))
            } else {
                (None, None)
            };

            // Audio
            let mut audio_channels = Vec::new();
            let mut next_channel = 2usize;
            let mic = match &cfg.microphone {
                Some(sel) => {
                    if !permissions::microphone().is_granted() {
                        permissions::request_microphone();
                        if !permissions::microphone().is_granted() {
                            return Err(Error::PermissionDenied("microphone"));
                        }
                    }
                    let m = MicRecorder::start(sel, log.clone())?;
                    audio_channels.push(AudioChannel { kind: "microphone", channel_index: next_channel, device_id: Some(m.device_id.clone()) });
                    next_channel += 1;
                    Some(m)
                }
                None => None,
            };
            #[cfg(target_os = "macos")]
            let (system_audio, sys_handler) = if cfg.capture_system_audio {
                let h = PcmSink::spawn(48_000, 2, log.clone(), "SystemAudioRecorder")?;
                let handler = SystemAudioHandler { sink: h.sink() };
                audio_channels.push(AudioChannel { kind: "systemAudio", channel_index: next_channel, device_id: None });
                next_channel += 1;
                (Some(h), Some(handler))
            } else {
                (None, None)
            };
            // system audio comes from ScreenCaptureKit; there is no Linux equivalent yet
            #[cfg(target_os = "linux")]
            let (system_audio, sys_handler): (Option<PcmSinkHandle>, Option<()>) = {
                if cfg.capture_system_audio {
                    log.warning(COMPONENT, "System audio recording is not supported on Linux; ignoring.");
                }
                let _ = PcmSink::spawn;
                (None, None)
            };

            // Camera (opened before the display so its warm-up does not delay the first session)
            let camera = match &cfg.camera {
                Some(id) => {
                    if !permissions::camera().is_granted() {
                        permissions::request_camera();
                        if !permissions::camera().is_granted() {
                            return Err(Error::PermissionDenied("camera"));
                        }
                    }
                    // reuse the camera the app already opened for its preview, else open it now
                    let capture = match camera_capture.filter(|c| &c.device_id == id) {
                        Some(c) => c,
                        None => CameraCapture::open(id, log.clone())?,
                    };
                    Some(CameraRecorder::start(capture, next_channel, cfg.video_quality, log.clone())?)
                }
                None => None,
            };

            // Display
            #[cfg(target_os = "linux")]
            let display = match picked {
                Some(w) => DisplayRecorder::start_window(&cfg, plan.clone(), log.clone(), w)?,
                None => DisplayRecorder::start(&cfg, plan.clone(), log.clone(), sys_handler)?,
            };
            #[cfg(not(target_os = "linux"))]
            let display = DisplayRecorder::start(&cfg, plan.clone(), log.clone(), sys_handler)?;
            #[cfg(target_os = "linux")]
            if let (Some(i), true) = (&input, display.provides_pointer()) {
                display.set_pointer_sink(i.pointer_sink());
            }

            let meta = MetadataBuilder {
                version: cfg.recorder_version.clone(),
                input: input.as_ref().map(|_| InputChannel { capture_key_strokes: cfg.capture_keystrokes }),
                display: DisplayChannel {
                    display_id: plan.display_id,
                    crop_rect: plan.crop,
                    excluded_window_ids: cfg.exclude_window_ids.clone(),
                    exclude_finder_desktop_icons: cfg.hide_desktop_icons,
                    quality: cfg.video_quality,
                    max_key_frame_interval: 120,
                    bounds: plan.bounds,
                    display_refresh_rate: plan.refresh_rate,
                    recording_scale: plan.recording_scale,
                },
                audio: audio_channels,
                camera: camera.as_ref().map(|c| CameraChannel { channel_index: next_channel, device_id: c.device_id().to_string() }),
                sessions: Vec::new(),
            };
            let mut inner = Inner {
                cfg,
                plan,
                log,
                meta,
                display,
                input,
                cursor,
                mic,
                system_audio,
                camera,
                current: None,
                state: RecorderState::Paused,
                markers: Vec::new(),
                finished: false,
            };
            inner.meta.write(&inner.cfg.output_dir, "partial")?;
            inner.begin_session()?;
            Ok(inner)
        }

        fn begin_session(&mut self) -> Result<()> {
            let index = self.meta.sessions.len();
            let dir = self.cfg.output_dir.clone();
            if let Some(i) = &self.input {
                i.begin_session(index, &dir)?;
            }
            for a in &self.meta.audio {
                let path = dir.join(a.output_filename(index));
                match a.kind {
                    "microphone" => {
                        if let Some(m) = &self.mic {
                            m.begin_session(&path)?;
                        }
                    }
                    _ => {
                        if let Some(s) = &self.system_audio {
                            s.sink().begin_session(&path)?;
                        }
                    }
                }
            }
            let (process_start_ms, unix_start_ms) = clock::now_pair();
            if let Some(c) = &self.camera {
                c.begin_session(index, &dir)?;
            }
            self.display.begin_session(index, &dir)?;
            self.current = Some(Open { index, process_start_ms, unix_start_ms });
            self.state = RecorderState::Recording;
            self.log.notice(COMPONENT, format!("Session {index} started. processTimeStartMs={process_start_ms} unixStartMs={unix_start_ms}"));
            Ok(())
        }

        fn end_session(&mut self) -> Result<()> {
            let Some(open) = self.current.take() else { return Ok(()) };
            let (process_end_ms, unix_end_ms) = clock::now_pair();
            let mut first_err = None;
            let mut note = |r: Result<()>, what: &str, log: &RecLog| {
                if let Err(e) = r {
                    log.error(COMPONENT, format!("{what}: {e}"));
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            };
            // Audio first: its writers append whatever arrives until told to stop, so closing
            // them right after the end time keeps the .m4a durations equal to the session.
            if let Some(m) = &self.mic {
                note(m.end_session(process_end_ms), "microphone", &self.log);
            }
            if let Some(s) = &self.system_audio {
                note(s.sink().end_session(process_end_ms), "systemAudio", &self.log);
            }
            if let Some(i) = &self.input {
                note(i.end_session(), "input", &self.log);
            }
            if let Some(c) = &self.camera {
                match c.end_session() {
                    Ok(frames) => self.log.notice(COMPONENT, format!("Session {} camera finished. frames={frames} received={}", open.index, c.frames_received())),
                    Err(e) => note(Err(e), "camera", &self.log),
                }
            }
            match self.display.end_session() {
                Ok(frames) => self.log.notice(COMPONENT, format!("Session {} video finished. frames={frames}", open.index)),
                Err(e) => note(Err(e), "video", &self.log),
            }
            self.meta.sessions.push(SessionTimes {
                index: open.index,
                process_start_ms: open.process_start_ms,
                process_end_ms,
                unix_start_ms: open.unix_start_ms,
                unix_end_ms,
            });
            self.state = RecorderState::Paused;
            self.log.notice(COMPONENT, format!("Session {} ended. durationMs={}", open.index, process_end_ms - open.process_start_ms));
            if let Err(e) = self.meta.write(&self.cfg.output_dir, "partial") {
                self.log.warning(COMPONENT, format!("Could not update metadata: {e}"));
            }
            match first_err {
                Some(e) => {
                    self.state = RecorderState::Failed(e.to_string());
                    Err(e)
                }
                None => Ok(()),
            }
        }

        pub fn pause(&mut self) -> Result<()> {
            if self.state != RecorderState::Recording {
                return Ok(());
            }
            self.end_session()
        }

        pub fn resume(&mut self) -> Result<()> {
            if self.state != RecorderState::Paused {
                return Ok(());
            }
            self.begin_session()
        }

        pub fn add_marker(&mut self) {
            let t = self.elapsed_ms();
            self.markers.push(t);
            self.log.notice(COMPONENT, format!("Marker added at {t} ms"));
        }

        pub fn elapsed_ms(&self) -> f64 {
            let done: f64 = self.meta.sessions.iter().map(|s| s.duration_ms()).sum();
            let cur = self.current.as_ref().map(|o| clock::process_time_ms() - o.process_start_ms).unwrap_or(0.0);
            done + cur
        }

        pub fn state(&self) -> RecorderState {
            if let Some(e) = self.display.error() {
                return RecorderState::Failed(e);
            }
            self.state.clone()
        }

        fn stop_all(&mut self) {
            self.display.stop();
            if let Some(c) = self.cursor.as_mut() {
                c.stop();
            }
            if let Some(i) = self.input.as_mut() {
                i.stop();
            }
            if let Some(m) = self.mic.as_mut() {
                m.stop();
            }
            if let Some(s) = self.system_audio.as_mut() {
                s.stop();
            }
            if let Some(c) = self.camera.as_mut() {
                c.stop();
            }
        }

        pub fn finish(mut self) -> Result<RecordingMetadata> {
            let end = if self.state == RecorderState::Recording { self.end_session() } else { Ok(()) };
            self.stop_all();
            self.finished = true;
            let stream_err = self.display.error();
            let meta = self.meta.write(&self.cfg.output_dir, "complete")?;
            self.log.notice(COMPONENT, format!("Recording complete. sessions={} durationMs={}", meta.sessions.len(), meta.duration_ms()));
            end?;
            if let Some(e) = stream_err {
                return Err(Error::Capture(e));
            }
            self.state = RecorderState::Finished;
            Ok(meta)
        }

        pub fn cancel(mut self) {
            let _ = self.end_session();
            self.stop_all();
            self.finished = true;
            self.log.notice(COMPONENT, "Recording cancelled; removing output directory.");
            let _ = std::fs::remove_dir_all(&self.cfg.output_dir);
            self.state = RecorderState::Cancelled;
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            if !self.finished {
                let _ = self.end_session();
                self.stop_all();
            }
        }
    }
}
