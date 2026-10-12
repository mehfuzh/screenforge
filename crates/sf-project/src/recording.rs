//! The `recording/` directory written by Screen Studio's `polyrecorder` helper.
//!
//! ```text
//! recording/
//!   metadata.json                 RecordingMetadata (below); metadata-raw.json is the
//!                                 untouched polyrecorder output
//!   polyrecorder.log
//!   channel-1-display-0.m3u8      HLS VOD playlist (EXT-X-MAP init segment + fMP4 .m4s)
//!   channel-1-display-0-0000.mp4  init segment, channel-1-display-0-NNNN.m4s media segments
//!   channel-2-microphone-0.m3u8 / .m4s / .m4a
//!   channel-N-camera-0.*          webcam (when enabled)
//!   cursors.json + cursors/*.png  cursor sprites captured during the session
//!   keystrokes-0.json, mouseclicks-0.json, mousemoves-0.json
//! ```
//!
//! Each channel may have several sessions (`-0`, `-1`, …) when a recording was paused/resumed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::schema::common::{Extra, Point, Rect, Size};
use crate::{Error, Result};

pub const METADATA_FILE: &str = "metadata.json";
pub const METADATA_RAW_FILE: &str = "metadata-raw.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMetadata {
    pub log_filename: Option<String>,
    pub polyrecorder_version: Option<String>,
    pub recorders: Vec<Recorder>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// `"complete"` | `"recording"` | `"failed"`
    pub state: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl RecordingMetadata {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(METADATA_FILE);
        let s = std::fs::read_to_string(&path).map_err(|source| Error::Io { path: path.clone(), source })?;
        serde_json::from_str(&s).map_err(|source| Error::Json { path, source })
    }

    pub fn recorders_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Recorder> + 'a {
        self.recorders.iter().filter(move |r| r.kind == kind)
    }

    pub fn display(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::DISPLAY).next().or_else(|| self.recorders_of(RecorderKind::WINDOW).next())
    }
    pub fn microphone(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::MICROPHONE).next()
    }
    pub fn camera(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::CAMERA).next()
    }
    pub fn system_audio(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::SYSTEM_AUDIO).next()
    }
    pub fn input(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::INPUT).next()
    }
    pub fn cursor(&self) -> Option<&Recorder> {
        self.recorders_of(RecorderKind::CURSOR).next()
    }

    /// Total recorded duration across sessions, ms.
    pub fn duration_ms(&self) -> f64 {
        self.sessions.iter().map(|s| s.duration_ms).sum()
    }
}

pub struct RecorderKind;
impl RecorderKind {
    pub const CURSOR: &'static str = "cursor";
    pub const INPUT: &'static str = "input";
    pub const DISPLAY: &'static str = "display";
    pub const WINDOW: &'static str = "window";
    pub const MICROPHONE: &'static str = "microphone";
    pub const SYSTEM_AUDIO: &'static str = "systemAudio";
    pub const CAMERA: &'static str = "camera";
    pub const EXTERNAL_DEVICE: &'static str = "externalDevice";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorder {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub configuration: Map<String, Value>,
    #[serde(default)]
    pub sessions: Vec<RecorderSession>,
    // cursor recorder only
    pub cursor_images_folder: Option<String>,
    pub cursors_info_file: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Recorder {
    pub fn crop_rect(&self) -> Option<Rect> {
        serde_json::from_value(self.configuration.get("cropRect")?.clone()).ok()
    }
    pub fn display_id(&self) -> Option<u64> {
        self.configuration.get("displayId")?.as_u64()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub duration_ms: f64,
    pub process_time_start_ms: f64,
    pub process_time_end_ms: f64,
    pub unix_start_ms: f64,
    pub unix_end_ms: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderSession {
    pub duration_ms: f64,
    pub process_time_start_ms: f64,
    pub process_time_end_ms: f64,
    pub unix_start_ms: f64,
    pub unix_end_ms: f64,
    /// `channel-1-display-0.m3u8` (or `.mp4` for older recordings) / `.m4a` for audio
    pub output_filename: Option<String>,
    /// Recorded screen region in points, display-relative (top-left origin).
    pub bounds: Option<Rect>,
    pub display_refresh_rate: Option<f64>,
    /// Video pixels = points × display scale × recordingScale.
    pub recording_scale: Option<f64>,
    // input recorder
    pub key_strokes_filename: Option<String>,
    pub mouse_clicks_filename: Option<String>,
    pub mouse_moves_filename: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl RecorderSession {
    /// Resolve the media file for this session inside `dir`, preferring the HLS playlist.
    pub fn media_path(&self, dir: &Path) -> Option<PathBuf> {
        let name = self.output_filename.as_deref()?;
        let direct = dir.join(name);
        if direct.exists() {
            return Some(direct);
        }
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        for ext in ["m3u8", "mp4", "m4a", "mov"] {
            let p = dir.join(format!("{stem}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

// ───────────────────────────── input events ─────────────────────────────

/// Mouse move / click / key event as logged by the input recorder. Coordinates are in
/// global screen points (display origin top-left, y down), not relative to the crop rect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputEvent {
    /// `mouseMoved` | `mouseDown` | `mouseUp` | `mouseDragged` | `scrollWheel` |
    /// `keyDown` | `keyUp` | `flagsChanged`
    #[serde(rename = "type")]
    pub kind: String,
    /// Milliseconds since process start; subtract the session's `processTimeStartMs`.
    pub process_time_ms: f64,
    pub unix_time_ms: f64,
    #[serde(default)]
    pub active_modifiers: Vec<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// `arrow`, `pointingHand`, `iBeam`, … (matches ids in `cursors.json`)
    pub cursor_id: Option<String>,
    /// `left` | `right` | `other`
    pub button: Option<String>,
    pub character: Option<String>,
    pub key_code: Option<u32>,
    pub is_a_repeat: Option<bool>,
    /// ScreenForge extension on key events: text caret position (global points) after the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret_y: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl InputEvent {
    pub fn position(&self) -> Option<Point> {
        Some(Point::new(self.x?, self.y?))
    }
}

pub fn load_events(path: &Path) -> Result<Vec<InputEvent>> {
    let s = std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.into(), source })?;
    serde_json::from_str(&s).map_err(|source| Error::Json { path: path.into(), source })
}

/// Entry of `cursors.json`: one captured cursor image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorInfo {
    pub id: String,
    pub hot_spot: Point,
    /// Logical size in points; the PNG may be 2× that.
    pub standard_size: Size,
    #[serde(default)]
    pub system_cursor: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

pub fn load_cursors(dir: &Path) -> Result<Vec<CursorInfo>> {
    let path = dir.join("cursors.json");
    let s = std::fs::read_to_string(&path).map_err(|source| Error::Io { path: path.clone(), source })?;
    serde_json::from_str(&s).map_err(|source| Error::Json { path, source })
}

/// All inputs of one session, loaded lazily by the engine.
#[derive(Debug, Default, Clone)]
pub struct InputLog {
    pub mouse_moves: Vec<InputEvent>,
    pub mouse_clicks: Vec<InputEvent>,
    pub key_strokes: Vec<InputEvent>,
}

impl InputLog {
    pub fn load(dir: &Path, session: &RecorderSession) -> Result<Self> {
        let load = |name: &Option<String>| -> Result<Vec<InputEvent>> {
            match name {
                Some(n) if dir.join(n).exists() => load_events(&dir.join(n)),
                _ => Ok(vec![]),
            }
        };
        Ok(InputLog {
            mouse_moves: load(&session.mouse_moves_filename)?,
            mouse_clicks: load(&session.mouse_clicks_filename)?,
            key_strokes: load(&session.key_strokes_filename)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recording")
    }

    #[test]
    fn parses_metadata() {
        let m = RecordingMetadata::load(&fixtures()).unwrap();
        assert_eq!(m.state.as_deref(), Some("complete"));
        let d = m.display().unwrap();
        assert_eq!(d.display_id(), Some(3));
        let crop = d.crop_rect().unwrap();
        assert_eq!((crop.width, crop.height), (1200.0, 900.0));
        let s = &d.sessions[0];
        assert_eq!(s.recording_scale, Some(0.5));
        assert!(s.media_path(&fixtures()).unwrap().ends_with("channel-1-display-0.m3u8"));
        assert!(m.duration_ms() > 163_000.0);
    }

    #[test]
    fn parses_events_and_cursors() {
        let clicks = load_events(&fixtures().join("mouseclicks-0.json")).unwrap();
        assert_eq!(clicks[0].kind, "mouseDown");
        assert_eq!(clicks[0].button.as_deref(), Some("left"));
        let keys = load_events(&fixtures().join("keystrokes-0.json")).unwrap();
        assert_eq!(keys[0].character.as_deref(), Some("w"));
        let cursors = load_cursors(&fixtures()).unwrap();
        assert!(cursors.iter().any(|c| c.id == "arrow"));
    }
}
