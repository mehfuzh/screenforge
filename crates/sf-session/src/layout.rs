//! Camera/screen layouts: `config.defaultLayout` overridden per field by the scene `layouts[]`
//! item active at the source time, resolved into stage-unit rects.
//!
//! Layout types (Screen Studio): `camera-overlay` (small camera pinned to a corner of the
//! visible screen), `split-screen` (screen and camera side by side inside the zoom area),
//! `fullscreen-camera` (camera fills the stage, screen hidden), `screen-only`, and
//! `cutout-camera` (person-matted camera; drawn like an overlay here, matting not implemented).

use serde_json::{Map, Value};
use sf_engine::stage::{fit_inside, StageGeometry};
use sf_project::schema::common::Rect;
use sf_project::v4::{layout_type, CameraOverlayLayout, CutoutCameraLayout, Layout, LayoutConfig, SplitScreenLayout};

/// Camera size = `sizeRatio01 × shorter screen side × OVERLAY_SIZE_SCALE`.
pub const OVERLAY_SIZE_SCALE: f64 = 0.5;
/// Overlay padding from the screen edge = `paddingRatio01 × camera size × OVERLAY_PADDING_SCALE`.
pub const OVERLAY_PADDING_SCALE: f64 = 0.4;
/// Split-screen: the camera cell is cropped to an aspect within this range.
pub const SPLIT_CAMERA_ASPECT: (f64, f64) = (0.5, 2.5);
/// Gap between the screen and the camera in split-screen, as a ratio of the stage height.
pub const SPLIT_GAP_RATIO: f64 = 0.025;
/// Split-screen with `splitScreenRatio01: null`: the camera cell takes what its aspect needs
/// along the split axis, but at most this share of the zoom area.
pub const SPLIT_AUTO_MAX_CAMERA_SHARE: f64 = 0.4;

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLayout {
    pub kind: String,
    pub overlay: CameraOverlayLayout,
    pub split: SplitScreenLayout,
    pub cutout: CutoutCameraLayout,
}

impl ResolvedLayout {
    pub fn is_screen_visible(&self) -> bool {
        self.kind != layout_type::FULLSCREEN_CAMERA
    }

    pub fn is_camera_visible(&self) -> bool {
        self.kind != layout_type::SCREEN_ONLY
    }
}

/// Merge `overrides` (a layout item's sub-object) over a serializable default.
fn merged<T: serde::Serialize + serde::de::DeserializeOwned + Clone>(base: &T, overrides: &Map<String, Value>) -> T {
    if overrides.is_empty() {
        return base.clone();
    }
    let Ok(Value::Object(mut m)) = serde_json::to_value(base) else { return base.clone() };
    for (k, v) in overrides {
        if !v.is_null() || k == "splitScreenRatio01" || k == "cutoutCameraAspectRatio" {
            m.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(Value::Object(m)).unwrap_or_else(|_| base.clone())
}

/// The layout active at `source_ms`: the enabled scene layout covering that time, each field
/// falling back to `defaults`.
pub fn resolve(defaults: &LayoutConfig, layouts: &[Layout], source_ms: f64) -> ResolvedLayout {
    let item = layouts.iter().rev().find(|l| !l.is_disabled && source_ms >= l.source_start_ms && source_ms < l.source_end_ms);
    match item {
        Some(l) => ResolvedLayout {
            kind: l.kind.clone(),
            overlay: merged(&defaults.camera_overlay, &l.camera_overlay),
            split: merged(&defaults.split_screen, &l.split_screen),
            cutout: merged(&defaults.cutout_camera, &l.cutout_camera),
        },
        None => ResolvedLayout { kind: defaults.kind.clone(), overlay: defaults.camera_overlay.clone(), split: defaults.split_screen.clone(), cutout: defaults.cutout_camera.clone() },
    }
}

/// Where the camera goes for a layout, in stage units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraSlot {
    None,
    /// Pinned inside the visible screen; computed per frame from the animated screen rect.
    Overlay { position01: [f64; 2], size_ratio01: f64, padding_ratio01: f64, aspect: f64, zoomed_scale: f64 },
    /// Fixed rect (split-screen cell or the whole stage).
    Fixed(Rect),
}

/// Stage geometry for the screen plus the camera slot under `layout`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutFrame {
    pub screen_geometry: StageGeometry,
    pub camera: CameraSlot,
    pub screen_visible: bool,
}

/// `camera_aspect` = `config.camera.aspectRatio` (used by the automatic split-screen ratio).
pub fn layout_frame(layout: &ResolvedLayout, base: &StageGeometry, camera_aspect: f64) -> LayoutFrame {
    match layout.kind.as_str() {
        layout_type::SCREEN_ONLY => LayoutFrame { screen_geometry: *base, camera: CameraSlot::None, screen_visible: true },
        layout_type::FULLSCREEN_CAMERA => LayoutFrame { screen_geometry: *base, camera: CameraSlot::Fixed(base.stage), screen_visible: false },
        layout_type::SPLIT_SCREEN => {
            let sp = &layout.split;
            let za = base.zoom_area;
            let horizontal = match sp.split_direction.as_str() {
                "horizontal" => true,
                "vertical" => false,
                _ => za.width >= za.height,
            };
            let gap = (base.stage.height * SPLIT_GAP_RATIO).max(za.x.min(za.y));
            let cam_aspect_cfg = if camera_aspect.is_finite() && camera_aspect > 0.0 { camera_aspect } else { 1.0 };
            let screen_share = match sp.split_screen_ratio01.filter(|r| r.is_finite()) {
                Some(r) => r.clamp(0.2, 0.8),
                None => {
                    // camera at its own aspect over the full cross-axis, capped
                    let (axis, cross) = if horizontal { (za.width, za.height) } else { (za.height, za.width) };
                    let wanted = if horizontal { cross * cam_aspect_cfg } else { cross / cam_aspect_cfg };
                    let cam = wanted.min(axis * SPLIT_AUTO_MAX_CAMERA_SHARE);
                    (1.0 - cam / (axis - gap).max(1e-6)).clamp(0.5, 0.8)
                }
            };
            let (screen_cell, camera_cell) = if horizontal {
                let sw = (za.width - gap) * screen_share;
                let cw = za.width - gap - sw;
                let (sx, cx) = if sp.split_flip { (za.x + cw + gap, za.x) } else { (za.x, za.x + sw + gap) };
                (Rect { x: sx, y: za.y, width: sw, height: za.height }, Rect { x: cx, y: za.y, width: cw, height: za.height })
            } else {
                let sh = (za.height - gap) * screen_share;
                let ch = za.height - gap - sh;
                let (sy, cy) = if sp.split_flip { (za.y + ch + gap, za.y) } else { (za.y, za.y + sh + gap) };
                (Rect { x: za.x, y: sy, width: za.width, height: sh }, Rect { x: za.x, y: cy, width: za.width, height: ch })
            };
            let cam_aspect = (camera_cell.width / camera_cell.height.max(1e-6)).clamp(SPLIT_CAMERA_ASPECT.0, SPLIT_CAMERA_ASPECT.1);
            let camera = fit_inside(camera_cell, cam_aspect);
            let screen_geometry = StageGeometry { stage: base.stage, zoom_area: screen_cell, initial_screen: fit_inside(screen_cell, base.screen_aspect), screen_aspect: base.screen_aspect };
            LayoutFrame { screen_geometry, camera: CameraSlot::Fixed(camera), screen_visible: true }
        }
        layout_type::CUTOUT_CAMERA => {
            let c = &layout.cutout;
            LayoutFrame {
                screen_geometry: *base,
                camera: CameraSlot::Overlay {
                    position01: [c.cutout_camera_position_x01.clamp(0.0, 1.0), 1.0],
                    size_ratio01: c.cutout_camera_size_ratio01,
                    padding_ratio01: layout.overlay.overlay_camera_padding_ratio01,
                    aspect: c.cutout_camera_aspect_ratio.filter(|a| *a > 0.0).unwrap_or(layout.overlay.overlay_camera_aspect_ratio),
                    zoomed_scale: c.cutout_camera_zoomed_scale,
                },
                screen_visible: true,
            }
        }
        _ => {
            let o = &layout.overlay;
            LayoutFrame {
                screen_geometry: *base,
                camera: CameraSlot::Overlay {
                    position01: [o.overlay_camera_position01.x.clamp(0.0, 1.0), o.overlay_camera_position01.y.clamp(0.0, 1.0)],
                    size_ratio01: o.overlay_camera_size_ratio01,
                    padding_ratio01: o.overlay_camera_padding_ratio01,
                    aspect: if o.overlay_camera_aspect_ratio > 0.0 { o.overlay_camera_aspect_ratio } else { 1.0 },
                    zoomed_scale: o.overlay_zoomed_camera_scale,
                },
                screen_visible: true,
            }
        }
    }
}

pub fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect { x: x0, y: y0, width: (x1 - x0).max(0.0), height: (y1 - y0).max(0.0) }
}

/// Camera rect (stage units) for an overlay slot: `size_ratio01 × shorter side of the zoom-1
/// screen`, scaled by `zoomed_scale` while zoomed, pinned at `position01` inside the visible
/// part of the (animated) screen with padding.
pub fn overlay_rect(slot: &CameraSlot, geometry: &StageGeometry, screen_rect: Rect, zoomed: bool) -> Option<Rect> {
    let CameraSlot::Overlay { position01, size_ratio01, padding_ratio01, aspect, zoomed_scale } = *slot else { return None };
    let init = geometry.initial_screen;
    let mut size = size_ratio01.clamp(0.05, 1.0) * init.width.min(init.height) * OVERLAY_SIZE_SCALE;
    if zoomed {
        size *= zoomed_scale.clamp(0.1, 2.0);
    }
    let (w, h) = if aspect >= 1.0 { (size * aspect, size) } else { (size, size / aspect) };
    let pad = padding_ratio01.max(0.0) * size * OVERLAY_PADDING_SCALE;
    let anchor = intersect(screen_rect, geometry.stage);
    let free_w = (anchor.width - 2.0 * pad - w).max(0.0);
    let free_h = (anchor.height - 2.0 * pad - h).max(0.0);
    Some(Rect { x: anchor.x + pad + free_w * position01[0], y: anchor.y + pad + free_h * position01[1], width: w, height: h })
}

/// Centre-crop `src01` (a region of a texture with pixel aspect `src_aspect`) to `dst_aspect`.
pub fn cover_crop(src01: Rect, src_aspect: f64, dst_aspect: f64) -> Rect {
    if !(src_aspect > 0.0 && dst_aspect > 0.0) {
        return src01;
    }
    if src_aspect > dst_aspect {
        let w = src01.width * dst_aspect / src_aspect;
        Rect { x: src01.x + (src01.width - w) / 2.0, y: src01.y, width: w, height: src01.height }
    } else {
        let h = src01.height * src_aspect / dst_aspect;
        Rect { x: src01.x, y: src01.y + (src01.height - h) / 2.0, width: src01.width, height: h }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::schema::common::Point;

    fn base() -> StageGeometry {
        StageGeometry::new("auto", 4.0 / 3.0, 0.1, false)
    }

    #[test]
    fn layout_item_overrides_defaults_per_field() {
        let mut defaults = LayoutConfig::default();
        defaults.camera_overlay.overlay_camera_size_ratio01 = 0.35;
        let mut item = Layout::new(1000.0, 2000.0, layout_type::CAMERA_OVERLAY);
        item.camera_overlay.insert("overlayCameraPosition01".into(), serde_json::json!({"x": 0, "y": 0}));
        let r = resolve(&defaults, &[item.clone()], 1500.0);
        assert_eq!(r.overlay.overlay_camera_position01, Point::new(0.0, 0.0));
        assert_eq!(r.overlay.overlay_camera_size_ratio01, 0.35);
        // outside the item → defaults
        let r = resolve(&defaults, &[item.clone()], 2500.0);
        assert_eq!(r.overlay.overlay_camera_position01, Point::new(1.0, 1.0));
        // disabled items are ignored
        item.is_disabled = true;
        let r = resolve(&defaults, &[item], 1500.0);
        assert_eq!(r.overlay.overlay_camera_position01, Point::new(1.0, 1.0));
    }

    #[test]
    fn overlay_sits_in_bottom_right_corner() {
        let g = base();
        let layout = resolve(&LayoutConfig::default(), &[], 0.0);
        let frame = layout_frame(&layout, &g, 1.0);
        let cam = overlay_rect(&frame.camera, &g, g.initial_screen, false).unwrap();
        let s = g.initial_screen;
        assert!(cam.x + cam.width < s.x + s.width && cam.y + cam.height < s.y + s.height);
        assert!(cam.x > s.x + s.width / 2.0 && cam.y > s.y + s.height / 2.0);
        assert!((cam.width - cam.height).abs() < 1e-9, "square camera for aspect 1");
        // zoomed → scaled by overlayZoomedCameraScale, still inside the stage
        let zoomed = overlay_rect(&frame.camera, &g, g.screen_rect([1.0, 1.0], 2.0), true).unwrap();
        assert!((zoomed.width - cam.width * 0.8).abs() < 1e-6);
        assert!(zoomed.x + zoomed.width <= g.stage.width + 1e-9);
    }

    #[test]
    fn split_screen_puts_camera_beside_screen() {
        let g = base();
        let mut defaults = LayoutConfig::default();
        defaults.kind = layout_type::SPLIT_SCREEN.into();
        let layout = resolve(&defaults, &[], 0.0);
        let frame = layout_frame(&layout, &g, 1.0);
        let CameraSlot::Fixed(cam) = frame.camera else { panic!("fixed camera expected") };
        let screen = frame.screen_geometry.initial_screen;
        assert!(screen.x + screen.width <= cam.x + 1e-9, "screen left of camera");
        assert!(cam.width <= g.zoom_area.width * SPLIT_AUTO_MAX_CAMERA_SHARE + 1e-9, "auto ratio caps the camera");
        assert!(cam.x + cam.width <= g.zoom_area.x + g.zoom_area.width + 1e-9);
        defaults.split_screen.split_flip = true;
        let flipped = layout_frame(&resolve(&defaults, &[], 0.0), &g, 1.0);
        let CameraSlot::Fixed(cam2) = flipped.camera else { panic!() };
        assert!(cam2.x < flipped.screen_geometry.initial_screen.x);
    }

    #[test]
    fn fullscreen_hides_screen_and_cover_crop_keeps_aspect() {
        let g = base();
        let mut defaults = LayoutConfig::default();
        defaults.kind = layout_type::FULLSCREEN_CAMERA.into();
        let frame = layout_frame(&resolve(&defaults, &[], 0.0), &g, 1.0);
        assert!(!frame.screen_visible);
        assert_eq!(frame.camera, CameraSlot::Fixed(g.stage));
        let c = cover_crop(Rect::UNIT, 4.0 / 3.0, 1.0);
        assert!((c.width - 0.75).abs() < 1e-9 && (c.x - 0.125).abs() < 1e-9 && c.height == 1.0);
    }
}
