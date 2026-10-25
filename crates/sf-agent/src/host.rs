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
    /// What the screen video does over time (source ms): see [`crate::motion::sections`].
    fn screen_motion(&self) -> Result<Vec<(f64, f64, crate::motion::Motion)>, String>;
    /// JPEG stills of the screen video at source times (ms), one per time.
    fn screen_frames(&self, source_ms: &[f64]) -> Result<Vec<Vec<u8>>, String>;
    /// Read `text` aloud with AI voice `voice` (a [`VOICES`] id) at `speed`, saved in the
    /// project's voice-over folder with its words for captions.
    fn speak(&self, text: &str, voice: &str, speed: f64) -> Result<Spoken, String> {
        let _ = (text, voice, speed);
        Err("AI voices are not available here".into())
    }
}

/// A take read by an AI voice: its file in the voice-over folder and its length.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub file_name: String,
    pub duration_ms: f64,
}

/// The AI voices `add_narration` offers: (id, description). The first is the default.
pub const VOICES: [(&str, &str); 6] = [
    ("am_fenrir", "male, warm and clear (default)"),
    ("am_michael", "male, calm"),
    ("am_puck", "male, lively"),
    ("af_heart", "female, warm"),
    ("af_bella", "female, bright"),
    ("af_nicole", "female, soft"),
];

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
Padding: /config/output/paddingRatio01 0–0.4. Output aspect ratio: /config/output/aspectRatio = \"auto\" (the recording's own) or width / height as a number, never a \"w:h\" string: 16:9 1.7778, 9:16 0.5625, 1:1 1, 4:3 1.3333, 3:4 0.75, 4:5 0.8. Avoid empty area while zoomed: /config/output/avoidEmptyZoomArea.
Screen: /config/styles/screenBorderRadius 0–64 (rounded corners); screenInset {size 0.125–24 (0 = off), alpha 0–1, color}; shadow {intensity 0–0.6, distance 0–100, angle 0–180, blur 0–30, isDirectional}.
Cursor: /config/cursor: hide, size 16–128, set/id (options.cursorSets), alwaysUseDefault, hideNotMovingAfterMs (null = off, 500–5000), baseRotation −180–180, rotationMode velocity|directional, rotateOnXMovementRatio 0–2, stopMovementInLastPartMs 0–5000, removeShakeTreshold 100–2000, optimizeOriginalTypes. Click sound: /config/audio/clickSoundEffect, clickSoundEffectVolume 0–1.
Camera: /config/defaultLayout/type (screen-only | camera-overlay | split-screen | fullscreen-camera | cutout-camera); defaultLayout/cameraOverlay {overlayCameraPosition01 {x,y} 0–1, overlayCameraSizeRatio01 0.1–0.75, overlayZoomedCameraScale 0.25–1, overlayCameraAspectRatio}; camera aspect ratio (width / height: 16/9, 4/3, 1, 3/4 or 9/16) goes in both overlayCameraAspectRatio and /config/camera/aspectRatio; /config/camera: hide, mirror, enableFaceTracking, sfFaceTrackingZoom 0.5–2 (face tracking framing: below 1 a wider shot, above 1 closer; absent = 1), sharpen01 0–1, grain 0–1, lut (options.cameraLooks or null), lutIntensity 0–1; /config/styles/cameraBorderRadiusRatio01 0–0.5. Camera layout for a part of the video: add_layout.
Camera colour grade (DaVinci Resolve primaries, in Resolve's units): /config/camera/sfColorGrade, absent = no grade; add it as a whole object (every field), remove it to reset. Wheels lift, gamma, gain, offset are [r, g, b, master]: r, g, b a colour push summing to 0 (±0.5 at the rim), master lift −1–1 (unity 0), gamma −1–1 (0), gain 0–4 (1), offset −25–75 (25). temperature −100–100 (warmer), tint −100–100 (magenta), contrast 0–2 (1), pivot 0–1 (0.435), colorBoost −100–100 (vibrance), shadows −100–100, highlights −100–100, saturation 0–100 (unity 50), hue 0–100 (unity 50). Neutral: {lift [0,0,0,0], gamma [0,0,0,0], gain [0,0,0,1], offset [0,0,0,25], temperature 0, tint 0, contrast 1, pivot 0.435, colorBoost 0, shadows 0, highlights 0, saturation 50, hue 50}.
Captions: /config/captions: enableTranscript (show captions; needs a transcript, see transcribe), showSoundLabels, font sans-serif|serif|mono, sizeRatio 0.05–0.125, color + backgroundColor (\"#ffffff\" on \"#000000\" or the reverse), position01 {x 0.5, y 0 top | 0.5 | 1 bottom}, wordsReveal, lineGrowth, wordEntrance. Keyboard shortcut labels: enableShortcuts, showShortcutsWithSingleLetters, shortcutsSizeRatio 0.05–0.2.
Audio: /config/audio: muteMicrophone, volume (microphone) 0–2, microphoneAudioPreset voice|noise-reduction|podcast, microphoneBoostDb 0–24 (older projects: the whole microphone while the audio is linked), muteSystemAudio. A quiet clip or voice-over take: update_audio with boost_db (that item only; detach the audio first to boost part of the recording). Music: set_music and add_music (not by hand).
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
