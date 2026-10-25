//! `metadata.json` in the exact shape polyrecorder writes it (see
//! `sf_project::recording::RecordingMetadata` for the parsed form).
//!
//! The file is written with sorted keys and 2-space indentation like Swift's
//! `JSONEncoder` with `.sortedKeys/.prettyPrinted`; `null`s are omitted.

use std::path::Path;

use serde_json::{json, Map, Value};
use sf_project::recording::{RecordingMetadata, METADATA_FILE, METADATA_RAW_FILE};
use sf_project::schema::common::Rect;

use crate::log::LOG_FILENAME;
use crate::{Error, Result};

/// Start/end of one record segment (between resume and pause). All recorders of a
/// recording share the same session times so their `sessions[i]` align by index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionTimes {
    pub index: usize,
    pub process_start_ms: f64,
    pub process_end_ms: f64,
    pub unix_start_ms: f64,
    pub unix_end_ms: f64,
}

impl SessionTimes {
    pub fn duration_ms(&self) -> f64 {
        self.process_end_ms - self.process_start_ms
    }

    fn base_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("durationMs".into(), json!(self.duration_ms()));
        m.insert("processTimeStartMs".into(), json!(self.process_start_ms));
        m.insert("processTimeEndMs".into(), json!(self.process_end_ms));
        m.insert("unixStartMs".into(), json!(self.unix_start_ms));
        m.insert("unixEndMs".into(), json!(self.unix_end_ms));
        m
    }
}

/// Static description of the display channel.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayChannel {
    pub display_id: u32,
    /// Display-relative crop in points (`None` = whole display).
    pub crop_rect: Option<Rect>,
    pub excluded_window_ids: Vec<u32>,
    pub exclude_finder_desktop_icons: bool,
    pub quality: f64,
    pub max_key_frame_interval: u32,
    /// Recorded rect in global screen points (what `sessions[].bounds` carries).
    pub bounds: Rect,
    pub display_refresh_rate: f64,
    /// Points per output pixel.
    pub recording_scale: f64,
    /// Window recordings follow the window: `[processTimeMs, x, y]`, the recorded rect's new
    /// top-left in global points from that time on (`bounds` until the first one).
    pub bounds_moves: Vec<[f64; 3]>,
}

/// Static description of an audio channel (microphone or system audio).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChannel {
    /// `"microphone"` | `"systemAudio"`
    pub kind: &'static str,
    pub channel_index: usize,
    pub device_id: Option<String>,
}

impl AudioChannel {
    pub fn id(&self) -> String {
        format!("channel-{}-{}", self.channel_index, self.kind)
    }
    pub fn output_filename(&self, session: usize) -> String {
        format!("{}-{session}.m4a", self.id())
    }
}

/// Static description of the webcam channel (`channel-N-camera`).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraChannel {
    pub channel_index: usize,
    pub device_id: String,
}

impl CameraChannel {
    pub fn id(&self) -> String {
        format!("channel-{}-camera", self.channel_index)
    }
    pub fn output_filename(&self, session: usize) -> String {
        format!("{}-{session}.m3u8", self.id())
    }
}

/// Collects everything that ends up in `metadata.json`.
#[derive(Debug, Clone)]
pub struct MetadataBuilder {
    pub version: String,
    pub input: Option<InputChannel>,
    pub display: DisplayChannel,
    pub audio: Vec<AudioChannel>,
    pub camera: Option<CameraChannel>,
    pub sessions: Vec<SessionTimes>,
}

/// The `input` channel (keyboard/mouse) plus the implicit `cursor` recorder.
#[derive(Debug, Clone, PartialEq)]
pub struct InputChannel {
    pub capture_key_strokes: bool,
}

/// File names of the input logs for a session index.
pub fn input_filenames(session: usize) -> (String, String, String) {
    (
        format!("keystrokes-{session}.json"),
        format!("mouseclicks-{session}.json"),
        format!("mousemoves-{session}.json"),
    )
}

/// `channel-1-display-S.m3u8`
pub fn display_playlist_name(session: usize) -> String {
    format!("channel-1-display-{session}.m3u8")
}

fn rect_json(r: &Rect) -> Value {
    json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height})
}

impl MetadataBuilder {
    /// Build the JSON document. `state` is `"partial"` while recording and `"complete"` after
    /// a clean stop.
    pub fn build(&self, state: &str) -> Value {
        let mut recorders = Vec::new();
        if let Some(input) = &self.input {
            recorders.push(json!({
                "configuration": {},
                "cursorImagesFolder": "cursors",
                "cursorsInfoFile": "cursors.json",
                "id": "channel-0-cursor",
                "type": "cursor",
            }));
            let sessions: Vec<Value> = self
                .sessions
                .iter()
                .map(|s| {
                    let (k, c, m) = input_filenames(s.index);
                    let mut j = s.base_json();
                    j.insert("keyStrokesFilename".into(), json!(k));
                    j.insert("mouseClicksFilename".into(), json!(c));
                    j.insert("mouseMovesFilename".into(), json!(m));
                    Value::Object(j)
                })
                .collect();
            recorders.push(json!({
                "configuration": {"captureKeyStrokes": input.capture_key_strokes},
                "id": "channel-0-input",
                "sessions": sessions,
                "type": "input",
            }));
        }

        let d = &self.display;
        let mut cfg = Map::new();
        if let Some(crop) = &d.crop_rect {
            let mut c = rect_json(crop);
            c["yAxis"] = json!("topBasedIncreasingDownwards");
            cfg.insert("cropRect".into(), c);
        }
        cfg.insert("displayId".into(), json!(d.display_id));
        cfg.insert("excludeFinderDesktopIcons".into(), json!(d.exclude_finder_desktop_icons));
        cfg.insert("excludedWindowIds".into(), json!(d.excluded_window_ids));
        if !d.bounds_moves.is_empty() {
            cfg.insert("sfBoundsMoves".into(), json!(d.bounds_moves));
        }
        cfg.insert(
            "videoCompressionSettings".into(),
            json!({"maxKeyFrameInterval": d.max_key_frame_interval, "quality": d.quality}),
        );
        let sessions: Vec<Value> = self
            .sessions
            .iter()
            .map(|s| {
                let mut j = s.base_json();
                j.insert("bounds".into(), rect_json(&d.bounds));
                j.insert("displayRefreshRate".into(), json!(d.display_refresh_rate));
                j.insert("outputFilename".into(), json!(display_playlist_name(s.index)));
                j.insert("recordingScale".into(), json!(d.recording_scale));
                Value::Object(j)
            })
            .collect();
        recorders.push(json!({
            "configuration": Value::Object(cfg),
            "id": "channel-1-display",
            "sessions": sessions,
            "type": "display",
        }));

        for a in &self.audio {
            let mut cfg = Map::new();
            if let Some(id) = &a.device_id {
                cfg.insert("deviceId".into(), json!(id));
            }
            let sessions: Vec<Value> = self
                .sessions
                .iter()
                .map(|s| {
                    let mut j = s.base_json();
                    j.insert("outputFilename".into(), json!(a.output_filename(s.index)));
                    Value::Object(j)
                })
                .collect();
            recorders.push(json!({
                "configuration": Value::Object(cfg),
                "id": a.id(),
                "sessions": sessions,
                "type": a.kind,
            }));
        }

        if let Some(c) = &self.camera {
            let sessions: Vec<Value> = self
                .sessions
                .iter()
                .map(|s| {
                    let mut j = s.base_json();
                    j.insert("outputFilename".into(), json!(c.output_filename(s.index)));
                    Value::Object(j)
                })
                .collect();
            recorders.push(json!({
                "configuration": {"deviceId": c.device_id},
                "id": c.id(),
                "sessions": sessions,
                "type": "camera",
            }));
        }

        let sessions: Vec<Value> = self.sessions.iter().map(|s| Value::Object(s.base_json())).collect();
        let doc = json!({
            "logFilename": LOG_FILENAME,
            "polyrecorderVersion": self.version,
            "recorders": recorders,
            "sessions": sessions,
            "state": state,
        });
        sort_keys(doc)
    }

    /// Write `metadata.json` (and an identical `metadata-raw.json`, which Screen Studio keeps
    /// as the untouched recorder output) into `dir` and return the parsed form.
    pub fn write(&self, dir: &Path, state: &str) -> Result<RecordingMetadata> {
        let doc = self.build(state);
        let text = serde_json::to_string_pretty(&doc).map_err(Error::Json)?;
        for name in [METADATA_FILE, METADATA_RAW_FILE] {
            let path = dir.join(name);
            std::fs::write(&path, &text).map_err(|source| Error::Io { path, source })?;
        }
        serde_json::from_value(doc).map_err(Error::Json)
    }
}

/// Recursively sort object keys (Swift `JSONEncoder.sortedKeys` order = byte order) and
/// print integral doubles as integers (`1200`, not `1200.0`), like Swift does.
pub fn sort_keys(v: Value) -> Value {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
            _ => Value::Number(n),
        },
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().map(|(k, v)| (k, sort_keys(v))).collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder() -> MetadataBuilder {
        MetadataBuilder {
            version: "screenforge-test".into(),
            input: Some(InputChannel { capture_key_strokes: true }),
            display: DisplayChannel {
                display_id: 3,
                crop_rect: Some(Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 }),
                excluded_window_ids: vec![1, 2],
                exclude_finder_desktop_icons: false,
                quality: 0.85,
                max_key_frame_interval: 120,
                bounds: Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 },
                display_refresh_rate: 60.0,
                recording_scale: 0.5,
                bounds_moves: vec![],
            },
            audio: vec![AudioChannel { kind: "microphone", channel_index: 2, device_id: Some("dev".into()) }],
            camera: Some(CameraChannel { channel_index: 3, device_id: "FaceTime HD Camera".into() }),
            sessions: vec![SessionTimes {
                index: 0,
                process_start_ms: 474.4,
                process_end_ms: 1474.4,
                unix_start_ms: 1.0e12,
                unix_end_ms: 1.0e12 + 1000.0,
            }],
        }
    }

    #[test]
    fn round_trips_through_sf_project() {
        let dir = tempfile::tempdir().unwrap();
        let meta = builder().write(dir.path(), "complete").unwrap();
        let loaded = RecordingMetadata::load(dir.path()).unwrap();
        assert_eq!(meta, loaded);
        assert_eq!(loaded.state.as_deref(), Some("complete"));
        let d = loaded.display().unwrap();
        assert_eq!(d.display_id(), Some(3));
        assert_eq!(d.crop_rect().unwrap().width, 1200.0);
        assert_eq!(d.sessions[0].output_filename.as_deref(), Some("channel-1-display-0.m3u8"));
        assert_eq!(d.sessions[0].recording_scale, Some(0.5));
        let i = loaded.input().unwrap();
        assert_eq!(i.sessions[0].mouse_moves_filename.as_deref(), Some("mousemoves-0.json"));
        assert!(loaded.cursor().is_some());
        assert_eq!(loaded.microphone().unwrap().sessions[0].output_filename.as_deref(), Some("channel-2-microphone-0.m4a"));
        let cam = loaded.camera().unwrap();
        assert_eq!(cam.id, "channel-3-camera");
        assert_eq!(cam.sessions[0].output_filename.as_deref(), Some("channel-3-camera-0.m3u8"));
        assert!((loaded.duration_ms() - 1000.0).abs() < 1e-9);
        assert!(d.bounds_moves().is_empty(), "a recording that never moved has no moves");
    }

    #[test]
    fn window_moves_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = builder();
        b.display.bounds_moves = vec![[900.0, 600.0, 300.0], [700.0, 540.0, 260.0]];
        b.write(dir.path(), "complete").unwrap();
        let loaded = RecordingMetadata::load(dir.path()).unwrap();
        // read back in time order
        assert_eq!(loaded.display().unwrap().bounds_moves(), vec![[700.0, 540.0, 260.0], [900.0, 600.0, 300.0]]);
    }

    #[test]
    fn keys_are_sorted_and_no_nulls() {
        let text = serde_json::to_string_pretty(&builder().build("partial")).unwrap();
        assert!(!text.contains("null"));
        let first = text.find("\"logFilename\"").unwrap();
        let second = text.find("\"polyrecorderVersion\"").unwrap();
        let third = text.find("\"recorders\"").unwrap();
        assert!(first < second && second < third);
        assert!(text.contains("\"yAxis\": \"topBasedIncreasingDownwards\""));
    }
}
