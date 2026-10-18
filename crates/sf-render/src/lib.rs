//! GPU frame compositor shared by the editor preview and the exporter.
//!
//! Every layer (background, shadow, screen, camera, masks, click effects, cursor) is one draw
//! of a rounded quad with the shader in `composite.wgsl`; a [`FrameDesc`] lists the layers for
//! one output frame.
//! The target is an offscreen RGBA8 texture that can be read back ([`Compositor::read_pixels`])
//! or shown in egui.

pub mod gpu;
pub mod lut;
pub mod texture;
pub mod text;

pub use gpu::Gpu;
pub use lut::{CubeLut, LutTexture};
pub use texture::Texture;

use sf_project::schema::common::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    /// Parse `#rgb`, `#rrggbb` or `#rrggbbaa` (sRGB) into linear-ish floats (we composite in
    /// sRGB space, matching a browser canvas, so no transfer is applied).
    pub fn from_hex(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|x| x as f32 / 255.0);
        match h.len() {
            3 => {
                let c = |i: usize| u8::from_str_radix(&h[i..i + 1].repeat(2), 16).ok().map(|x| x as f32 / 255.0);
                Some(Color { r: c(0)?, g: c(1)?, b: c(2)?, a: 1.0 })
            }
            6 => Some(Color { r: v(0)?, g: v(2)?, b: v(4)?, a: 1.0 }),
            8 => Some(Color { r: v(0)?, g: v(2)?, b: v(4)?, a: v(6)? }),
            _ => None,
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
}

#[derive(Debug, Clone)]
pub enum BackgroundLayer {
    Color(Color),
    Gradient { start: [f32; 2], end: [f32; 2], stops: Vec<(Color, f32)> },
    /// Cover-fit image.
    Image { texture: std::sync::Arc<Texture>, blur: f32 },
    Transparent,
}

/// Soft shadow under a rounded rect (see `composite.wgsl` mode 3).
///
/// Non-directional (`falloff == 0`): the rect's SDF, shifted by `offset`, feathered over `blur`
/// (full inside `-blur/2`, gone at `+blur`), i.e. Screen Studio's "drop shadow" branch.
/// Directional (`falloff > 0`): cheap stand-in for Screen Studio's light-source ray march: the
/// same feathered body, additionally attenuated by `(1 - d/falloff)²` where `d` is the distance
/// from the *unshifted* rect, so the shadow fades with the distance it is cast.
#[derive(Debug, Clone, Copy)]
pub struct ShadowLayer {
    pub color: Color,
    pub offset: [f32; 2],
    pub blur: f32,
    pub opacity: f32,
    /// Directional falloff distance in output px; `0` = plain drop shadow.
    pub falloff: f32,
}

impl ShadowLayer {
    /// Build from Screen Studio's `styles.shadow` (`distance`, `blur` already in output px).
    pub fn from_style(intensity: f32, distance_px: f32, angle_deg: f32, blur_px: f32, directional: bool) -> Option<ShadowLayer> {
        if intensity <= 0.0 {
            return None;
        }
        let a = angle_deg.to_radians();
        let offset = [a.cos() * distance_px, a.sin() * distance_px];
        let falloff = if directional { distance_px + blur_px * 2.0 } else { 0.0 };
        Some(ShadowLayer { color: Color::BLACK, offset, blur: blur_px.max(0.5), opacity: intensity, falloff })
    }
}

#[derive(Debug, Clone)]
pub struct ScreenLayer {
    pub texture: std::sync::Arc<Texture>,
    /// Visible part of the texture (normalized), i.e. crop × zoom viewport.
    pub src01: Rect,
    /// Destination in output pixels.
    pub dst: Rect,
    pub corner_radius: f32,
    pub shadow: Option<ShadowLayer>,
    /// Inset ring: (size px, colour)
    pub inset: Option<(f32, Color)>,
}

#[derive(Debug, Clone)]
pub struct CameraLayer {
    pub texture: std::sync::Arc<Texture>,
    pub src01: Rect,
    pub dst: Rect,
    pub corner_radius: f32,
    pub mirror: bool,
    pub shadow: Option<ShadowLayer>,
    /// Picture processing (Resolve style primaries, LUT, sharpen, grain); `None` = untouched.
    pub grade: Option<CameraGrade>,
    /// 3D LUT applied by `grade.lut_mix` (ignored when `grade` is `None`).
    pub lut: Option<std::sync::Arc<LutTexture>>,
}

/// Per-pixel camera processing, in math units ready for the shader. Built from the project's
/// Resolve-style [`sf_project::v4::ColorGrade`] with [`CameraGrade::from_config`].
///
/// Order of operations (`composite.wgsl`): sharpen → temperature/tint → offset → lift → gamma
/// → gain → contrast around the pivot → shadows/highlights → color boost → saturation → hue →
/// 3D LUT (mixed by `lut_mix`) → grain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraGrade {
    /// Added to the signal, scaled by `1 − signal` (raises blacks, pins white).
    pub lift: [f32; 3],
    /// Per-channel exponents (`1` = unity).
    pub gamma_exp: [f32; 3],
    /// Per-channel multipliers (`1` = unity).
    pub gain: [f32; 3],
    /// Added to every channel.
    pub offset: [f32; 3],
    /// −1 … 1 (warm).
    pub temperature: f32,
    /// −1 … 1 (magenta).
    pub tint: f32,
    pub contrast: f32,
    pub pivot: f32,
    /// −1 … 1.
    pub shadows: f32,
    /// −1 … 1.
    pub highlights: f32,
    /// −1 … 1.
    pub color_boost: f32,
    /// Multiplier, `1` = unity.
    pub saturation: f32,
    /// Hue rotation in turns (−0.5 … 0.5).
    pub hue_turns: f32,
    /// 0 … 1 blend toward the LUT output.
    pub lut_mix: f32,
    /// 0 … 1.
    pub grain: f32,
    /// 0 … 1.
    pub sharpen: f32,
    /// Changes the grain pattern per frame.
    pub seed: f32,
}

impl CameraGrade {
    pub const NEUTRAL: CameraGrade = CameraGrade {
        lift: [0.0; 3],
        gamma_exp: [1.0; 3],
        gain: [1.0; 3],
        offset: [0.0; 3],
        temperature: 0.0,
        tint: 0.0,
        contrast: 1.0,
        pivot: 0.435,
        shadows: 0.0,
        highlights: 0.0,
        color_boost: 0.0,
        saturation: 1.0,
        hue_turns: 0.0,
        lut_mix: 0.0,
        grain: 0.0,
        sharpen: 0.0,
        seed: 0.0,
    };

    /// Convert Resolve display units. `lut_mix` is `config.camera.lutIntensity` when a LUT is
    /// bound (pass 0 otherwise); `grain`/`sharpen` are `config.camera.grain`/`sharpen01`.
    pub fn from_config(g: &sf_project::v4::ColorGrade, lut_mix: f64, grain: f64, sharpen: f64, seed: f32) -> CameraGrade {
        let f3 = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
        let gamma = g.gamma_rgb();
        let gamma_exp = |v: f64| -> f32 {
            // +γ brightens midtones (exponent < 1), −γ darkens; the same slope either side of 0
            let v = v.clamp(-4.0, 4.0);
            (if v >= 0.0 { 1.0 / (1.0 + v) } else { 1.0 - v }) as f32
        };
        CameraGrade {
            lift: f3(g.lift_rgb()).map(|v| v.clamp(-1.0, 1.0)),
            gamma_exp: [gamma_exp(gamma[0]), gamma_exp(gamma[1]), gamma_exp(gamma[2])],
            gain: f3(g.gain_rgb()).map(|v| v.clamp(0.0, 4.0)),
            offset: f3(g.offset_rgb()).map(|v| v.clamp(-1.0, 1.0)),
            temperature: (g.temperature / 100.0).clamp(-1.0, 1.0) as f32,
            tint: (g.tint / 100.0).clamp(-1.0, 1.0) as f32,
            contrast: g.contrast.clamp(0.0, 4.0) as f32,
            pivot: g.pivot.clamp(0.0, 1.0) as f32,
            shadows: (g.shadows / 100.0).clamp(-1.0, 1.0) as f32,
            highlights: (g.highlights / 100.0).clamp(-1.0, 1.0) as f32,
            color_boost: (g.color_boost / 100.0).clamp(-1.0, 1.0) as f32,
            saturation: (g.saturation / 50.0).clamp(0.0, 4.0) as f32,
            hue_turns: ((g.hue - 50.0) / 100.0).clamp(-0.5, 0.5) as f32,
            lut_mix: lut_mix.clamp(0.0, 1.0) as f32,
            grain: grain.clamp(0.0, 1.0) as f32,
            sharpen: sharpen.clamp(0.0, 1.0) as f32,
            seed,
        }
    }

    pub fn is_neutral(&self) -> bool {
        CameraGrade { seed: self.seed, ..Self::NEUTRAL } == *self
    }
}

#[derive(Debug, Clone)]
pub struct SpriteLayer {
    pub texture: std::sync::Arc<Texture>,
    /// Top-left in output pixels (already offset by the hotspot).
    pub dst: Rect,
    pub pivot: [f32; 2],
    pub rotation_rad: f32,
    pub opacity: f32,
}

#[derive(Debug, Clone)]
pub struct SolidLayer {
    pub dst: Rect,
    pub color: Color,
    pub corner_radius: f32,
}

/// Screen Studio masks (`scene.masks[]`), drawn over the screen layer.
#[derive(Debug, Clone)]
pub enum MaskLayer {
    /// `sensitive-data`: re-sample a region of the screen texture blurred and tint it toward
    /// `cover` (the average colour around the region) by `cover_mix`.
    Blur {
        texture: std::sync::Arc<Texture>,
        /// Part of the texture under `dst` (normalized).
        src01: Rect,
        /// Texture region the blur may read from (normalized; the visible crop).
        clip01: Rect,
        dst: Rect,
        corner_radius: f32,
        /// Blur radius in texture pixels.
        radius_px: f32,
        cover: Color,
        cover_mix: f32,
    },
    /// `highlight`: darken `dst` (the screen) by `opacity` everywhere except inside `holes`.
    Highlight { dst: Rect, corner_radius: f32, opacity: f32, holes: Vec<Rect> },
}

/// A translucent disc with a ring at its edge (click effects).
#[derive(Debug, Clone, Copy)]
pub struct RingLayer {
    pub center: [f32; 2],
    pub radius: f32,
    /// Ring thickness in px (0 = disc only).
    pub ring_width: f32,
    pub fill: Color,
    pub edge: Color,
}

#[derive(Debug, Clone)]
pub enum Layer {
    Screen(ScreenLayer),
    Camera(CameraLayer),
    Sprite(SpriteLayer),
    Solid(SolidLayer),
    Mask(MaskLayer),
    Ring(RingLayer),
}

/// Everything needed to draw one output frame.
#[derive(Debug, Clone)]
pub struct FrameDesc {
    pub width: u32,
    pub height: u32,
    pub background: BackgroundLayer,
    pub layers: Vec<Layer>,
}

pub use gpu::Compositor;
