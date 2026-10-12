//! Screen Studio 2.4 – 3.x project schema (`project.json` without a `version` field, flat
//! `config`). Only the fields needed for migration are typed; everything else is kept as JSON.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::common::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_saved_at: Option<DateTime<Utc>>,
    pub config: Config,
    #[serde(default)]
    pub meta: Map<String, Value>,
    pub scenes: Vec<Scene>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Flat v3 config. Every field is optional so partially written files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub background_type: Option<String>,
    pub background_color: Option<HexColor>,
    pub background_gradient: Option<Gradient>,
    pub background_system_name: Option<String>,
    pub background_image: Option<Value>,
    pub background_blur: Option<f64>,
    pub background_padding_ratio: Option<f64>,
    pub inset_padding: Option<Value>,
    pub inset_color: Option<HexColor>,
    pub inset_alpha: Option<f64>,
    pub motion_blur_amount: Option<f64>,
    pub cursor_size: Option<f64>,
    pub cursor_set: Option<CursorSet>,
    pub cursor_rotate_on_x_movement_ratio: Option<f64>,
    pub cursor_base_rotation: Option<f64>,
    pub use_default_cursor_if_app_cursor_has_low_resolution: Option<bool>,
    pub always_use_default_cursor: Option<bool>,
    pub hide_not_moving_cursor_after_ms: Option<f64>,
    pub loop_cursor_position_before_end_ms: Option<f64>,
    pub remove_curshor_shake_treshold: Option<f64>,
    pub optimize_original_cursor_types: Option<bool>,
    pub click_effect: Option<String>,
    pub click_sound_effect: Option<String>,
    pub click_sound_effect_volume: Option<f64>,
    pub hide_cursor: Option<bool>,
    pub mouse_movement_spring: Option<Spring>,
    pub screen_movement_spring: Option<Spring>,
    pub mouse_click_spring: Option<Spring>,
    pub disable_mouse_movement_spring: Option<bool>,
    pub default_output_aspect_ratio: Option<Value>,
    pub window_border_radius: Option<f64>,
    pub always_keep_zoomed_in: Option<bool>,
    pub shadow_intensity: Option<f64>,
    pub shadow_angle: Option<f64>,
    pub shadow_distance: Option<f64>,
    pub shadow_blur: Option<f64>,
    pub shadow_is_directional: Option<bool>,
    pub hide_camera: Option<bool>,
    pub mirror_camera: Option<bool>,
    pub camera_roundness: Option<f64>,
    pub camera_size: Option<f64>,
    pub camera_position: Option<String>,
    pub camera_position_point: Option<Point>,
    pub camera_scale_during_zoom: Option<f64>,
    pub camera_aspect_ratio: Option<Value>,
    pub stop_cursor_movement_in_last_part_ms: Option<f64>,
    pub show_shortcuts: Option<bool>,
    pub hidden_shortcuts: Option<Map<String, Value>>,
    pub show_shortcuts_with_single_letters: Option<bool>,
    pub show_transcript: Option<bool>,
    pub transcript_size_ratio: Option<f64>,
    pub shortcuts_size_ratio: Option<f64>,
    pub audio_volume: Option<f64>,
    pub mute_microphone: Option<bool>,
    pub mute_system_audio: Option<bool>,
    pub mute_external_device_audio: Option<bool>,
    pub improve_microphone_audio: Option<bool>,
    pub background_audio_file_name: Option<String>,
    pub mute_background_audio: Option<bool>,
    pub background_audio_volume: Option<f64>,
    pub microphone_in_stereo_mode: Option<bool>,
    pub device_frame_key: Option<String>,
    pub default_layout: Option<Value>,
    pub enable_device_mockup: Option<bool>,
    pub adjust_device_frame_to_recording_size: Option<bool>,
    pub recording_range: Option<[f64; 2]>,
    pub recording_crop: Option<Rect>,
    // pre-3.x keys still accepted by Screen Studio's migrator
    pub cursor_type: Option<String>,
    pub background_audio: Option<String>,
    pub default_camera_mode: Option<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub session_index: u32,
    #[serde(default)]
    pub zoom_ranges: Vec<Value>,
    #[serde(default)]
    pub slices: Vec<super::v4::Slice>,
    #[serde(default)]
    pub layouts: Option<Vec<Value>>,
    /// pre-3.x name of `layouts`
    #[serde(default)]
    pub camera_modes: Option<Vec<Value>>,
    #[serde(default)]
    pub masks: Vec<Value>,
    #[serde(default)]
    pub resolved_typing_speed_increase_suggestions: Vec<Value>,
    #[serde(default)]
    pub voice_overs: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}
