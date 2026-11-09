//! ScreenForge extension: chapters, for the chapter list of a YouTube description.
//!
//! `sfChapters` in the scene's unknown-field map (Screen Studio ignores it, and would drop it
//! on its own save) lists where each chapter starts. A chapter is pinned to a moment of the
//! recording, so it follows cuts and speed changes like a zoom; a chapter that starts on a
//! title card names that card instead, since a card has no recording time of its own. See
//! `sf_engine::chapters` for their place on the edited timeline.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::new_id;
use crate::schema::common::Extra;
use crate::v4::Scene;

pub const KEY: &str = "sfChapters";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub id: String,
    pub title: String,
    /// The moment of the recording the chapter starts at.
    #[serde(default)]
    pub source_ms: f64,
    /// Id of the title card (a slice) the chapter starts at, instead of `source_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Chapter {
    pub fn new(title: impl Into<String>, source_ms: f64, card: Option<String>) -> Self {
        Chapter { id: new_id(), title: title.into(), source_ms, card, extra: Extra::new() }
    }
}

pub trait SceneChaptersExt {
    fn chapters(&self) -> Vec<Chapter>;
    /// Store `chapters`; an empty list removes the key.
    fn set_chapters(&mut self, chapters: Vec<Chapter>);
}

impl SceneChaptersExt for Scene {
    fn chapters(&self) -> Vec<Chapter> {
        self.extra.get(KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    fn set_chapters(&mut self, chapters: Vec<Chapter>) {
        if chapters.is_empty() {
            self.extra.remove(KEY);
        } else {
            self.extra.insert(KEY.into(), serde_json::to_value(chapters).unwrap_or(Value::Null));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_round_trip_through_the_scene_and_keep_unknown_fields() {
        let mut scene = crate::Project::new("T").scenes.remove(0);
        assert!(scene.chapters().is_empty());
        let mut c = Chapter::new("Intro", 0.0, None);
        c.extra.insert("future".into(), Value::from(1));
        scene.set_chapters(vec![c.clone(), Chapter::new("Setup", 12_000.0, Some("card".into()))]);
        let back: Scene = serde_json::from_value(serde_json::to_value(&scene).unwrap()).unwrap();
        let list = back.chapters();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], c);
        assert_eq!(list[1].card.as_deref(), Some("card"));
        let mut empty = back.clone();
        empty.set_chapters(vec![]);
        assert!(!empty.extra.contains_key(KEY));
    }
}
