//! ScreenForge project model.
//!
//! A `.screenforge` project is a macOS package directory. It shares its layout with
//! `.screenstudio` packages, which are imported by copying (see [`package::import_screen_studio`]):
//!
//! ```text
//! Name.screenforge/
//!   meta.json                 superjson {version, requiredVersion, createdAt}
//!   project.json              superjson Project (v4 nested config, v3 flat config)
//!   project.json.bak          previous save
//!   recording-markers.json    superjson [] markers set while recording
//!   data/<key>                superjson per-key scratch values (playback position, flags)
//!   QuickLook/Preview.png     Finder thumbnail
//!   transcripts/              superjson caption transcripts per microphone session
//!   recording/                polyrecorder output (metadata.json, HLS fMP4 segments,
//!                             cursors/, keystrokes/mouseclicks/mousemoves JSON)
//! ```
//!
//! All JSON documents are wrapped in a [superjson](https://github.com/blitz-js/superjson)
//! envelope `{ "json": ..., "meta": { "values": {...}, "v": 1 } }`; see [`superjson`].
//! Unknown fields are preserved on every struct (`extra`) so that files written by a newer
//! Screen Studio survive a load/save round-trip through this crate.

pub mod audio_clips;
pub mod ids;
pub mod music_clips;
pub mod package;
pub mod preset;
pub mod recording;
pub mod schema;
pub mod superjson;
pub mod transcript;

pub use package::{ImportProgress, ProjectPackage, PACKAGE_EXTENSION, PRESET_EXTENSION, SCREEN_STUDIO_EXTENSION, SCREEN_STUDIO_PRESET_EXTENSION};
pub use preset::Preset;
pub use audio_clips::{AudioClip, AudioTrack, SceneAudioExt};
pub use music_clips::{MusicClip, SceneMusicExt};
pub use schema::v4::{self as v4, Project};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON error in {path}: {source}")]
    Json {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("not a project package: {0}")]
    NotAPackage(std::path::PathBuf),
    #[error("unsupported project version {0}")]
    UnsupportedVersion(String),
    #[error("superjson: {0}")]
    SuperJson(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
