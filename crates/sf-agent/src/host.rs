//! What the agent can do beyond the project file, provided by the editor: the values the
//! pickers offer (wallpapers, music, cursor sets …), presets, Unsplash photos, files the user
//! names, and actions only the app can run (transcribing, export).

use serde_json::{json, Value};
use sf_project::Project;

/// The editor's side of the agent. Called on the agent's thread.
pub trait Host: Send + Sync {
    /// Values the pickers offer, as JSON (`wallpapers`, `music`, `cursorSets`, `cameraLooks`,
    /// `presets`, `recordedAudio` …).
    fn options(&self) -> Value;
    /// Apply the saved preset `name` (its settings, and the files they use) to `project`.
    fn apply_preset(&self, name: &str, project: &mut Project) -> Result<(), String>;
    /// Search Unsplash: `[{ id, author, color, description? }]`.
    fn search_photos(&self, query: &str) -> Result<Value, String>;
    /// Download photo `id` (from a search) into the project: the value for
    /// `config.styles.background.unsplash`.
    fn download_photo(&self, id: &str) -> Result<Value, String>;
    /// Copy the file at `path` into the project's assets: its file name there.
    fn import_file(&self, path: &str) -> Result<String, String>;
}

/// Something only the editor can do, asked for by a tool and run when the round's edits are
/// applied.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Transcribe the recording (Captions → Generate transcript).
    Transcribe { language: Option<String> },
    /// Open the export dialog.
    OpenExport,
}

/// The sidebar's controls and where they live in the project, for `get_options`.
pub const SETTINGS: &str = "Settings in the project file (patch_project paths; ranges are what the sidebar allows; a field missing from get_config is set with op add, not replace):
Background (General): /config/styles/background/type = system (wallpaper, systemName from options.wallpapers) | gradient (gradient {start,end,stops[{color,at}]}, presets in options.gradients) | color (color \"#rrggbb\") | image (image = a file name in the project's assets, see import_file) | unsplash (use search_photos and use_photo). background/blur 0–100.
Padding: /config/output/paddingRatio01 0–0.4. Output aspect ratio: /config/output/aspectRatio (\"auto\", \"16:9\", \"9:16\", \"1:1\", \"4:3\", \"4:5\" …). Avoid empty area while zoomed: /config/output/avoidEmptyZoomArea.
Screen: /config/styles/screenBorderRadius 0–64 (rounded corners); screenInset {size 0.125–24 (0 = off), alpha 0–1, color}; shadow {intensity 0–0.6, distance 0–100, angle 0–180, blur 0–30, isDirectional}.
Cursor: /config/cursor: hide, size 16–128, set/id (options.cursorSets), alwaysUseDefault, hideNotMovingAfterMs (null = off, 500–5000), baseRotation −180–180, rotationMode velocity|directional, rotateOnXMovementRatio 0–2, stopMovementInLastPartMs 0–5000, removeShakeTreshold 100–2000, optimizeOriginalTypes. Click sound: /config/audio/clickSoundEffect, clickSoundEffectVolume 0–1.
Camera: /config/defaultLayout/type (screen-only | camera-overlay | split-screen | fullscreen-camera | cutout-camera); defaultLayout/cameraOverlay {overlayCameraPosition01 {x,y} 0–1, overlayCameraSizeRatio01 0.1–0.75, overlayZoomedCameraScale 0.25–1, overlayCameraAspectRatio}; /config/camera: hide, mirror, enableFaceTracking, sharpen01 0–1, grain 0–1, lut (options.cameraLooks or null), lutIntensity 0–1; /config/styles/cameraBorderRadiusRatio01 0–0.5. Camera layout for a part of the video: add_layout.
Captions: /config/captions: enableTranscript (show captions; needs a transcript, see transcribe), showSoundLabels, font sans-serif|serif|mono, sizeRatio 0.05–0.125, color + backgroundColor (\"#ffffff\" on \"#000000\" or the reverse), position01 {x 0.5, y 0 top | 0.5 | 1 bottom}, wordsReveal, lineGrowth, wordEntrance. Keyboard shortcut labels: enableShortcuts, showShortcutsWithSingleLetters, shortcutsSizeRatio 0.05–0.2.
Audio: /config/audio: muteMicrophone, volume (microphone) 0–2, microphoneAudioPreset voice|noise-reduction|podcast, microphoneBoostDb 0–24 (absent until set), muteSystemAudio. Music: set_music and add_music (not by hand).
Animations: /config/animations/screenMovementSpring {stiffness 5–600, damping 5–200, mass 0.1–15} (zoom camera), mouseMovementSpring (same, or null for raw cursor), motionBlurAmount 0–1; /config/zooms/startAnimationEarly, sfZoomOnTyping (zoom follows typing; absent = on).
Crop: /config/crop/rect01 {x, y, width, height} 0–1 of the recording.
Device frame (a phone or tablet around the recording; tall recordings stand upright, wide ones lie on their side): /config/device/frameKey = one of options.deviceFrames, or null for none (also set /config/device/enableMockup true).
Per clip (index from get_project): /scenes/0/slices/<i>/hideCursor, disableSmoothMouseMovement, volume.
Project name: /name.";

/// `get_options`: the host's catalogs plus what every project offers.
pub fn options(host: Option<&dyn Host>) -> Value {
    let mut v = host.map(|h| h.options()).unwrap_or_else(|| json!({}));
    if let Value::Object(m) = &mut v {
        m.insert("gradients".into(), json!(sf_project::schema::common::Gradient::PRESETS.iter().map(|(name, stops)| json!({ "name": name, "stops": stops.iter().map(|(c, at)| json!({ "color": c, "at": at })).collect::<Vec<_>>() })).collect::<Vec<_>>()));
        m.insert("settings".into(), Value::from(SETTINGS));
        let frames: Vec<String> = sf_engine::device::KINDS.iter().flat_map(|k| sf_engine::device::FINISHES.iter().map(move |f| sf_engine::device::Frame { kind: k.0, finish: f.0 }.key())).collect();
        m.insert("deviceFrames".into(), json!(frames));
    }
    v
}
