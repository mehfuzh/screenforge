//! Recording configuration.

use std::path::PathBuf;

use sf_project::schema::common::Rect;

/// What to capture. Coordinates are global screen points, origin top-left, y down
/// (the same space as `discover().displays[].bounds_points` and the input event logs).
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureTarget {
    /// A whole display (`CGDirectDisplayID`).
    Display { display_id: u32 },
    /// A rectangle of a display. `rect_points` is in global screen points; it is
    /// intersected with the display and made display-relative for ScreenCaptureKit.
    Area { display_id: u32, rect_points: Rect },
    /// A single window (`CGWindowID`). Like Screen Studio's "window" mode this records
    /// the display region under the window's bounds (so the result stays a `display`
    /// channel with a `cropRect`), not an isolated window stream.
    Window { window_id: u32 },
    /// Linux on Wayland: a window the user picks in the system's screen-sharing dialog when
    /// recording starts (Wayland lets no application list or locate other windows). Records
    /// that window's own stream; positions in the recording are relative to the window.
    PortalWindow,
    /// macOS: an iPhone or iPad connected by cable ([`crate::device::DeviceInfo::id`]). Its
    /// screen is the `display` channel, sized like the device's screen; there are no input logs
    /// (the device has no Mac pointer or keyboard).
    Device { id: String },
}

/// Everything the UI decides before pressing record.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingConfig {
    /// Directory to create. It must not exist yet (or must be empty); on `cancel` it is removed.
    pub output_dir: PathBuf,
    pub target: CaptureTarget,
    /// Microphone to record: an id from `discover().microphones` (CoreAudio UID), a device
    /// name, or `"default"`. `None` records no microphone.
    pub microphone: Option<String>,
    /// Record system audio through ScreenCaptureKit (`channel-N-systemAudio-S.m4a`).
    pub capture_system_audio: bool,
    /// Camera to record: an id from `discover().cameras`. `None` records no camera.
    pub camera: Option<String>,
    /// `CGWindowID`s to remove from the capture (the app's own windows).
    pub exclude_window_ids: Vec<u32>,
    /// Record keyboard events into `keystrokes-S.json`.
    pub capture_keystrokes: bool,
    /// Record mouse/keyboard events and cursor images at all. Requires Input Monitoring.
    pub capture_input: bool,
    /// Exclude Finder's desktop icon windows from the capture.
    pub hide_desktop_icons: bool,
    /// Constant frame rate fed to the encoder (Screen Studio: 60).
    pub fps: u32,
    /// Encoder quality hint, 0..=1 (Screen Studio: 0.85). Stored in metadata and used to
    /// scale the target bitrate.
    pub video_quality: f64,
    /// Version string written to `metadata.json#polyrecorderVersion`.
    pub recorder_version: String,
}

impl RecordingConfig {
    /// Defaults matching Screen Studio: 60 fps, keystrokes on, no audio, no camera.
    pub fn new(output_dir: impl Into<PathBuf>, target: CaptureTarget) -> Self {
        RecordingConfig {
            output_dir: output_dir.into(),
            target,
            microphone: None,
            capture_system_audio: false,
            camera: None,
            exclude_window_ids: Vec::new(),
            capture_keystrokes: true,
            capture_input: true,
            hide_desktop_icons: false,
            fps: 60,
            video_quality: 0.85,
            recorder_version: format!("screenforge-{}", env!("CARGO_PKG_VERSION")),
        }
    }
}
