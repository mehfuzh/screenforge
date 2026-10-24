//! An opened project ready for playback/export.
//!
//! [`Session`] owns the decoded screen recording and webcam ([`SourceStream`]s: sequential
//! ffmpeg reader + last frame), the cursor track and sprites, the animated camera, and
//! produces a [`FrameDesc`] for any output time. Frames are produced most efficiently in
//! increasing time order; seeking backwards or far forwards re-opens the decoders.
//!
//! Geometry follows Screen Studio's stage model (see `sf_engine::stage`): everything is laid
//! out in stage units (height 1000) and scaled to the output size at the end.

pub mod assets;
pub mod captions;
pub mod create;
pub mod cursor_sets;
pub mod preview;
pub mod face;
pub mod layout;
pub mod shortcuts;
pub mod source;
pub mod stream;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use sf_engine::camera::CameraSolver;
use sf_engine::cursor::{CursorSmoother, CursorTrack};
use sf_engine::spring::{SpringParams, SpringVec};
use sf_engine::stage::{OutputSize, StageGeometry};
use sf_engine::timeline::Timeline;
use sf_media::Frame;
use sf_project::schema::common::Rect;
use sf_project::v4::{ClickEffect, Mask};
use sf_project::ProjectPackage;
use sf_render::{BackgroundLayer, CameraGrade, CameraLayer, Color, FrameDesc, Gpu, Layer, LutTexture, MaskLayer, RingLayer, ScreenLayer, ShadowLayer, SolidLayer, SpriteLayer, Texture};

use crate::layout::{CameraSlot, ResolvedLayout};
pub use source::SourceInfo;
pub use stream::SourceStream;

pub struct CursorSprite {
    pub texture: Arc<Texture>,
    /// Hotspot in sprite points.
    pub hotspot: [f64; 2],
    /// Logical size in points.
    pub size: [f64; 2],
}

/// Cursor hidden while typing if a key was pressed within this window and no mouse event since.
const TYPING_HIDE_MS: f64 = 1000.0;
/// How long a click effect stays visible after a mouse-down.
pub const CLICK_EFFECT_MS: f64 = 420.0;
/// At most this many overlapping click effects (Screen Studio: 3).
const MAX_CLICK_EFFECTS: usize = 3;
/// `sensitive-data` masks blend the blurred pixels toward the surrounding colour by this much.
const MASK_COVER_MIX: f32 = 0.5;

pub struct Session {
    pub pkg: ProjectPackage,
    pub source: SourceInfo,
    pub timeline: Timeline,
    pub cursor_track: CursorTrack,
    pub assets: assets::Assets,
    pub geometry: StageGeometry,
    gpu: Gpu,
    screen: SourceStream,
    camera_stream: Option<SourceStream>,
    cursor_sprites: HashMap<String, Arc<CursorSprite>>,
    /// Sprites of the drawn cursor sets, uploaded once per `(set, cursor id)`.
    set_sprites: HashMap<(String, String), Option<Arc<CursorSprite>>>,
    background_tex: Option<(String, Arc<Texture>)>,
    camera: CameraSolver,
    cursor_smoother: CursorSmoother,
    /// Animated webcam rect (stage units: x, y, w, h).
    camera_rect: SpringVec<4>,
    camera_rect_initialised: bool,
    /// Face tracking for the webcam window (`config.camera.enableFaceTracking`).
    pub face_tracker: face::FaceTracker,
    /// The camera LUT (`config.camera.lut`) as uploaded, keyed by name; `None` inside = failed to load.
    lut_tex: Option<(String, Option<Arc<LutTexture>>)>,
    last_output_ms: Option<f64>,
    /// Decode width cap for previews (None = native).
    pub decode_max_width: Option<u32>,
    /// Playback speed the preview is decoding for (1 = every frame). The editor sets the
    /// current clip's speed while playing; export leaves it at 1.
    pub decode_speed: f64,
    pub captions: captions::Captions,
    pub shortcuts: shortcuts::Shortcuts,
    /// Where the screen was drawn in the last frame (output px), for editing masks on the preview.
    pub last_screen_px: Option<Rect>,
}

impl Session {
    pub fn open(path: &Path, gpu: Gpu) -> Result<Self> {
        let pkg = ProjectPackage::open(path)?;
        let source = SourceInfo::from_package(&pkg).context("locating the screen recording")?;
        let scene = pkg.project.scenes.first().context("project has no scenes")?;
        let timeline = Timeline::from_scene(scene);
        let cursor_track = source.cursor_track(&pkg).unwrap_or_default();
        let screen = SourceStream::new(&gpu, source.media_path.clone(), "screen");
        let camera_stream = source.camera.first().map(|p| SourceStream::new(&gpu, p.clone(), "camera"));
        let geometry = Self::geometry_for(&pkg, &source);
        let (camera, cursor_smoother) = Self::solvers(&pkg, &source, &cursor_track, geometry);
        let camera_rect = SpringVec::new(SpringParams::from(pkg.project.config.animations.screen_movement_spring), [0.0; 4]);
        let captions = captions::Captions::load(&pkg, source.process_time_start_ms, &timeline);
        let shortcuts = shortcuts::Shortcuts::load(&pkg, &cursor_track);
        let mut s = Session {
            pkg,
            source,
            timeline,
            cursor_track,
            assets: assets::Assets::discover(),
            geometry,
            gpu,
            screen,
            camera_stream,
            cursor_sprites: HashMap::new(),
            set_sprites: HashMap::new(),
            background_tex: None,
            camera,
            cursor_smoother,
            camera_rect,
            camera_rect_initialised: false,
            face_tracker: face::FaceTracker::new(),
            lut_tex: None,
            last_output_ms: None,
            decode_max_width: None,
            decode_speed: 1.0,
            captions,
            shortcuts,
            last_screen_px: None,
        };
        s.load_cursor_sprites();
        Ok(s)
    }

    fn geometry_for(pkg: &ProjectPackage, source: &SourceInfo) -> StageGeometry {
        let cfg = &pkg.project.config;
        let (cw, ch) = source.content_size_px(&cfg.crop.rect01);
        let aspect = cfg.output.aspect_ratio.as_f64().map(|r| format!("{r}:1")).unwrap_or_else(|| "auto".into());
        StageGeometry::new(&aspect, cw as f64 / ch.max(1) as f64, cfg.output.padding_ratio01, cfg.output.avoid_empty_zoom_area)
    }

    fn solvers(pkg: &ProjectPackage, source: &SourceInfo, track: &CursorTrack, geometry: StageGeometry) -> (CameraSolver, CursorSmoother) {
        let cfg = &pkg.project.config;
        let crop = cfg.crop.rect01;
        let (bw, bh) = (source.bounds.width, source.bounds.height);
        let crop_px = Rect { x: crop.x * bw, y: crop.y * bh, width: crop.width * bw, height: crop.height * bh };
        let camera = CameraSolver::new(&pkg.project.scenes[0].zooms, track, crop_px, geometry, SpringParams::from(cfg.animations.screen_movement_spring), cfg.zooms.start_animation_early);
        let cursor = CursorSmoother::new(SpringParams::from(cfg.animations.mouse_movement_spring));
        (camera, cursor)
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The package directory was renamed/moved: refresh every path derived from it.
    pub fn relocate(&mut self, new_path: &Path) -> Result<()> {
        self.pkg.path = new_path.to_path_buf();
        self.source = SourceInfo::from_package(&self.pkg)?;
        let mut screen = SourceStream::new(&self.gpu, self.source.media_path.clone(), "screen");
        screen.max_width = self.decode_max_width;
        self.screen = screen;
        self.camera_stream = self.source.camera.first().map(|p| SourceStream::new(&self.gpu, p.clone(), "camera"));
        self.last_output_ms = None;
        Ok(())
    }

    /// Whether a webcam channel is available (recorded or `SF_DEBUG_CAMERA`).
    pub fn has_camera(&self) -> bool {
        self.camera_stream.is_some()
    }

    /// Call after editing the project (slices/zooms/config) so derived state is rebuilt.
    pub fn rebuild(&mut self) {
        let scene = &self.pkg.project.scenes[0];
        self.timeline = Timeline::from_scene(scene);
        self.geometry = Self::geometry_for(&self.pkg, &self.source);
        let (camera, cursor) = Self::solvers(&self.pkg, &self.source, &self.cursor_track, self.geometry);
        self.camera = camera;
        self.cursor_smoother = cursor;
        self.camera_rect.set_params(SpringParams::from(self.pkg.project.config.animations.screen_movement_spring));
        self.camera_rect_initialised = false;
        self.last_output_ms = None;
        self.reload_captions();
        self.shortcuts = shortcuts::Shortcuts::load(&self.pkg, &self.cursor_track);
    }

    pub fn duration_ms(&self) -> f64 {
        self.timeline.duration_ms()
    }

    /// Path of the project's background audio track, if any (project `assets/` first, then
    /// the bundled library).
    pub fn background_audio_path(&self) -> Option<std::path::PathBuf> {
        self.music_path(self.pkg.project.config.audio.background_audio_file_name.as_deref()?)
    }

    /// File of a music track by its project name: a file added to the project's `assets/`,
    /// or a track of the bundled library (`<category>/<file>`).
    pub fn music_path(&self, name: &str) -> Option<std::path::PathBuf> {
        let local = self.pkg.path.join("assets").join(name.rsplit('/').next().unwrap_or(name));
        if local.is_file() {
            return Some(local);
        }
        self.assets.background_audio(name)
    }

    /// Audio file of a voice-over (`<project>/voiceovers/<file_name>`).
    pub fn voice_over_path(&self, file_name: &str) -> std::path::PathBuf {
        self.pkg.path.join(sf_project::v4::VoiceOver::DIR).join(file_name)
    }

    /// Voice-overs of the scene on the output timeline (see `sf_engine::audio::voice_over_spans`).
    pub fn voice_over_spans(&self) -> Vec<sf_engine::audio::VoiceOverSpan> {
        sf_engine::audio::voice_over_spans(&self.pkg.project.scenes[0], &self.timeline)
    }

    /// Resolved zooms (source time) for timeline display.
    pub fn timeline_zooms(&self) -> &[sf_engine::camera::ResolvedZoom] {
        self.camera.zooms()
    }

    /// Current camera zoom for the preview indicator: `(level, visible01)` where `visible01` is
    /// the part of the screen inside the stage as fractions `[x, y, w, h]`. Level 1 = no zoom.
    pub fn zoom_state(&self) -> (f64, [f64; 4]) {
        let geo = self.camera.geometry();
        let cur = self.camera.current();
        let level = cur.width / geo.initial_screen.width.max(1e-6);
        let area = geo.stage;
        let x0 = ((area.x - cur.x) / cur.width.max(1e-6)).clamp(0.0, 1.0);
        let y0 = ((area.y - cur.y) / cur.height.max(1e-6)).clamp(0.0, 1.0);
        let x1 = ((area.x + area.width - cur.x) / cur.width.max(1e-6)).clamp(0.0, 1.0);
        let y1 = ((area.y + area.height - cur.y) / cur.height.max(1e-6)).clamp(0.0, 1.0);
        (level, [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)])
    }

    /// Generate automatic zooms from clicks (and keystrokes when `zooms.zoom_on_typing` is on),
    /// like Screen Studio's "create automatic zooms".
    pub fn auto_zooms(&self) -> Vec<sf_project::v4::Zoom> {
        let typing = self.pkg.project.config.zooms.zoom_on_typing();
        sf_engine::autozoom::generate_with(&self.cursor_track, self.source.duration_ms, sf_project::v4::Zoom::DEFAULT_LEVEL, typing)
    }

    /// The camera layout in effect at a source time.
    pub fn layout_at(&self, source_ms: f64) -> ResolvedLayout {
        layout::resolve(&self.pkg.project.config.default_layout, &self.pkg.project.scenes[0].layouts, source_ms)
    }

    fn load_cursor_sprites(&mut self) {
        let dir = self.pkg.recording_dir();
        let Ok(infos) = sf_project::recording::load_cursors(&dir) else { return };
        for c in infos {
            let png = dir.join("cursors").join(format!("{}.png", c.id));
            if let Ok(tex) = Texture::from_image_path(&self.gpu.device, &self.gpu.queue, &png) {
                self.cursor_sprites.insert(
                    c.id.clone(),
                    Arc::new(CursorSprite { texture: tex, hotspot: [c.hot_spot.x, c.hot_spot.y], size: [c.standard_size.width, c.standard_size.height] }),
                );
            }
        }
    }

    /// The sprite to draw for a cursor id under `config.cursor.set`: the set's own drawing when
    /// it has one, else the recorded sprite for that id, else the set's arrow, else the
    /// recorded arrow. Drawn sprites are rasterised and uploaded on first use.
    fn cursor_sprite(&mut self, set: &str, id: &str) -> Option<Arc<CursorSprite>> {
        if set != cursor_sets::RECORDED {
            if let Some(sp) = self.set_sprite(set, id) {
                return Some(sp);
            }
        }
        if let Some(sp) = self.cursor_sprites.get(id) {
            return Some(sp.clone());
        }
        if set != cursor_sets::RECORDED {
            if let Some(sp) = self.set_sprite(set, "arrow") {
                return Some(sp);
            }
        }
        self.cursor_sprites.get("arrow").cloned()
    }

    fn set_sprite(&mut self, set: &str, id: &str) -> Option<Arc<CursorSprite>> {
        let key = (set.to_string(), id.to_string());
        if let Some(sp) = self.set_sprites.get(&key) {
            return sp.clone();
        }
        let sp = cursor_sets::draw(set, id).map(|mut s| {
            sf_render::texture::premultiply(&mut s.rgba);
            let tex = Texture::from_rgba(&self.gpu.device, &self.gpu.queue, s.width, s.height, &s.rgba, &format!("cursor {set}/{id}"));
            Arc::new(CursorSprite { texture: tex, hotspot: s.hotspot, size: s.size })
        });
        self.set_sprites.insert(key, sp.clone());
        sp
    }

    /// Output canvas size for the project at a given height.
    pub fn output_size(&self, height: u32) -> OutputSize {
        self.geometry.output_size(height)
    }

    // ───────────── decoding ─────────────

    fn seek_sources(&mut self, source_ms: f64, want_camera: bool) -> Result<()> {
        let fps = stream::DEFAULT_FPS / self.decode_speed.max(1.0);
        self.screen.max_width = self.decode_max_width;
        self.screen.fps = fps;
        self.screen.seek(&self.gpu, source_ms)?;
        if want_camera {
            if let Some(cam) = self.camera_stream.as_mut() {
                cam.max_width = self.decode_max_width;
                cam.fps = fps;
                let t = source_ms + self.source.camera_time_offset_ms;
                if let Err(e) = cam.seek(&self.gpu, t) {
                    // a broken webcam file must not take the whole frame down
                    tracing::warn!("camera decode failed at {t:.0} ms: {e:#}");
                    cam.close();
                }
            }
        }
        Ok(())
    }

    // ───────────── composition ─────────────

    fn background_layer(&mut self) -> BackgroundLayer {
        let bg = &self.pkg.project.config.styles.background;
        match bg.kind.as_str() {
            "color" => BackgroundLayer::Color(Color::from_hex(&bg.color).unwrap_or(Color::BLACK)),
            "gradient" => gradient_layer(&bg.gradient),
            "system" | "image" | "unsplash" => {
                let key = match bg.kind.as_str() {
                    "system" => bg.system_name.clone().map(|n| ("system", n)),
                    "unsplash" => bg.unsplash.as_ref().and_then(|v| v.get("image")).and_then(|v| v.as_str()).map(|s| ("image", format!("assets/{s}"))),
                    _ => bg.image.as_ref().and_then(|v| v.as_str()).map(|s| ("image", if s.contains('/') { s.to_string() } else { format!("assets/{s}") })),
                };
                if let Some((kind, name)) = key {
                    let cache_key = format!("{kind}:{name}");
                    if self.background_tex.as_ref().map(|(k, _)| k != &cache_key).unwrap_or(true) {
                        let path = if kind == "system" { self.assets.background(&name) } else { Some(self.pkg.path.join(&name)) };
                        let tex = path.and_then(|p| Texture::from_image_path(&self.gpu.device, &self.gpu.queue, &p).ok());
                        self.background_tex = tex.map(|t| (cache_key.clone(), t));
                    }
                    if let Some((_, t)) = &self.background_tex {
                        return BackgroundLayer::Image { texture: t.clone(), blur: bg.blur as f32 };
                    }
                }
                gradient_layer(&bg.gradient)
            }
            _ => gradient_layer(&bg.gradient),
        }
    }

    /// Build the frame for output time `output_ms`. `out` is the canvas size.
    pub fn frame_desc_at(&mut self, output_ms: f64, out: OutputSize) -> Result<FrameDesc> {
        let output_ms = output_ms.clamp(0.0, self.duration_ms().max(0.0));
        let source_ms = self.timeline.output_to_source(output_ms).unwrap_or(0.0);
        let span = self.timeline.span_at_output(output_ms).cloned();
        let speed = span.map(|s| 1.0 / s.time_scale).unwrap_or(1.0);
        let jump = match self.last_output_ms {
            Some(prev) if output_ms >= prev && output_ms - prev < 500.0 => false,
            _ => true,
        };
        let crossed_cut = jump || self.timeline.is_cut_at_output(output_ms, 1.0);
        // springs run in source time so speed-ups don't change their feel
        let dt = if jump { 0.0 } else { (output_ms - self.last_output_ms.unwrap_or(output_ms)) * speed };
        self.last_output_ms = Some(output_ms);

        let cfg = self.pkg.project.config.clone();
        let layout = self.layout_at(source_ms);
        let camera_wanted = self.camera_stream.is_some() && !cfg.camera.hide && layout.is_camera_visible();
        let frame = layout::layout_frame(&layout, &self.geometry, cfg.camera.aspect_ratio);
        let screen_visible = frame.screen_visible || !camera_wanted;

        self.seek_sources(source_ms, camera_wanted)?;

        let slice = span.map(|s| self.pkg.project.scenes[0].slices[s.index].clone());
        let crop = cfg.crop.rect01;
        let bounds = self.source.bounds;
        let s = out.scale(); // px per stage unit
        let geo = frame.screen_geometry;
        self.camera.set_geometry(geo);

        // ── cursor (crop points → normalized) ──
        let smoothing = slice.as_ref().map(|sl| !sl.disable_smooth_mouse_movement).unwrap_or(true);
        if !smoothing {
            self.cursor_smoother.set_params(SpringParams::INSTANT);
        } else {
            self.cursor_smoother.set_params(SpringParams::from(cfg.animations.mouse_movement_spring));
        }
        if crossed_cut {
            if let Some(r) = self.cursor_track.raw_at(source_ms) {
                self.cursor_smoother.reset([r.x, r.y], source_ms);
            }
        }
        let tilt_enabled = cfg.cursor.rotation_mode == "velocity";
        let cursor = self.cursor_smoother.step(&self.cursor_track, source_ms, dt, cfg.cursor.rotate_on_x_movement_ratio, cfg.cursor.base_rotation, tilt_enabled);
        let to_crop01 = |p: [f64; 2]| [(p[0] / bounds.width - crop.x) / crop.width, (p[1] / bounds.height - crop.y) / crop.height];
        let cursor01 = cursor.map(|(p, _, _)| to_crop01(p));

        // ── screen rect (stage units) ──
        let resting = cursor01.unwrap_or([0.5, 0.5]);
        if crossed_cut {
            self.camera.reset(source_ms, resting);
        }
        let rect = self.camera.step(source_ms, resting, dt);
        let zoomed = self.camera.active_zoom(source_ms).is_some();
        let zoom_factor = rect.width / geo.initial_screen.width.max(1e-6);
        let screen_px = Rect { x: rect.x * s, y: rect.y * s, width: rect.width * s, height: rect.height * s };

        let mut layers = Vec::new();
        let sh = &cfg.styles.shadow;
        let shadow = ShadowLayer::from_style(sh.intensity as f32, (sh.distance * s) as f32, sh.angle as f32, (sh.blur * s) as f32, sh.is_directional);
        let inset = (cfg.styles.screen_inset.size > 0.0).then(|| {
            let i = &cfg.styles.screen_inset;
            ((i.size * s) as f32, Color::from_hex(&i.color).unwrap_or(Color::BLACK).with_alpha(i.alpha as f32))
        });
        let mut corner_radius = (cfg.styles.screen_border_radius * zoom_factor * s) as f32;
        // a device frame fills the screen's rect and casts its shadow; the recording sits
        // inside its bezel (the cursor, masks and clicks follow it there)
        let device = cfg.device.frame_key.as_deref().filter(|_| cfg.device.enable_mockup && screen_visible).and_then(sf_engine::device::Frame::parse).map(|f| sf_engine::device::layout(f, screen_px));
        let (screen_px, shadow, inset) = match &device {
            Some(d) => {
                // the shadow goes under the body (its first full-size shape), not the buttons
                let body = d.under.iter().position(|p| p.rect == d.body);
                for (i, part) in d.under.iter().enumerate() {
                    let cast = if Some(i) == body { shadow } else { None };
                    layers.push(Layer::Solid(SolidLayer { dst: part.rect, color: Color::from_hex(part.color).unwrap_or(Color::BLACK), corner_radius: part.radius as f32, shadow: cast }));
                }
                corner_radius = d.screen_radius as f32;
                (d.screen, None, None)
            }
            None => (screen_px, shadow, inset),
        };
        self.last_screen_px = screen_visible.then_some(screen_px);
        if screen_visible {
            layers.push(Layer::Screen(ScreenLayer { texture: self.screen.texture.clone(), src01: crop, dst: screen_px, corner_radius, shadow, inset }));
            self.push_masks(&mut layers, source_ms, screen_px, corner_radius);
            self.push_click_effects(&mut layers, source_ms, screen_px, zoom_factor * s, &cfg.cursor.click_effect, to_crop01);
            for part in device.iter().flat_map(|d| &d.over) {
                layers.push(Layer::Solid(SolidLayer { dst: part.rect, color: Color::from_hex(part.color).unwrap_or(Color::BLACK), corner_radius: part.radius as f32, shadow: None }));
            }
        }

        // ── webcam ──
        let camera_target = match frame.camera {
            CameraSlot::None => None,
            CameraSlot::Fixed(r) => Some(r),
            slot @ CameraSlot::Overlay { .. } => layout::overlay_rect(&slot, &geo, rect, zoomed),
        };
        if let (true, Some(target)) = (camera_wanted, camera_target) {
            let t = [target.x, target.y, target.width, target.height];
            let cam = if !self.camera_rect_initialised || crossed_cut {
                self.camera_rect.snap(t);
                self.camera_rect_initialised = true;
                t
            } else {
                self.camera_rect.step(t, dt)
            };
            let dst = Rect { x: cam[0] * s, y: cam[1] * s, width: cam[2] * s, height: cam[3] * s };
            let lut = self.camera_lut(cfg.camera.lut.as_deref());
            if let Some(stream) = &self.camera_stream {
                let tex = stream.texture.clone();
                let crop01 = cfg.camera.crop01.unwrap_or(Rect::UNIT);
                let dst_aspect = dst.width / dst.height.max(1e-6);
                let tex_aspect = tex.width as f64 / tex.height.max(1) as f64;
                let src01 = if cfg.camera.enable_face_tracking {
                    if let Some(f) = stream.frame() {
                        self.face_tracker.observe(&f.data, f.width, f.height, f.format == sf_media::PixelFormat::Bgra8, f.pts_ms, source_ms);
                    }
                    self.face_tracker.window(source_ms, dt, crossed_cut, crop01, dst_aspect, tex_aspect, cfg.camera.face_tracking_zoom())
                } else {
                    let src_aspect = (tex.width as f64 * crop01.width) / (tex.height as f64 * crop01.height).max(1e-6);
                    layout::cover_crop(crop01, src_aspect, dst_aspect)
                };
                let fullscreen = matches!(frame.camera, CameraSlot::Fixed(r) if r == geo.stage);
                let radius = if fullscreen { 0.0 } else { (cfg.styles.camera_border_radius_ratio01.clamp(0.0, 0.5) * dst.width.min(dst.height)) as f32 };
                // grain changes pattern per camera frame (30 fps), not per output frame
                let seed = ((source_ms / (1000.0 / 30.0)).floor() % 997.0) as f32;
                let grade = CameraGrade::from_config(&cfg.camera.grade(), if lut.is_some() { cfg.camera.lut_intensity } else { 0.0 }, cfg.camera.grain, cfg.camera.sharpen01, seed);
                let grade = (!grade.is_neutral()).then_some(grade);
                layers.push(Layer::Camera(CameraLayer { texture: tex, src01, dst, corner_radius: radius, mirror: cfg.camera.mirror, shadow: if fullscreen { None } else { shadow }, grade, lut }));
            }
        } else {
            self.camera_rect_initialised = false;
        }

        // ── cursor sprite ──
        let hide_cursor = !screen_visible || cfg.cursor.hide || slice.as_ref().map(|sl| sl.hide_cursor).unwrap_or(false);
        let idle_hidden = cfg.cursor.hide_not_moving_after_ms.map(|ms| !self.cursor_track.moved_within(source_ms, ms)).unwrap_or(false);
        let typing_hidden = self.cursor_track.is_typing(source_ms, TYPING_HIDE_MS);
        if let (false, Some(c01), Some((_, pressed_scale, tilt_deg)), Some(raw)) = (hide_cursor || idle_hidden || typing_hidden, cursor01, cursor, self.cursor_track.raw_at(source_ms)) {
            let id = if cfg.cursor.always_use_default { "arrow".to_string() } else { self.cursor_track.cursor_ids.get(raw.cursor_id_index).cloned().unwrap_or_else(|| "arrow".into()) };
            let set = cfg.cursor.set.id.clone();
            let sprite = self.cursor_sprite(&set, &id);
            if let Some(sp) = sprite {
                let px = screen_px.x + c01[0] * screen_px.width;
                let py = screen_px.y + c01[1] * screen_px.height;
                // `cursor.size` is the sprite height in stage units at zoom 1; it grows with the zoom
                let scale = cfg.cursor.size * s * zoom_factor * pressed_scale / sp.size[1].max(1.0);
                let w = sp.size[0] * scale;
                let h = sp.size[1] * scale;
                let dst = Rect { x: px - sp.hotspot[0] * scale, y: py - sp.hotspot[1] * scale, width: w, height: h };
                layers.push(Layer::Sprite(SpriteLayer { texture: sp.texture.clone(), dst, pivot: [px as f32, py as f32], rotation_rad: tilt_deg.to_radians() as f32, opacity: 1.0 }));
            }
        }

        // ── captions and shortcut labels, above everything ──
        self.captions.push_layer(&self.pkg, &self.gpu, source_ms, out.width, out.height, &mut layers);
        self.shortcuts.push_layer(&self.pkg, &self.gpu, source_ms, out.width, out.height, &mut layers);

        Ok(FrameDesc { width: out.width, height: out.height, background: self.background_layer(), layers })
    }

    /// The camera LUT named by `config.camera.lut`: the project's `assets/` first (imported
    /// `.cube` files), then the bundled looks (`assets/luts/`). Uploaded once per name.
    fn camera_lut(&mut self, name: Option<&str>) -> Option<Arc<LutTexture>> {
        let name = name?.trim();
        if name.is_empty() {
            return None;
        }
        if let Some((n, t)) = &self.lut_tex {
            if n == name {
                return t.clone();
            }
        }
        let local = self.pkg.path.join("assets").join(name.rsplit('/').next().unwrap_or(name));
        let path = if local.is_file() { Some(local) } else { self.assets.lut(name) };
        let tex = match path {
            Some(p) => match sf_render::CubeLut::load(&p) {
                Ok(lut) => Some(LutTexture::upload(&self.gpu.device, &self.gpu.queue, &lut, &p.display().to_string())),
                Err(e) => {
                    tracing::warn!("camera LUT {name:?}: {e:#}");
                    None
                }
            },
            None => {
                tracing::warn!("camera LUT {name:?} not found");
                None
            }
        };
        self.lut_tex = Some((name.to_string(), tex.clone()));
        tex
    }

    /// Canvas for the whole, uncropped recording at a given height (the crop editor's picture).
    pub fn source_size(&self, height: u32) -> OutputSize {
        let aspect = self.source.native_width.max(1) as f64 / self.source.native_height.max(1) as f64;
        OutputSize { width: ((height as f64 * aspect).round() as u32).max(2) & !1, height }
    }

    /// The uncropped recording at `output_ms` filling `out`, with nothing drawn over it: what the
    /// crop editor shows. The animated camera and cursor restart on the next regular frame.
    pub fn source_frame_desc_at(&mut self, output_ms: f64, out: OutputSize) -> Result<FrameDesc> {
        let output_ms = output_ms.clamp(0.0, self.duration_ms().max(0.0));
        let source_ms = self.timeline.output_to_source(output_ms).unwrap_or(0.0);
        self.seek_sources(source_ms, false)?;
        self.last_output_ms = None;
        let dst = Rect { x: 0.0, y: 0.0, width: out.width as f64, height: out.height as f64 };
        let screen = ScreenLayer { texture: self.screen.texture.clone(), src01: Rect::UNIT, dst, corner_radius: 0.0, shadow: None, inset: None };
        Ok(FrameDesc { width: out.width, height: out.height, background: BackgroundLayer::Color(Color::BLACK), layers: vec![Layer::Screen(screen)] })
    }

    /// Re-read the transcripts (after generating one, or toggling sound labels).
    pub fn reload_captions(&mut self) {
        self.captions = captions::Captions::load(&self.pkg, self.source.process_time_start_ms, &self.timeline);
    }

    /// SubRip captions for the edited timeline (empty when there is no transcript).
    pub fn captions_srt(&self) -> String {
        self.captions.to_srt(&self.timeline)
    }

    /// WebVTT captions for the edited timeline (the same cues as [`Self::captions_srt`]).
    pub fn captions_vtt(&self) -> String {
        self.captions.to_vtt(&self.timeline)
    }

    /// Masks active at `source_ms`, drawn over the screen. Bounds are pixel rects in
    /// cropped-recording space.
    fn push_masks(&self, layers: &mut Vec<Layer>, source_ms: f64, screen_px: Rect, corner_radius: f32) {
        let crop = self.pkg.project.config.crop.rect01;
        let (nw, nh) = (self.source.native_width.max(1) as f64, self.source.native_height.max(1) as f64);
        let (cw, ch) = ((nw * crop.width).max(1.0), (nh * crop.height).max(1.0));
        let to_out = |b: &Rect| Rect { x: screen_px.x + b.x / cw * screen_px.width, y: screen_px.y + b.y / ch * screen_px.height, width: b.width / cw * screen_px.width, height: b.height / ch * screen_px.height };
        let tex = &self.screen.texture;
        let tex_scale = tex.width as f64 / nw;
        for m in &self.pkg.project.scenes[0].masks {
            if m.is_disabled || source_ms < m.source_start_ms || source_ms >= m.source_end_ms || m.bounds.is_empty() {
                continue;
            }
            match m.kind.as_str() {
                Mask::HIGHLIGHT => {
                    let holes: Vec<Rect> = m.bounds.iter().map(to_out).collect();
                    layers.push(Layer::Mask(MaskLayer::Highlight { dst: screen_px, corner_radius, opacity: m.highlight_mask_opacity.clamp(0.0, 1.0) as f32, holes }));
                }
                _ => {
                    for b in &m.bounds {
                        let dst = to_out(b);
                        if dst.width <= 0.0 || dst.height <= 0.0 {
                            continue;
                        }
                        // region in texture space (normalized) and in texture pixels
                        let src01 = Rect { x: crop.x + b.x / nw, y: crop.y + b.y / nh, width: b.width / nw, height: b.height / nh };
                        let tex_rect = Rect { x: src01.x * tex.width as f64, y: src01.y * tex.height as f64, width: src01.width * tex.width as f64, height: src01.height * tex.height as f64 };
                        let cover = self.screen.frame().map(|f| average_border_color(f, tex_rect)).unwrap_or(Color::BLACK);
                        let radius_px = (m.blur.max(0.0) * tex_scale) as f32;
                        let corner = (8.0 * screen_px.width / cw).min(dst.width.min(dst.height) / 2.0) as f32;
                        layers.push(Layer::Mask(MaskLayer::Blur { texture: tex.clone(), src01, clip01: crop, dst, corner_radius: corner, radius_px, cover, cover_mix: MASK_COVER_MIX }));
                    }
                }
            }
        }
    }

    /// Expanding disc/ring at each mouse-down of the last [`CLICK_EFFECT_MS`].
    fn push_click_effects(&self, layers: &mut Vec<Layer>, source_ms: f64, screen_px: Rect, px_per_unit: f64, effect: &Option<ClickEffect>, to_crop01: impl Fn([f64; 2]) -> [f64; 2]) {
        let Some(effect) = effect else { return };
        let cursor_px = self.pkg.project.config.cursor.size * px_per_unit;
        let recent = self.cursor_track.clicks.iter().rev().filter(|c| c.down && c.time_ms <= source_ms && source_ms - c.time_ms < CLICK_EFFECT_MS).take(MAX_CLICK_EFFECTS);
        for c in recent {
            let t = ((source_ms - c.time_ms) / CLICK_EFFECT_MS).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - t) * (1.0 - t);
            let p01 = to_crop01([c.x, c.y]);
            let center = [(screen_px.x + p01[0] * screen_px.width) as f32, (screen_px.y + p01[1] * screen_px.height) as f32];
            let fade = (1.0 - t) as f32;
            let layer = match effect {
                // Subtle press feedback: a small soft disc that grows a little and fades out.
                ClickEffect::Circle => RingLayer {
                    center,
                    radius: (cursor_px * (0.32 + 0.22 * ease)) as f32,
                    ring_width: (cursor_px * 0.025).clamp(1.0, 2.5) as f32,
                    fill: Color::WHITE.with_alpha(0.16 * fade),
                    edge: Color::WHITE.with_alpha(0.45 * fade * fade),
                },
                // Ripple is a UV displacement wave in Screen Studio; approximated by a travelling ring.
                ClickEffect::Ripple { pull_size01 } => {
                    let reach = cursor_px * (0.8 + pull_size01.unwrap_or(0.04) * 15.0);
                    RingLayer { center, radius: (reach * ease) as f32, ring_width: (cursor_px * 0.06).clamp(1.5, 4.0) as f32, fill: Color::TRANSPARENT, edge: Color::WHITE.with_alpha(0.4 * fade * fade) }
                }
            };
            layers.push(Layer::Ring(layer));
        }
    }
}

/// Average colour of a one-pixel-ish ring just outside `rect` (texture pixels) in a decoded
/// RGBA frame: the "cover" colour for sensitive-data masks.
fn average_border_color(frame: &Frame, rect: Rect) -> Color {
    let (w, h) = (frame.width as i64, frame.height as i64);
    if w == 0 || h == 0 || frame.data.len() < (w * h * 4) as usize {
        return Color::BLACK;
    }
    let x0 = rect.x.floor() as i64 - 2;
    let y0 = rect.y.floor() as i64 - 2;
    let x1 = (rect.x + rect.width).ceil() as i64 + 2;
    let y1 = (rect.y + rect.height).ceil() as i64 + 2;
    let mut acc = [0u64; 3];
    let mut n = 0u64;
    let mut add = |x: i64, y: i64| {
        let (x, y) = (x.clamp(0, w - 1), y.clamp(0, h - 1));
        let i = ((y * w + x) * 4) as usize;
        acc[0] += frame.data[i] as u64;
        acc[1] += frame.data[i + 1] as u64;
        acc[2] += frame.data[i + 2] as u64;
        n += 1;
    };
    let steps = 24;
    for k in 0..=steps {
        let fx = x0 + (x1 - x0) * k / steps;
        let fy = y0 + (y1 - y0) * k / steps;
        add(fx, y0);
        add(fx, y1);
        add(x0, fy);
        add(x1, fy);
    }
    if n == 0 {
        return Color::BLACK;
    }
    Color { r: acc[0] as f32 / (255.0 * n as f32), g: acc[1] as f32 / (255.0 * n as f32), b: acc[2] as f32 / (255.0 * n as f32), a: 1.0 }
}

fn gradient_layer(g: &sf_project::schema::common::Gradient) -> BackgroundLayer {
    BackgroundLayer::Gradient {
        start: [g.start.x as f32, g.start.y as f32],
        end: [g.end.x as f32, g.end.y as f32],
        stops: g.stops.iter().map(|s| (Color::from_hex(&s.color).unwrap_or(Color::BLACK), s.at as f32)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_average_samples_outside_the_rect() {
        // 8×8 frame: inside 2..6 is white, the border is pure blue
        let mut data = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let i = (y * 8 + x) * 4;
                let inside = (2..6).contains(&x) && (2..6).contains(&y);
                data[i..i + 4].copy_from_slice(if inside { &[255, 255, 255, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let frame = Frame { width: 8, height: 8, format: sf_media::PixelFormat::Rgba8, pts_ms: 0.0, data: data.into() };
        let c = average_border_color(&frame, Rect { x: 2.0, y: 2.0, width: 4.0, height: 4.0 });
        assert!(c.r < 0.01 && c.g < 0.01 && c.b > 0.99, "got {c:?}");
    }
}
