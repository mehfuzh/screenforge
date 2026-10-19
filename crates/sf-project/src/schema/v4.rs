//! Screen Studio 4.x project schema (`project.json`, `version: "4.0.0"`).
//!
//! Field names mirror the on-disk JSON exactly (camelCase via serde). Every struct keeps an
//! `extra` map of unknown fields so that documents written by a newer Screen Studio survive a
//! round trip. Defaults match a freshly created Screen Studio 4.0.4 project.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::common::*;
use crate::ids::new_id;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: String,
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `null` on a project that was never saved.
    pub last_saved_at: Option<DateTime<Utc>>,
    pub config: Config,
    pub scenes: Vec<Scene>,
    /// Scratch values (`recordingFlags`, `last-playback-position-source-time-ms`, …).
    /// Also mirrored as individual files under `data/` in the package.
    #[serde(default)]
    pub data: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Project {
    /// A new, empty project with Screen Studio 4.0.4 defaults.
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Project {
            version: super::CURRENT_VERSION.into(),
            id: new_id(),
            name: name.into(),
            created_at: now,
            updated_at: now,
            last_saved_at: None,
            config: Config::default(),
            scenes: vec![Scene::recording("Default", 0)],
            data: {
                let mut m = Map::new();
                m.insert("recordingFlags".into(), Value::Array(vec![]));
                m
            },
            extra: Extra::new(),
        }
    }

    /// Dot paths of all `Date` values, for the superjson envelope.
    pub const DATE_PATHS: &'static [&'static str] = &["createdAt", "updatedAt", "lastSavedAt"];
}

// ───────────────────────────── config ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub crop: CropConfig,
    pub cursor: CursorConfig,
    pub captions: CaptionsConfig,
    pub audio: AudioConfig,
    pub camera: CameraConfig,
    pub animations: AnimationsConfig,
    pub processing: ProcessingConfig,
    pub zooms: ZoomsConfig,
    pub default_layout: LayoutConfig,
    pub device: DeviceConfig,
    pub output: OutputConfig,
    pub styles: StylesConfig,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Which part of the recorded screen is shown (normalized to the recording bounds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CropConfig {
    pub rect01: Rect,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorConfig {
    pub hide: bool,
    /// Cursor size in output points (v3 stored a multiplier; 48 ≈ 1.5×).
    pub size: f64,
    pub stop_movement_in_last_part_ms: f64,
    pub set: CursorSet,
    /// `"velocity"` tilts the cursor along its direction of travel; `"none"` keeps it upright.
    pub rotation_mode: String,
    pub rotate_on_x_movement_ratio: f64,
    pub base_rotation: f64,
    pub use_default_if_app_has_low_resolution: bool,
    pub always_use_default: bool,
    pub hide_not_moving_after_ms: Option<f64>,
    pub loop_position_before_end_ms: Option<f64>,
    /// Movements shorter than this (in source px per second) are treated as shake and dropped.
    pub remove_shake_treshold: f64,
    pub optimize_original_types: bool,
    /// `null` = none.
    pub click_effect: Option<ClickEffect>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig {
            hide: false,
            size: 48.0,
            stop_movement_in_last_part_ms: 0.0,
            set: CursorSet::default(),
            rotation_mode: "velocity".into(),
            rotate_on_x_movement_ratio: 0.5,
            base_rotation: 0.0,
            use_default_if_app_has_low_resolution: true,
            always_use_default: false,
            hide_not_moving_after_ms: None,
            loop_position_before_end_ms: None,
            remove_shake_treshold: 500.0,
            optimize_original_types: true,
            click_effect: None,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionsConfig {
    pub enable_transcript: bool,
    pub show_sound_labels: bool,
    pub enable_shortcuts: bool,
    pub hidden_shortcuts: Map<String, Value>,
    pub show_shortcuts_with_single_letters: bool,
    pub shortcuts_size_ratio: f64,
    pub font: String,
    pub size_ratio: f64,
    pub color: HexColor,
    pub background_color: HexColor,
    pub position01: Point,
    /// `"line-by-line"` | `"word-by-word"` …
    pub words_reveal: String,
    /// `"left-anchored"` | `"centered"` …
    pub line_growth: String,
    /// `"appear"` | `"fade"` | `"slide"` …
    pub word_entrance: String,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CaptionsConfig {
    fn default() -> Self {
        CaptionsConfig {
            enable_transcript: true,
            show_sound_labels: false,
            enable_shortcuts: false,
            hidden_shortcuts: Map::new(),
            show_shortcuts_with_single_letters: false,
            shortcuts_size_ratio: 0.1,
            font: "sans-serif".into(),
            size_ratio: 0.05,
            color: "#ffffff".into(),
            background_color: "#000000".into(),
            position01: Point::new(0.5, 1.0),
            words_reveal: "line-by-line".into(),
            line_growth: "left-anchored".into(),
            word_entrance: "appear".into(),
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioConfig {
    pub volume: f64,
    pub mute_microphone: bool,
    /// `"voice"` | `"original"` …
    pub microphone_audio_preset: String,
    pub mute_system_audio: bool,
    pub background_audio_volume: f64,
    /// Bundled track name (under `assets/background-audio/<genre>/`) or user file name.
    pub background_audio_file_name: Option<String>,
    pub mute_background_audio: bool,
    pub mute_external_device_audio: bool,
    /// `"apple-magic-mouse"` | `"logitech-mx-master"` | null
    pub click_sound_effect: Option<String>,
    pub click_sound_effect_volume: f64,
    /// ScreenForge extension: extra microphone gain in dB (0..24), applied after the audio
    /// mode and followed by a limiter. `None` = 0 (off).
    #[serde(default, rename = "sfMicrophoneBoostDb", skip_serializing_if = "Option::is_none")]
    pub microphone_boost_db: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl AudioConfig {
    pub const MAX_MICROPHONE_BOOST_DB: f64 = 24.0;

    /// Microphone boost in dB, 0 when unset.
    pub fn microphone_boost_db(&self) -> f64 {
        self.microphone_boost_db.filter(|b| b.is_finite()).unwrap_or(0.0).clamp(0.0, Self::MAX_MICROPHONE_BOOST_DB)
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            volume: 1.0,
            mute_microphone: false,
            microphone_audio_preset: "voice".into(),
            mute_system_audio: false,
            background_audio_volume: 0.05,
            background_audio_file_name: None,
            mute_background_audio: false,
            mute_external_device_audio: false,
            click_sound_effect: None,
            click_sound_effect_volume: 0.25,
            microphone_boost_db: None,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraConfig {
    pub hide: bool,
    pub mirror: bool,
    /// Width / height. `1` = square.
    pub aspect_ratio: f64,
    pub enable_face_tracking: bool,
    pub hide_during_silence_ms: Option<f64>,
    pub lut: Option<String>,
    pub lut_intensity: f64,
    pub grain: f64,
    pub sharpen01: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop01: Option<Rect>,
    pub background: CameraBackground,
    /// ScreenForge extension: how tightly face tracking frames the face (1 = default framing,
    /// smaller = wider shot, larger = closer). `None` = 1.
    #[serde(default, rename = "sfFaceTrackingZoom", skip_serializing_if = "Option::is_none")]
    pub face_tracking_zoom: Option<f64>,
    /// ScreenForge extension: DaVinci Resolve style primaries for the camera picture.
    /// `None` = no grade (neutral).
    #[serde(default, rename = "sfColorGrade", skip_serializing_if = "Option::is_none")]
    pub color_grade: Option<ColorGrade>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CameraConfig {
    fn default() -> Self {
        CameraConfig {
            hide: false,
            mirror: false,
            aspect_ratio: 1.0,
            enable_face_tracking: false,
            hide_during_silence_ms: None,
            lut: None,
            lut_intensity: 1.0,
            grain: 0.0,
            sharpen01: 0.0,
            crop01: None,
            background: CameraBackground::default(),
            face_tracking_zoom: None,
            color_grade: None,
            extra: Extra::new(),
        }
    }
}

impl CameraConfig {
    pub fn face_tracking_zoom(&self) -> f64 {
        self.face_tracking_zoom.filter(|z| z.is_finite() && *z > 0.0).unwrap_or(1.0)
    }

    /// The grade in effect (`sfColorGrade`, or the neutral defaults).
    pub fn grade(&self) -> ColorGrade {
        self.color_grade.clone().unwrap_or_default()
    }
}

/// DaVinci Resolve style primaries (`sfColorGrade`, a ScreenForge extension).
///
/// Values are kept in the units Resolve shows in its Primaries palette so a grade reads like a
/// Resolve node: master wheels `Lift 0.00 · Gamma 0.00 · Gain 1.00 · Offset 25.00`, adjustment
/// controls `Temp 0 · Tint 0 · Contrast 1.000 · Pivot 0.435 · Color Boost 0 · Shadows 0 ·
/// Highlights 0 · Saturation 50 · Hue 50` (50 = unity for saturation and hue).
///
/// Each wheel is `[r, g, b, master]`: the puck sets the zero-sum `r, g, b` chroma offset
/// (±0.5 at the wheel's rim), the ring under it the `master`. The effective per-channel value
/// is `master + channel` (offset: `(master − 25) / 50 + channel`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorGrade {
    pub lift: [f64; 4],
    pub gamma: [f64; 4],
    pub gain: [f64; 4],
    pub offset: [f64; 4],
    /// −100 (cooler) … 100 (warmer).
    pub temperature: f64,
    /// −100 (green) … 100 (magenta).
    pub tint: f64,
    /// 0 … 2, unity 1.
    pub contrast: f64,
    /// Contrast pivot, 0 … 1.
    pub pivot: f64,
    /// −100 … 100, saturates the least saturated colours first (vibrance).
    pub color_boost: f64,
    /// −100 … 100.
    pub shadows: f64,
    /// −100 … 100.
    pub highlights: f64,
    /// 0 … 100, unity 50.
    pub saturation: f64,
    /// 0 … 100, unity 50; the full range is one turn of the hue circle.
    pub hue: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ColorGrade {
    pub const LIFT_DEFAULT: f64 = 0.0;
    pub const GAMMA_DEFAULT: f64 = 0.0;
    pub const GAIN_DEFAULT: f64 = 1.0;
    pub const OFFSET_DEFAULT: f64 = 25.0;
    pub const PIVOT_DEFAULT: f64 = 0.435;

    /// `true` when every control sits at its unity value (the grade changes nothing).
    pub fn is_neutral(&self) -> bool {
        *self == ColorGrade { extra: self.extra.clone(), ..Default::default() }
    }

    /// Effective per-channel values (math units): lift and gamma offsets, gain multipliers,
    /// offset added to the signal.
    pub fn lift_rgb(&self) -> [f64; 3] {
        wheel_rgb(&self.lift)
    }
    pub fn gamma_rgb(&self) -> [f64; 3] {
        wheel_rgb(&self.gamma)
    }
    pub fn gain_rgb(&self) -> [f64; 3] {
        wheel_rgb(&self.gain)
    }
    pub fn offset_rgb(&self) -> [f64; 3] {
        let m = (self.offset[3] - Self::OFFSET_DEFAULT) / 50.0;
        [m + self.offset[0], m + self.offset[1], m + self.offset[2]]
    }
}

fn wheel_rgb(w: &[f64; 4]) -> [f64; 3] {
    [w[3] + w[0], w[3] + w[1], w[3] + w[2]]
}

impl Default for ColorGrade {
    fn default() -> Self {
        ColorGrade {
            lift: [0.0, 0.0, 0.0, Self::LIFT_DEFAULT],
            gamma: [0.0, 0.0, 0.0, Self::GAMMA_DEFAULT],
            gain: [0.0, 0.0, 0.0, Self::GAIN_DEFAULT],
            offset: [0.0, 0.0, 0.0, Self::OFFSET_DEFAULT],
            temperature: 0.0,
            tint: 0.0,
            contrast: 1.0,
            pivot: Self::PIVOT_DEFAULT,
            color_boost: 0.0,
            shadows: 0.0,
            highlights: 0.0,
            saturation: 50.0,
            hue: 50.0,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraBackground {
    pub blur_amount01: f64,
    pub edge_falloff01: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CameraBackground {
    fn default() -> Self {
        CameraBackground { blur_amount01: 0.0, edge_falloff01: 0.2, extra: Extra::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationsConfig {
    /// `null` = cursor smoothing disabled.
    pub mouse_movement_spring: Option<Spring>,
    pub screen_movement_spring: Spring,
    pub motion_blur_amount: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl AnimationsConfig {
    pub const DEFAULT_SPRING: Spring =
        Spring { stiffness: 125.0, damping: 12.0, mass: 1.5, clamp: Some(true), precision: Some(0.002) };
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        AnimationsConfig {
            mouse_movement_spring: Some(Self::DEFAULT_SPRING),
            screen_movement_spring: Self::DEFAULT_SPRING,
            motion_blur_amount: 1.0,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingConfig {
    pub improve_recording_quality01: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        ProcessingConfig { improve_recording_quality01: 0.5, extra: Extra::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ZoomsConfig {
    pub start_animation_early: bool,
    /// ScreenForge extension: keystrokes also open/extend automatic zooms (Screen Studio only
    /// uses clicks). `None` means the default, which is on.
    #[serde(default, rename = "sfZoomOnTyping", skip_serializing_if = "Option::is_none")]
    pub zoom_on_typing: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ZoomsConfig {
    pub fn zoom_on_typing(&self) -> bool {
        self.zoom_on_typing.unwrap_or(true)
    }
}

/// Default camera/screen layout. `type` selects which sub-config is active; all three are
/// always persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutConfig {
    /// `"camera-overlay"` | `"split-screen"` | `"cutout-camera"` | `"screen-only"` | `"fullscreen-camera"`
    #[serde(rename = "type")]
    pub kind: String,
    pub camera_overlay: CameraOverlayLayout,
    pub split_screen: SplitScreenLayout,
    pub cutout_camera: CutoutCameraLayout,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            kind: layout_type::CAMERA_OVERLAY.into(),
            camera_overlay: CameraOverlayLayout::default(),
            split_screen: SplitScreenLayout::default(),
            cutout_camera: CutoutCameraLayout::default(),
            extra: Extra::new(),
        }
    }
}

/// `cursor.clickEffect`: `{type:"ripple", pullSize01?}` or `{type:"circle"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClickEffect {
    Ripple {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pull_size01: Option<f64>,
    },
    Circle,
}

pub mod layout_type {
    pub const CAMERA_OVERLAY: &str = "camera-overlay";
    pub const SPLIT_SCREEN: &str = "split-screen";
    pub const CUTOUT_CAMERA: &str = "cutout-camera";
    pub const SCREEN_ONLY: &str = "screen-only";
    pub const FULLSCREEN_CAMERA: &str = "fullscreen-camera";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOverlayLayout {
    pub overlay_camera_position01: Point,
    pub overlay_camera_size_ratio01: f64,
    pub overlay_camera_padding_ratio01: f64,
    pub overlay_camera_aspect_ratio: f64,
    pub overlay_zoomed_camera_scale: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CameraOverlayLayout {
    fn default() -> Self {
        CameraOverlayLayout {
            overlay_camera_position01: Point::new(1.0, 1.0),
            overlay_camera_size_ratio01: 0.5,
            overlay_camera_padding_ratio01: 0.5,
            overlay_camera_aspect_ratio: 1.0,
            overlay_zoomed_camera_scale: 0.8,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitScreenLayout {
    /// `"auto"` | `"horizontal"` | `"vertical"`
    pub split_direction: String,
    pub split_flip: bool,
    pub split_screen_ratio01: Option<f64>,
    pub enable_padding: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for SplitScreenLayout {
    fn default() -> Self {
        SplitScreenLayout {
            split_direction: "auto".into(),
            split_flip: false,
            split_screen_ratio01: None,
            enable_padding: false,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutoutCameraLayout {
    pub cutout_camera_position_x01: f64,
    pub cutout_camera_size_ratio01: f64,
    pub cutout_camera_zoomed_scale: f64,
    pub cutout_camera_aspect_ratio: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CutoutCameraLayout {
    fn default() -> Self {
        CutoutCameraLayout {
            cutout_camera_position_x01: 1.0,
            cutout_camera_size_ratio01: 0.5,
            cutout_camera_zoomed_scale: 0.8,
            cutout_camera_aspect_ratio: None,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    pub enable_mockup: bool,
    /// Device frame asset key (e.g. `"iPad Pro 11 M4 Silver"`), `null` = auto/none.
    pub frame_key: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig { enable_mockup: true, frame_key: None, extra: Extra::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfig {
    /// `"auto"` or a number (width / height).
    pub aspect_ratio: AspectRatio,
    pub padding_ratio01: f64,
    pub avoid_empty_zoom_area: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig {
            aspect_ratio: AspectRatio::AUTO,
            padding_ratio01: 0.1,
            avoid_empty_zoom_area: false,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylesConfig {
    pub screen_border_radius: f64,
    pub camera_border_radius_ratio01: f64,
    pub screen_inset: ScreenInset,
    pub shadow: Shadow,
    pub background: Background,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for StylesConfig {
    fn default() -> Self {
        StylesConfig {
            screen_border_radius: 16.0,
            camera_border_radius_ratio01: 0.15,
            screen_inset: ScreenInset::default(),
            shadow: Shadow::default(),
            background: Background::default(),
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenInset {
    pub size: f64,
    pub origin01: Point,
    pub alpha: f64,
    pub color: HexColor,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ScreenInset {
    fn default() -> Self {
        ScreenInset {
            size: 0.0,
            origin01: Point::new(0.0, 0.0),
            alpha: 0.5,
            color: "#000000".into(),
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shadow {
    pub intensity: f64,
    pub distance: f64,
    /// Degrees; 45 = down-right.
    pub angle: f64,
    pub blur: f64,
    pub is_directional: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            intensity: 0.3,
            distance: 25.0,
            angle: 45.0,
            blur: 20.0,
            is_directional: true,
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Background {
    /// `"system"` | `"color"` | `"gradient"` | `"image"` | `"unsplash"` | `"wallpaper"` | `"none"`
    #[serde(rename = "type")]
    pub kind: String,
    pub color: HexColor,
    pub gradient: Gradient,
    /// Bundled wallpaper relative to `assets/backgrounds/`, e.g. `"macOS/san-francisco-dark.jpg"`.
    pub system_name: Option<String>,
    /// User supplied image file name stored inside the package.
    pub image: Option<Value>,
    pub unsplash: Option<Value>,
    pub blur: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

pub mod background_type {
    pub const SYSTEM: &str = "system";
    pub const COLOR: &str = "color";
    pub const GRADIENT: &str = "gradient";
    pub const IMAGE: &str = "image";
    pub const UNSPLASH: &str = "unsplash";
}

impl Default for Background {
    fn default() -> Self {
        Background {
            kind: background_type::SYSTEM.into(),
            color: "#4776e6".into(),
            gradient: Gradient::default(),
            system_name: Some("macOS/san-francisco-dark.jpg".into()),
            image: None,
            unsplash: None,
            blur: 0.0,
            extra: Extra::new(),
        }
    }
}

// ───────────────────────────── scenes ─────────────────────────────

/// A scene is one timeline. `type: "recording"` scenes play the captured session; other
/// scene types (kept generically in `extra`) exist for intros/outros.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub zooms: Vec<Zoom>,
    #[serde(default)]
    pub slices: Vec<Slice>,
    #[serde(default)]
    pub layouts: Vec<Layout>,
    #[serde(default)]
    pub masks: Vec<Mask>,
    #[serde(default)]
    pub voice_overs: Vec<VoiceOver>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A camera layout override over a source-time range. Options not set here fall back to
/// `config.defaultLayout`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    #[serde(default = "two")]
    pub version: u32,
    /// `screen-only` | `camera-overlay` | `split-screen` | `fullscreen-camera` | `cutout-camera`
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub is_disabled: bool,
    #[serde(default)]
    pub camera_overlay: Map<String, Value>,
    #[serde(default)]
    pub split_screen: Map<String, Value>,
    #[serde(default)]
    pub fullscreen_camera: Map<String, Value>,
    #[serde(default)]
    pub screen_only: Map<String, Value>,
    #[serde(default)]
    pub cutout_camera: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

fn two() -> u32 {
    2
}

impl Layout {
    pub const MIN_DURATION_MS: f64 = 500.0;
    pub const INIT_DURATION_MS: f64 = 3000.0;

    pub fn new(start_ms: f64, end_ms: f64, kind: &str) -> Self {
        let mut camera_overlay = Map::new();
        camera_overlay.insert("overlayCameraPosition01".into(), serde_json::json!({ "x": 1, "y": 1 }));
        let mut cutout = Map::new();
        cutout.insert("cutoutCameraPositionX01".into(), serde_json::json!(1));
        Layout {
            id: new_id(),
            source_start_ms: start_ms,
            source_end_ms: end_ms,
            version: 2,
            kind: kind.into(),
            is_disabled: false,
            camera_overlay,
            split_screen: Map::new(),
            fullscreen_camera: Map::new(),
            screen_only: Map::new(),
            cutout_camera: cutout,
            extra: Extra::new(),
        }
    }
}

/// A static mask over the recording. `bounds` are pixel rects in cropped-recording space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub id: String,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    #[serde(default)]
    pub is_disabled: bool,
    /// `sensitive-data` | `highlight`
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub bounds: Vec<Rect>,
    #[serde(default = "mask_blur")]
    pub blur: f64,
    #[serde(default = "half")]
    pub highlight_mask_opacity: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

fn mask_blur() -> f64 {
    32.0
}
fn half() -> f64 {
    0.5
}

impl Mask {
    pub const MIN_DURATION_MS: f64 = 500.0;
    pub const INIT_DURATION_MS: f64 = 3000.0;
    pub const SENSITIVE_DATA: &'static str = "sensitive-data";
    pub const HIGHLIGHT: &'static str = "highlight";

    pub fn new(start_ms: f64, end_ms: f64, kind: &str, bounds: Vec<Rect>) -> Self {
        Mask { id: new_id(), source_start_ms: start_ms, source_end_ms: end_ms, is_disabled: false, kind: kind.into(), bounds, blur: 32.0, highlight_mask_opacity: 0.5, extra: Extra::new() }
    }
}

/// A voice-over audio file placed at a source time (`<project>/voiceovers/<fileName>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceOver {
    pub id: String,
    pub source_start_ms: f64,
    pub voiceover_duration_ms: f64,
    #[serde(default)]
    pub is_disabled: bool,
    pub file_name: String,
    #[serde(default = "one_f")]
    pub time_scale: f64,
    #[serde(default = "one_f")]
    pub volume: f64,
    /// `{type:"microphone"}` or `{type:"speechify", text, voiceId, language, speechMarks}`
    #[serde(default)]
    pub source: Value,
    #[serde(default = "first_frame")]
    pub attach_mode: String,
    #[serde(default = "yes")]
    pub hide_recording: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

fn one_f() -> f64 {
    1.0
}
fn first_frame() -> String {
    "first-frame".into()
}
fn yes() -> bool {
    true
}

impl Scene {
    pub fn recording(name: &str, index: u32) -> Self {
        Scene {
            kind: "recording".into(),
            id: new_id(),
            name: name.into(),
            index,
            zooms: vec![],
            slices: vec![],
            layouts: vec![],
            masks: vec![],
            voice_overs: vec![],
            extra: Extra::new(),
        }
    }

    /// Output duration of the scene in ms, i.e. the sum of the slices scaled by speed.
    pub fn output_duration_ms(&self) -> f64 {
        self.slices.iter().map(Slice::output_duration_ms).sum()
    }
}

/// A contiguous piece of the source recording placed on the timeline, in order.
/// Gaps between consecutive slices are cuts; `time_scale` > 1 speeds the slice up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slice {
    pub id: String,
    /// Playback speed multiplier (0.5 = 2× faster: output = source × timeScale).
    pub time_scale: f64,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    pub volume: f64,
    pub system_audio_volume: f64,
    pub external_device_audio_volume: f64,
    pub hide_cursor: bool,
    pub disable_smooth_mouse_movement: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Slice {
    pub fn new(source_start_ms: f64, source_end_ms: f64) -> Self {
        Slice {
            id: new_id(),
            time_scale: 1.0,
            source_start_ms,
            source_end_ms,
            volume: 1.0,
            system_audio_volume: 1.0,
            external_device_audio_volume: 1.0,
            hide_cursor: false,
            disable_smooth_mouse_movement: false,
            extra: Extra::new(),
        }
    }

    pub fn source_duration_ms(&self) -> f64 {
        (self.source_end_ms - self.source_start_ms).max(0.0)
    }

    /// Duration on the output timeline. Screen Studio's `timeScale` multiplies source time:
    /// a slice with `timeScale: 0.5` plays in half the time (2× speed).
    pub fn output_duration_ms(&self) -> f64 {
        self.source_duration_ms() * self.time_scale
    }
}

/// A zoom range in *source* time.
///
/// `zoom` is the magnification (1–4, default 2). `kind` is `"follow-click-groups"` /
/// `"follow-mouse"` (automatic target from the cursor) or `"manual"` with `manual_target_point`
/// (normalized to the crop). `presentation` is `"screen"` (classic) or `"loupe"` (glass loupe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Zoom {
    pub id: String,
    pub zoom: f64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_target_point: Option<Point>,
    #[serde(default)]
    pub is_disabled: bool,
    pub source_start_ms: f64,
    pub source_end_ms: f64,
    #[serde(default)]
    pub has_instant_animation: bool,
    /// `"screen"` (classic, default) or `"loupe"` (glass loupe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<String>,
    /// `{radius01:0.35, bevelRatio:0.2, chromaticAberration:0.5, hasGlassOptics:true}` merged over defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loupe_options: Option<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

pub mod zoom_type {
    pub const FOLLOW_CLICK_GROUPS: &str = "follow-click-groups";
    pub const FOLLOW_MOUSE: &str = "follow-mouse";
    pub const MANUAL: &str = "manual";
}

impl Zoom {
    pub const DEFAULT_LEVEL: f64 = 2.0;

    /// An automatic (cursor following) zoom over `[start_ms, end_ms)` of source time.
    pub fn auto(start_ms: f64, end_ms: f64, level: f64) -> Self {
        Zoom {
            id: new_id(),
            zoom: level,
            kind: zoom_type::FOLLOW_CLICK_GROUPS.into(),
            manual_target_point: None,
            is_disabled: false,
            source_start_ms: start_ms.floor(),
            source_end_ms: end_ms.floor(),
            has_instant_animation: false,
            presentation: None,
            loupe_options: None,
            extra: Extra::new(),
        }
    }

    /// A manual zoom centred on `target01` (normalized within the crop).
    pub fn manual(start_ms: f64, end_ms: f64, level: f64, target01: Point) -> Self {
        let mut z = Self::auto(start_ms, end_ms, level);
        z.kind = zoom_type::MANUAL.into();
        z.manual_target_point = Some(target01);
        z
    }

    pub fn is_auto(&self) -> bool {
        self.kind != zoom_type::MANUAL
    }

    pub fn duration_ms(&self) -> f64 {
        (self.source_end_ms - self.source_start_ms).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn microphone_boost_round_trips_and_stays_off_by_default() {
        let a = super::AudioConfig::default();
        assert_eq!(a.microphone_boost_db(), 0.0);
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("sfMicrophoneBoostDb"), "unset boost is not written: {json}");
        let mut b = a.clone();
        b.microphone_boost_db = Some(12.0);
        let back: super::AudioConfig = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.microphone_boost_db(), 12.0);
        b.microphone_boost_db = Some(99.0);
        assert_eq!(b.microphone_boost_db(), super::AudioConfig::MAX_MICROPHONE_BOOST_DB);
        b.microphone_boost_db = Some(f64::NAN);
        assert_eq!(b.microphone_boost_db(), 0.0);
    }

    use super::*;

    #[test]
    fn default_project_serializes() {
        let p = Project::new("Test");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["version"], "4.0.0");
        assert_eq!(v["config"]["styles"]["background"]["type"], "system");
        assert_eq!(v["config"]["output"]["aspectRatio"], "auto");
        assert_eq!(v["config"]["defaultLayout"]["type"], "camera-overlay");
        assert_eq!(v["scenes"][0]["type"], "recording");
    }
}
