//! Caption transcripts: `<project>/transcripts/microphone-session-<n>.json`.
//!
//! Same files as Screen Studio: a superjson envelope
//! `{location, generatedAt, generator, transcript: [{index, startMs, endMs, text, type?}]}`.
//! Times are ms into the microphone session's audio file. The transcript text is not stored
//! in `project.json`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::common::Extra;
use crate::{superjson, Error, Result};

pub const TRANSCRIPTS_DIR: &str = "transcripts";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// `"microphone-session"` (or `"voice-over-file"`, which is not generated yet).
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_index: Option<u32>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `{type: "whisper", model, prompt?, language?}` | `{type: "system", locale}` | …
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Generator {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptItem {
    pub index: u32,
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
    /// `"sound-label"` for non-speech labels such as `[music]`.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl TranscriptItem {
    pub fn new(index: u32, start_ms: f64, end_ms: f64, text: impl Into<String>) -> Self {
        TranscriptItem { index, start_ms, end_ms, text: text.into(), kind: None, extra: Extra::new() }
    }

    pub fn is_sound_label(&self) -> bool {
        self.kind.as_deref() == Some("sound-label")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptFile {
    pub location: Location,
    pub generated_at: DateTime<Utc>,
    pub generator: Generator,
    pub transcript: Vec<TranscriptItem>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl TranscriptFile {
    pub fn microphone(session_index: u32, generator: Generator, transcript: Vec<TranscriptItem>) -> Self {
        TranscriptFile {
            location: Location { kind: "microphone-session".into(), session_index: Some(session_index), extra: Extra::new() },
            generated_at: Utc::now(),
            generator,
            transcript,
            extra: Extra::new(),
        }
    }

    pub fn microphone_path(project_dir: &Path, session_index: u32) -> PathBuf {
        project_dir.join(TRANSCRIPTS_DIR).join(format!("microphone-session-{session_index}.json"))
    }

    /// The transcript of a microphone session, if one was generated.
    pub fn load_microphone(project_dir: &Path, session_index: u32) -> Result<Option<Self>> {
        let path = Self::microphone_path(project_dir, session_index);
        if !path.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&path).map_err(|source| Error::Io { path: path.clone(), source })?;
        superjson::from_str(&s).map(Some).map_err(|source| Error::Json { path, source })
    }

    pub fn save(&self, project_dir: &Path) -> Result<PathBuf> {
        let index = self.location.session_index.unwrap_or(0);
        let path = Self::microphone_path(project_dir, index);
        let dir = project_dir.join(TRANSCRIPTS_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        let s = superjson::to_string_pretty(self, &["generatedAt"])?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, s).map_err(|source| Error::Io { path: tmp.clone(), source })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::Io { path: path.clone(), source })?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_screen_studio_shape() {
        let dir = tempfile::tempdir().unwrap();
        let gen = Generator { kind: "whisper".into(), model: Some("base".into()), language: None, prompt: None, extra: Extra::new() };
        let mut label = TranscriptItem::new(1, 900.0, 1400.0, "[music]");
        label.kind = Some("sound-label".into());
        let file = TranscriptFile::microphone(0, gen, vec![TranscriptItem::new(0, 120.0, 480.0, "Hello"), label]);
        let path = file.save(dir.path()).unwrap();
        assert!(path.ends_with("transcripts/microphone-session-0.json"));

        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["meta"]["values"]["generatedAt"][0], "Date");
        assert_eq!(raw["json"]["location"], serde_json::json!({"type": "microphone-session", "sessionIndex": 0}));
        assert_eq!(raw["json"]["transcript"][0], serde_json::json!({"index": 0, "startMs": 120, "endMs": 480, "text": "Hello"}));
        assert_eq!(raw["json"]["transcript"][1]["type"], "sound-label");

        let back = TranscriptFile::load_microphone(dir.path(), 0).unwrap().unwrap();
        assert_eq!(back.transcript, file.transcript);
        assert!(TranscriptFile::load_microphone(dir.path(), 1).unwrap().is_none());
    }
}
