//! Migration of older project schemas to the current v4 schema, mirroring Screen Studio's
//! own migrator (`project.json` without a `version` field → "3.0.0" → "4.0.0").
//!
//! Field-by-field v3 → v4 mapping.

use serde_json::{Map, Value};

use super::common::*;
use super::v3;
use super::v4;
use crate::ids::new_id;

/// Detects the schema version of a decoded `project.json` (the inner `json` value).
pub fn detect_version(json: &Value) -> &str {
    if let Some(v) = json.get("version").and_then(Value::as_str) {
        return v;
    }
    let cfg = json.get("config");
    let is_v3 = cfg.map(|c| c.get("recordingCrop").is_some() || c.get("crop").is_none()).unwrap_or(true);
    if is_v3 {
        "3.0.0"
    } else {
        "4.0.0"
    }
}

/// Load a project of any supported version, migrating to v4 as needed.
pub fn load_any(json: Value) -> crate::Result<v4::Project> {
    let v = detect_version(&json).to_string();
    let major = v.split('.').next().unwrap_or("");
    match major {
        "4" => {
            let mut p: v4::Project = serde_json::from_value(json).map_err(|e| crate::Error::SuperJson(e.to_string()))?;
            normalize(&mut p);
            Ok(p)
        }
        "3" => {
            let old: v3::Project = serde_json::from_value(json).map_err(|e| crate::Error::SuperJson(e.to_string()))?;
            let mut p = migrate_v3(old);
            normalize(&mut p);
            Ok(p)
        }
        _ => Err(crate::Error::UnsupportedVersion(v)),
    }
}

/// Post-load normaliser applied to every project (Screen Studio's `Hge`): fills fields
/// added after 4.0.0 and guarantees at least one slice.
pub fn normalize(p: &mut v4::Project) {
    // cursor.size: values <= 12 are legacy multipliers of a 16 px cursor
    if p.config.cursor.size <= 12.0 {
        p.config.cursor.size *= 16.0;
    }
    for scene in &mut p.scenes {
        for l in &mut scene.layouts {
            l.cutout_camera.entry("cutoutCameraPositionX01").or_insert(Value::from(1));
        }
    }
}

fn snap_to_anchor(p: Point) -> Point {
    if (p.x - 0.5).abs() < 1e-9 && (p.y - 0.5).abs() < 1e-9 {
        return Point::new(1.0, 1.0);
    }
    let snap = |v: f64| {
        if v < 1.0 / 3.0 {
            0.0
        } else if v > 2.0 / 3.0 {
            1.0
        } else {
            0.5
        }
    };
    Point::new(snap(p.x), snap(p.y))
}

fn point_of(v: Option<&Value>) -> Option<Point> {
    serde_json::from_value(v?.clone()).ok()
}

fn num(v: Option<&Value>) -> Option<f64> {
    v?.as_f64()
}

fn clamp_or(v: Option<f64>, lo: f64, hi: f64, fallback: f64) -> f64 {
    match v {
        Some(x) if x.is_finite() => x.clamp(lo, hi),
        _ => fallback,
    }
}

pub fn migrate_v3(old: v3::Project) -> v4::Project {
    let c = old.config;
    let mut cfg = v4::Config::default();

    // crop
    cfg.crop.rect01 = c.recording_crop.unwrap_or(Rect::UNIT);

    // cursor
    let cur = &mut cfg.cursor;
    cur.hide = c.hide_cursor.unwrap_or(false);
    cur.size = c.cursor_size.unwrap_or(48.0);
    cur.stop_movement_in_last_part_ms = c.stop_cursor_movement_in_last_part_ms.unwrap_or(0.0);
    cur.set = c.cursor_set.unwrap_or_else(|| CursorSet {
        id: if c.cursor_type.as_deref() == Some("cursorTouch") { "touch" } else { "macos-tahoe" }.into(),
        variants: Map::new(),
    });
    cur.click_effect = c.click_effect.as_deref().and_then(|e| match e {
        "circle" => Some(v4::ClickEffect::Circle),
        "ripple" => Some(v4::ClickEffect::Ripple { pull_size01: None }),
        _ => None,
    });
    cur.rotate_on_x_movement_ratio = c.cursor_rotate_on_x_movement_ratio.unwrap_or(0.5);
    cur.base_rotation = c.cursor_base_rotation.unwrap_or(0.0);
    cur.use_default_if_app_has_low_resolution = c.use_default_cursor_if_app_cursor_has_low_resolution.unwrap_or(true);
    cur.always_use_default = c.always_use_default_cursor.unwrap_or(false);
    cur.hide_not_moving_after_ms = c.hide_not_moving_cursor_after_ms;
    cur.loop_position_before_end_ms = c.loop_cursor_position_before_end_ms;
    cur.remove_shake_treshold = c.remove_curshor_shake_treshold.unwrap_or(500.0);
    cur.optimize_original_types = c.optimize_original_cursor_types.unwrap_or(true);

    // captions
    let cap = &mut cfg.captions;
    cap.enable_transcript = c.show_transcript.unwrap_or(true);
    cap.enable_shortcuts = c.show_shortcuts.unwrap_or(false);
    cap.hidden_shortcuts = c.hidden_shortcuts.unwrap_or_default();
    cap.show_shortcuts_with_single_letters = c.show_shortcuts_with_single_letters.unwrap_or(false);

    // audio
    let au = &mut cfg.audio;
    au.volume = c.audio_volume.unwrap_or(1.0);
    au.mute_microphone = c.mute_microphone.unwrap_or(false);
    au.mute_system_audio = c.mute_system_audio.unwrap_or(false);
    au.mute_external_device_audio = c.mute_external_device_audio.unwrap_or(false);
    au.background_audio_file_name = c.background_audio_file_name.or(c.background_audio);
    au.mute_background_audio = c.mute_background_audio.unwrap_or(false);
    au.background_audio_volume = c.background_audio_volume.unwrap_or(0.05);
    au.click_sound_effect = c.click_sound_effect;
    au.click_sound_effect_volume = c.click_sound_effect_volume.unwrap_or(0.25);

    // camera
    let cam = &mut cfg.camera;
    cam.hide = c.hide_camera.unwrap_or(false);
    cam.mirror = c.mirror_camera.unwrap_or(false);
    cam.aspect_ratio = match c.camera_aspect_ratio.as_ref().and_then(Value::as_str) {
        Some("horizontal") => 16.0 / 9.0,
        Some("vertical") => 9.0 / 16.0,
        _ => 1.0,
    };

    // animations
    let an = &mut cfg.animations;
    let with_defaults = |s: Spring| Spring { clamp: s.clamp.or(Some(true)), precision: s.precision.or(Some(0.002)), ..s };
    an.mouse_movement_spring = if c.disable_mouse_movement_spring.unwrap_or(false) { None } else { Some(c.mouse_movement_spring.map(with_defaults).unwrap_or(v4::AnimationsConfig::DEFAULT_SPRING)) };
    an.screen_movement_spring = c.screen_movement_spring.map(with_defaults).unwrap_or(v4::AnimationsConfig::DEFAULT_SPRING);
    an.motion_blur_amount = c.motion_blur_amount.unwrap_or(1.0);

    // styles
    let st = &mut cfg.styles;
    st.screen_border_radius = c.window_border_radius.unwrap_or(16.0);
    st.camera_border_radius_ratio01 = c.camera_roundness.unwrap_or(0.25);
    let side = |k: &str| c.inset_padding.as_ref().and_then(|p| p.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
    let (t, l, r, b) = (side("top"), side("left"), side("right"), side("bottom"));
    st.screen_inset = if t + l + r + b == 0.0 {
        v4::ScreenInset::default()
    } else {
        v4::ScreenInset {
            size: (l + r).max(t + b) / 2.0,
            origin01: Point::new(if l + r > 0.0 { l / (l + r) } else { 0.5 }, if t + b > 0.0 { t / (t + b) } else { 0.5 }),
            alpha: c.inset_alpha.unwrap_or(0.5),
            color: c.inset_color.unwrap_or_else(|| "#000000".into()),
            extra: Extra::new(),
        }
    };
    st.background = v4::Background {
        kind: c.background_type.unwrap_or_else(|| "color".into()),
        color: c.background_color.unwrap_or_else(|| "#3F37C9".into()),
        gradient: c.background_gradient.unwrap_or_else(|| Gradient {
            start: Point::new(0.0, 0.0),
            end: Point::new(1.0, 1.0),
            stops: vec![GradientStop { color: "#3F37C9".into(), at: 0.0 }, GradientStop { color: "#8C87DF".into(), at: 1.0 }],
        }),
        system_name: c.background_system_name,
        image: c.background_image,
        unsplash: None,
        blur: c.background_blur.unwrap_or(0.0),
        extra: Extra::new(),
    };
    st.shadow = v4::Shadow {
        intensity: clamp_or(c.shadow_intensity, 0.0, 0.6, 0.3),
        distance: clamp_or(c.shadow_distance, 0.0, 100.0, 25.0),
        angle: clamp_or(c.shadow_angle, 0.0, 180.0, 90.0),
        blur: clamp_or(c.shadow_blur, 0.0, 30.0, 20.0),
        is_directional: c.shadow_is_directional.unwrap_or(false),
        extra: Extra::new(),
    };

    // device
    cfg.device.enable_mockup = c.enable_device_mockup.unwrap_or(true);
    cfg.device.frame_key = c.device_frame_key;

    // output
    cfg.output.aspect_ratio = match c.default_output_aspect_ratio.as_ref() {
        Some(Value::Object(o)) => match (o.get("x").and_then(Value::as_f64), o.get("y").and_then(Value::as_f64)) {
            (Some(x), Some(y)) if y > 0.0 && (x / y).is_finite() => AspectRatio::Ratio(x / y),
            _ => AspectRatio::AUTO,
        },
        Some(Value::Number(n)) => n.as_f64().map(AspectRatio::Ratio).unwrap_or(AspectRatio::AUTO),
        _ => AspectRatio::AUTO,
    };
    cfg.output.padding_ratio01 = c.background_padding_ratio.unwrap_or(10.0) / 100.0;
    cfg.output.avoid_empty_zoom_area = false;

    // default layout
    let dl = c.default_layout.as_ref();
    let dcm = c.default_camera_mode.as_ref();
    let pos = point_of(dl.and_then(|l| l.get("cameraPositionPoint")))
        .or_else(|| point_of(dcm.and_then(|m| m.get("cameraPositionPoint"))))
        .or(c.camera_position_point)
        .unwrap_or_else(|| if c.camera_position.as_deref() == Some("bottom-left") { Point::new(0.0, 1.0) } else { Point::new(1.0, 1.0) });
    let lay = &mut cfg.default_layout;
    lay.kind = v4::layout_type::CAMERA_OVERLAY.into();
    lay.camera_overlay = v4::CameraOverlayLayout {
        overlay_camera_position01: snap_to_anchor(pos),
        overlay_camera_size_ratio01: num(dl.and_then(|l| l.get("cameraSize"))).or_else(|| num(dcm.and_then(|m| m.get("cameraSize")))).or(c.camera_size).unwrap_or(0.35),
        overlay_camera_padding_ratio01: 0.5,
        overlay_camera_aspect_ratio: cam.aspect_ratio,
        overlay_zoomed_camera_scale: c.camera_scale_during_zoom.unwrap_or(0.7),
        extra: Extra::new(),
    };
    lay.split_screen = v4::SplitScreenLayout::default();
    lay.cutout_camera = v4::CutoutCameraLayout::default();

    // scenes
    let zoom_ranges_default: Vec<Value> = vec![];
    let scenes: Vec<v4::Scene> = if old.scenes.is_empty() {
        vec![v4::Scene::recording("Default", 0)]
    } else {
        old.scenes
            .into_iter()
            .map(|s| {
                let mut extra = s.extra;
                if !s.resolved_typing_speed_increase_suggestions.is_empty() {
                    extra.insert("resolvedTypingSpeedIncreaseSuggestions".into(), Value::Array(s.resolved_typing_speed_increase_suggestions));
                }
                let layouts_src = s.layouts.or(s.camera_modes).unwrap_or_default();
                v4::Scene {
                    kind: "recording".into(),
                    id: s.id,
                    name: s.name,
                    index: s.session_index,
                    zooms: s.zoom_ranges.iter().filter_map(migrate_zoom).collect(),
                    slices: s.slices,
                    layouts: layouts_src.iter().filter_map(migrate_layout).collect(),
                    masks: s.masks.iter().filter_map(migrate_mask).collect(),
                    voice_overs: s.voice_overs.iter().filter_map(migrate_voice_over).collect(),
                    extra,
                }
            })
            .collect()
    };
    let _ = zoom_ranges_default;

    let mut data = Map::new();
    data.insert("recordingFlags".into(), old.meta.get("recordingFlags").cloned().unwrap_or(Value::Array(vec![])));

    v4::Project {
        version: super::CURRENT_VERSION.into(),
        id: old.id,
        name: old.name,
        created_at: old.created_at,
        updated_at: old.updated_at,
        last_saved_at: old.last_saved_at,
        config: cfg,
        scenes,
        data,
        extra: old.extra,
    }
}

fn range_of(v: &Value) -> Option<(f64, f64)> {
    let s = num(v.get("startTime")).or_else(|| num(v.get("sourceStartMs")))?;
    let e = num(v.get("endTime")).or_else(|| num(v.get("sourceEndMs"))).unwrap_or(s);
    Some((s, e))
}

fn id_of(v: &Value) -> String {
    v.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(new_id)
}

fn migrate_zoom(z: &Value) -> Option<v4::Zoom> {
    let (s, e) = range_of(z)?;
    let target = point_of(z.get("manualTargetPoint")).or_else(|| {
        let a: Rect = serde_json::from_value(z.get("manualZoomArea")?.clone()).ok()?;
        Some(Point::new(a.x + a.width / 2.0, a.y + a.height / 2.0))
    });
    let kind = z.get("type").and_then(Value::as_str).unwrap_or(v4::zoom_type::FOLLOW_CLICK_GROUPS).to_string();
    Some(v4::Zoom {
        id: id_of(z),
        zoom: num(z.get("zoom")).unwrap_or(v4::Zoom::DEFAULT_LEVEL),
        kind,
        manual_target_point: target,
        is_disabled: z.get("isDisabled").and_then(Value::as_bool).unwrap_or(false),
        source_start_ms: s,
        source_end_ms: e,
        has_instant_animation: z.get("hasInstantAnimation").and_then(Value::as_bool).unwrap_or(false),
        presentation: None,
        loupe_options: None,
        extra: Extra::new(),
    })
}

fn migrate_layout(l: &Value) -> Option<v4::Layout> {
    let (s, e) = range_of(l)?;
    let kind = match l.get("type").and_then(Value::as_str) {
        Some("only-screen") => v4::layout_type::SCREEN_ONLY,
        Some("only-camera") => v4::layout_type::FULLSCREEN_CAMERA,
        _ => v4::layout_type::CAMERA_OVERLAY,
    };
    let mut out = v4::Layout::new(s, e, kind);
    out.id = id_of(l);
    out.is_disabled = l.get("isDisabled").and_then(Value::as_bool).unwrap_or(false);
    let pos = snap_to_anchor(point_of(l.get("cameraPositionPoint")).unwrap_or(Point::new(1.0, 1.0)));
    out.camera_overlay.insert("overlayCameraPosition01".into(), serde_json::to_value(pos).unwrap_or(Value::Null));
    Some(out)
}

fn migrate_mask(m: &Value) -> Option<v4::Mask> {
    let (s, e) = range_of(m)?;
    let kind = match m.get("type").and_then(Value::as_str) {
        Some("highlight") => v4::Mask::HIGHLIGHT,
        _ => v4::Mask::SENSITIVE_DATA,
    };
    let bounds: Vec<Rect> = m.get("bounds").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default();
    let mut out = v4::Mask::new(s, e, kind, bounds);
    out.id = id_of(m);
    out.is_disabled = m.get("isDisabled").and_then(Value::as_bool).unwrap_or(false);
    if let Some(b) = num(m.get("blur")) {
        out.blur = b;
    }
    if let Some(o) = num(m.get("highlightMaskOpacity")) {
        out.highlight_mask_opacity = o;
    }
    Some(out)
}

fn migrate_voice_over(v: &Value) -> Option<v4::VoiceOver> {
    let s = num(v.get("startTime")).or_else(|| num(v.get("sourceStartMs")))?;
    Some(v4::VoiceOver {
        id: id_of(v),
        source_start_ms: s,
        voiceover_duration_ms: num(v.get("voiceoverDurationMs")).unwrap_or(0.0),
        is_disabled: v.get("isDisabled").and_then(Value::as_bool).unwrap_or(false),
        file_name: v.get("fileName").and_then(Value::as_str).unwrap_or_default().to_string(),
        time_scale: num(v.get("timeScale")).unwrap_or(1.0),
        volume: num(v.get("volume")).unwrap_or(1.0),
        source: v.get("source").cloned().unwrap_or_else(|| serde_json::json!({ "type": "microphone" })),
        attach_mode: v.get("attachMode").and_then(Value::as_str).unwrap_or("first-frame").to_string(),
        hide_recording: v.get("hideRecording").and_then(Value::as_bool).unwrap_or(true),
        file_offset_ms: 0.0,
        boost_db: 0.0,
        layer: 0,
        extra: Extra::new(),
    })
}
