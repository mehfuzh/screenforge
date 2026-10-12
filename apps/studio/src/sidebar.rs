//! Right-hand inspector: project configuration grouped like Screen Studio's sidebar.

use egui::RichText;

use sf_project::schema::common::Point;

use crate::editor::{fmt_ms, Editor};
use crate::theme;
use crate::widgets;

/// Slider over an f64 field of the project; applies the edit (with undo) when released.
fn slider(ed: &mut Editor, ui: &mut egui::Ui, label: &str, get: impl Fn(&sf_project::Project) -> f64, set: impl Fn(&mut sf_project::Project, f64) + 'static, range: std::ops::RangeInclusive<f64>, suffix: &str) {
    let mut v = get(&ed.session.pkg.project);
    let resp = ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(egui::Slider::new(&mut v, range).suffix(suffix)))
    });
    let r = resp.inner.inner;
    if r.changed() {
        // live preview without pushing undo for every pixel of drag
        set(&mut ed.session.pkg.project, v);
        ed.session.rebuild();
        ed.invalidate_preview();
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        let p = ed.session.pkg.project.clone();
        ed.undo.push(p);
    }
}

fn color_edit(ui: &mut egui::Ui, label: &str, hex: &mut String) -> bool {
    let mut c = sf_render::Color::from_hex(hex).unwrap_or(sf_render::Color::BLACK);
    let mut rgb = [c.r, c.g, c.b];
    let changed = ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.color_edit_button_rgb(&mut rgb).changed())
    });
    if changed.inner.inner {
        c.r = rgb[0];
        c.g = rgb[1];
        c.b = rgb[2];
        *hex = format!("#{:02x}{:02x}{:02x}", (c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8);
        true
    } else {
        false
    }
}

/// Inspector for the selected zoom item (`Editor::selected_zoom`).
pub fn zoom_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(id) = ed.selected_zoom.clone() else { return };
    let Some(z) = ed.session.pkg.project.scenes[0].zooms.iter().find(|z| z.id == id).cloned() else {
        ed.selected_zoom = None;
        return;
    };
    widgets::title(ui, "Zoom");
    (|ui: &mut egui::Ui| {
        let out0 = ed.session.timeline.source_to_output(z.source_start_ms);
        let out1 = ed.session.timeline.source_to_output(z.source_end_ms - 0.001);
        ui.label(RichText::new(format!("Source {} – {} ({:.2}s)", fmt_ms(z.source_start_ms), fmt_ms(z.source_end_ms), z.duration_ms() / 1000.0)).small().color(theme::p().text_dim));
        if let (Some(a), Some(b)) = (out0, out1) {
            ui.label(RichText::new(format!("Timeline {} – {}", fmt_ms(a), fmt_ms(b))).small().color(theme::p().text_dim));
        } else {
            ui.label(RichText::new("Partly inside a cut region").small().color(theme::p().text_dim));
        }
        {
            let gid = id.clone();
            let sid = id.clone();
            slider(ed, ui, "Level", move |p| p.scenes[0].zooms.iter().find(|z| z.id == gid).map(|z| z.zoom).unwrap_or(1.0), move |p, v| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == sid) { z.zoom = v } }, 1.0..=4.0, "×");
        }
        ui.horizontal(|ui| {
            ui.label("Type");
            let auto = z.is_auto();
            if ui.selectable_label(auto, "Auto").clicked() && !auto {
                let zid = id.clone();
                ed.edit(move |p| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == zid) { z.kind = sf_project::v4::zoom_type::FOLLOW_CLICK_GROUPS.into() } });
            }
            if ui.selectable_label(!auto, "Manual").clicked() && auto {
                let zid = id.clone();
                ed.edit(move |p| {
                    if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == zid) {
                        z.kind = sf_project::v4::zoom_type::MANUAL.into();
                        if z.manual_target_point.is_none() {
                            z.manual_target_point = Some(Point::new(0.5, 0.5));
                        }
                    }
                });
            }
        });
        if !z.is_auto() {
            let target = |p: &sf_project::Project, id: &str| p.scenes[0].zooms.iter().find(|z| z.id == id).and_then(|z| z.manual_target_point).unwrap_or(Point::new(0.5, 0.5));
            let (gid, sid) = (id.clone(), id.clone());
            slider(ed, ui, "Target X", move |p| target(p, &gid).x, move |p, v| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == sid) { let mut t = z.manual_target_point.unwrap_or(Point::new(0.5, 0.5)); t.x = v; z.manual_target_point = Some(t) } }, 0.0..=1.0, "");
            let (gid, sid) = (id.clone(), id.clone());
            slider(ed, ui, "Target Y", move |p| target(p, &gid).y, move |p, v| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == sid) { let mut t = z.manual_target_point.unwrap_or(Point::new(0.5, 0.5)); t.y = v; z.manual_target_point = Some(t) } }, 0.0..=1.0, "");
        }
        let mut disabled = z.is_disabled;
        if ui.checkbox(&mut disabled, "Disabled").changed() {
            let zid = id.clone();
            ed.edit(move |p| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == zid) { z.is_disabled = disabled } });
        }
        let mut instant = z.has_instant_animation;
        if ui.checkbox(&mut instant, "Instant animation").changed() {
            let zid = id.clone();
            ed.edit(move |p| { if let Some(z) = p.scenes[0].zooms.iter_mut().find(|z| z.id == zid) { z.has_instant_animation = instant } });
        }
        ui.horizontal(|ui| {
            ui.label("Start");
            ui.label(RichText::new(format!("{:.0} ms", z.source_start_ms)).monospace());
            ui.label("End");
            ui.label(RichText::new(format!("{:.0} ms", z.source_end_ms)).monospace());
        });
        if ui.button("Remove zoom").clicked() {
            ed.delete_selected();
        }
    })(ui);
}


/// Sidebar pages, selected from the icon rail (⌘1 … ⌘0 in Screen Studio).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Cursor,
    CameraLayout,
    CameraStyle,
    Captions,
    Audio,
    Shortcuts,
    Animations,
    DeviceFrame,
}

impl Section {
    pub const ALL: [Section; 9] = [Section::General, Section::Cursor, Section::CameraLayout, Section::CameraStyle, Section::Captions, Section::Audio, Section::Shortcuts, Section::Animations, Section::DeviceFrame];

    /// Pages whose feature is not rendered yet: shown greyed out in the rail.
    pub fn is_roadmap(self) -> bool {
        matches!(self, Section::Captions | Section::Shortcuts | Section::DeviceFrame)
    }

    pub fn icon(self) -> widgets::Icon {
        match self {
            Section::General => widgets::Icon::General,
            Section::Cursor => widgets::Icon::Cursor,
            Section::CameraLayout => widgets::Icon::CameraLayout,
            Section::CameraStyle => widgets::Icon::CameraStyle,
            Section::Captions => widgets::Icon::Captions,
            Section::Audio => widgets::Icon::Audio,
            Section::Shortcuts => widgets::Icon::Keyboard,
            Section::Animations => widgets::Icon::Animations,
            Section::DeviceFrame => widgets::Icon::DeviceFrame,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Cursor => "Cursor",
            Section::CameraLayout => "Camera Layout",
            Section::CameraStyle => "Camera Style",
            Section::Captions => "Captions",
            Section::Audio => "Audio",
            Section::Shortcuts => "Keyboard Shortcuts",
            Section::Animations => "Animations",
            Section::DeviceFrame => "Device frame",
        }
    }
}

fn cfg_slider(ed: &mut Editor, ui: &mut egui::Ui, label: &str, get: impl Fn(&sf_project::Project) -> f64, set: impl Fn(&mut sf_project::Project, f64), range: std::ops::RangeInclusive<f64>, default: Option<f64>, suffix: &str) {
    let mut v = get(&ed.session.pkg.project);
    let r = widgets::labeled_slider(ui, label, &mut v, range, default, suffix);
    if r.response.changed() || r.reset {
        set(&mut ed.session.pkg.project, v);
        ed.session.rebuild();
        ed.invalidate_preview();
        // volumes (microphone, system, background) take effect in playback immediately
        ed.refresh_audio_levels();
    }
    if r.response.drag_stopped() || r.reset || (r.response.changed() && !r.response.dragged()) {
        let p = ed.session.pkg.project.clone();
        ed.undo.push(p);
        ed.redo.clear();
    }
}

pub fn ui(ed: &mut Editor, ui: &mut egui::Ui, section: Section) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(8.0);
        if ed.selected_zoom.is_some() {
            zoom_item_ui(ed, ui);
            ui.add_space(12.0);
            ui.separator();
        }
        if ed.selected_audio.is_some() {
            audio_item_ui(ed, ui);
            ui.add_space(12.0);
            ui.separator();
        }
        match section {
            Section::General => general_ui(ed, ui),
            Section::Cursor => cursor_ui(ed, ui),
            Section::CameraLayout => camera_layout_ui(ed, ui),
            Section::CameraStyle => camera_style_ui(ed, ui),
            Section::Captions => captions_ui(ed, ui),
            Section::Audio => audio_ui(ed, ui),
            Section::Shortcuts => shortcuts_ui(ed, ui),
            Section::Animations => animations_ui(ed, ui),
            Section::DeviceFrame => device_ui(ed, ui),
        }
        ui.add_space(24.0);
    });
}

/// Built-in wallpaper categories, shown first; any other installed categories
/// (e.g. from a local Screen Studio) follow in this order, then the rest alphabetically.
const BUNDLED_CATEGORIES: [&str; 5] = ["macOS", "Aurora", "Dusk", "Neon", "Studio"];
const KNOWN_EXTRA_CATEGORIES: [&str; 8] = ["Spring", "Sunset", "Radiant", "Energy", "Iridescent", "Midnight", "Glassmorphism", "Raycast"];

fn wallpaper_categories(list: &[String]) -> Vec<String> {
    let present: Vec<String> = {
        let mut v: Vec<String> = list.iter().filter_map(|n| n.split_once('/').map(|(c, _)| c.to_string())).collect();
        v.sort();
        v.dedup();
        v
    };
    let mut out: Vec<String> = vec![];
    for c in BUNDLED_CATEGORIES.iter().chain(KNOWN_EXTRA_CATEGORIES.iter()) {
        if present.iter().any(|p| p == c) {
            out.push(c.to_string());
        }
    }
    for p in &present {
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

fn general_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Background");
    let kinds = ["system", "gradient", "color", "image", "unsplash"];
    let kind = ed.session.pkg.project.config.styles.background.kind.clone();
    let idx = kinds.iter().position(|k| *k == kind).unwrap_or(0);
    if let Some(i) = widgets::segmented_with_disabled(ui, idx, &["Wallpaper", "Gradient", "Color", "Image", "Unsplash"], &[4]) {
        let k = kinds[i].to_string();
        ed.edit(move |p| p.config.styles.background.kind = k);
    }
    ui.add_space(8.0);
    match kind.as_str() {
        "color" => {
            widgets::title(ui, "Color");
            let mut hex = ed.session.pkg.project.config.styles.background.color.clone();
            if color_edit(ui, "Background color", &mut hex) {
                ed.edit(|p| p.config.styles.background.color = hex);
            }
            if ui.button("Randomize background color").clicked() {
                let hex = random_hex();
                ed.edit(move |p| p.config.styles.background.color = hex);
            }
        }
        "gradient" => {
            widgets::title(ui, "Gradient");
            let stops = ed.session.pkg.project.config.styles.background.gradient.stops.clone();
            for (i, s) in stops.iter().enumerate() {
                let mut hex = s.color.clone();
                if color_edit(ui, &format!("Stop {}", i + 1), &mut hex) {
                    ed.edit(move |p| p.config.styles.background.gradient.stops[i].color = hex);
                }
            }
            if ui.button("Randomize gradient colors").clicked() {
                let (a, b) = (random_hex(), random_hex());
                ed.edit(move |p| {
                    let st = &mut p.config.styles.background.gradient.stops;
                    if st.len() >= 2 {
                        st[0].color = a;
                        st[1].color = b;
                    }
                });
            }
        }
        "system" => {
            widgets::title(ui, "Wallpaper");
            let list = ed.session.assets.list_backgrounds();
            let current = ed.session.pkg.project.config.styles.background.system_name.clone().unwrap_or_default();
            let cats = wallpaper_categories(&list);
            let cat_refs: Vec<&str> = cats.iter().map(String::as_str).collect();
            let cat_idx = cats.iter().position(|c| current.starts_with(&format!("{c}/"))).unwrap_or(0);
            let cat = ui.memory(|m| m.data.get_temp::<usize>(egui::Id::new("wp-cat"))).unwrap_or(cat_idx).min(cats.len().saturating_sub(1));
            let cat = if cats.is_empty() { 0 } else { widgets::pills(ui, cat, &cat_refs).unwrap_or(cat) };
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("wp-cat"), cat));
            let category = cats.get(cat).cloned().unwrap_or_default();
            let in_cat: Vec<String> = list.iter().filter(|n| n.starts_with(&format!("{category}/"))).cloned().collect();
            if ui.add_sized([ui.available_width(), 40.0], egui::Button::new("Pick random wallpaper")).clicked() && !list.is_empty() {
                let pick = list[(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) % list.len() as u128) as usize].clone();
                ed.edit(move |p| p.config.styles.background.system_name = Some(pick));
            }
            ui.add_space(6.0);
            if list.is_empty() {
                widgets::hint(ui, "No wallpapers found next to the app; the gradient is used meanwhile.");
            }
            // thumbnail grid (file names; images are loaded through egui's image loader)
            let cols = 6;
            let gap = 8.0;
            let tile = ((ui.available_width() - (cols as f32 - 1.0) * gap) / cols as f32).floor();
            egui::Grid::new("wallpapers").spacing([gap, gap]).show(ui, |ui| {
                for (i, name) in in_cat.iter().enumerate() {
                    let selected = *name == current;
                    let path = ed.session.assets.background(name);
                    let resp = wallpaper_tile(ui, path.as_deref(), name, tile, selected);
                    if resp.clicked() {
                        let n = name.clone();
                        ed.edit(move |p| p.config.styles.background.system_name = Some(n));
                    }
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        }
        "image" => {
            widgets::title(ui, "Image");
            if ui.button("Choose image…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg"]).pick_file() {
                    let name = p.file_name().unwrap().to_string_lossy().into_owned();
                    let dest = ed.session.pkg.path.join("assets").join(&name);
                    let _ = std::fs::create_dir_all(dest.parent().unwrap());
                    if std::fs::copy(&p, &dest).is_ok() {
                        ed.edit(|pr| pr.config.styles.background.image = Some(serde_json::Value::String(name)));
                    }
                }
            }
        }
        _ => widgets::hint(ui, "Unsplash backgrounds are on the roadmap."),
    }
    cfg_slider(ed, ui, "Background blur", |p| p.config.styles.background.blur, |p, v| p.config.styles.background.blur = v, 0.0..=100.0, Some(0.0), "");
    cfg_slider(ed, ui, "Padding", |p| p.config.output.padding_ratio01 * 100.0, |p, v| p.config.output.padding_ratio01 = v / 100.0, 0.0..=40.0, Some(10.0), "%");
    cfg_slider(ed, ui, "Screen rounded corners", |p| p.config.styles.screen_border_radius, |p, v| p.config.styles.screen_border_radius = v, 0.0..=64.0, Some(16.0), "");
    let mut inset_on = ed.session.pkg.project.config.styles.screen_inset.size > 0.0;
    if widgets::toggle_row(ui, "Inset", "Add inset around the screen.", &mut inset_on) {
        ed.edit(move |p| p.config.styles.screen_inset.size = if inset_on { 8.0 } else { 0.0 });
    }
    if inset_on {
        cfg_slider(ed, ui, "Inset size", |p| p.config.styles.screen_inset.size, |p, v| p.config.styles.screen_inset.size = v, 0.125..=24.0, Some(8.0), "");
        cfg_slider(ed, ui, "Inset opacity", |p| p.config.styles.screen_inset.alpha, |p, v| p.config.styles.screen_inset.alpha = v, 0.0..=1.0, Some(0.5), "");
        let mut hex = ed.session.pkg.project.config.styles.screen_inset.color.clone();
        if color_edit(ui, "Inset color", &mut hex) {
            ed.edit(|p| p.config.styles.screen_inset.color = hex);
        }
    }
    cfg_slider(ed, ui, "Shadow", |p| p.config.styles.shadow.intensity, |p, v| p.config.styles.shadow.intensity = v, 0.0..=0.6, Some(0.3), "");
    let directional = ed.session.pkg.project.config.styles.shadow.is_directional;
    if let Some(i) = widgets::segmented(ui, if directional { 1 } else { 0 }, &["Classic", "Directional"]) {
        ed.edit(move |p| p.config.styles.shadow.is_directional = i == 1);
    }
    egui::CollapsingHeader::new("Advanced shadow settings").show(ui, |ui| {
        cfg_slider(ed, ui, "Shadow Distance", |p| p.config.styles.shadow.distance, |p, v| p.config.styles.shadow.distance = v, 0.0..=100.0, Some(25.0), "");
        cfg_slider(ed, ui, "Shadow Angle", |p| p.config.styles.shadow.angle, |p, v| p.config.styles.shadow.angle = v, 0.0..=180.0, Some(45.0), "°");
        cfg_slider(ed, ui, "Shadow Blur", |p| p.config.styles.shadow.blur, |p, v| p.config.styles.shadow.blur = v, 0.0..=30.0, Some(20.0), "");
    });
}

fn cursor_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Cursor");
    let mut hide = ed.session.pkg.project.config.cursor.hide;
    if widgets::toggle_row(ui, "Hide cursor", "", &mut hide) {
        ed.edit(move |p| p.config.cursor.hide = hide);
    }
    cfg_slider(ed, ui, "Cursor size", |p| p.config.cursor.size, |p, v| p.config.cursor.size = v, 16.0..=128.0, Some(48.0), "");
    widgets::title(ui, "Cursor style");
    let sets = ["macos-tahoe", "macos", "touch"];
    let cur = ed.session.pkg.project.config.cursor.set.id.clone();
    let idx = sets.iter().position(|s| *s == cur).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, idx, &["macOS Tahoe", "macOS", "Touch"]) {
        let id = sets[i].to_string();
        ed.edit(move |p| p.config.cursor.set.id = id);
    }
    widgets::hint(ui, "The recorded cursor sprites are used; cursor sets need the assets folder.");
    let mut always = ed.session.pkg.project.config.cursor.always_use_default;
    if widgets::toggle_row(ui, "Always use pointer cursor", "Don't change cursor, even if selecting text, etc.", &mut always) {
        ed.edit(move |p| p.config.cursor.always_use_default = always);
    }
    let mut idle = ed.session.pkg.project.config.cursor.hide_not_moving_after_ms.is_some();
    if widgets::toggle_row(ui, "Hide cursor if not moving", "", &mut idle) {
        ed.edit(move |p| p.config.cursor.hide_not_moving_after_ms = if idle { Some(2000.0) } else { None });
    }
    if idle {
        cfg_slider(ed, ui, "Hide not moving cursor after", |p| p.config.cursor.hide_not_moving_after_ms.unwrap_or(2000.0), |p, v| p.config.cursor.hide_not_moving_after_ms = Some(v), 500.0..=5000.0, Some(2000.0), " ms");
    }
    widgets::title(ui, "Click effect");
    let effect = ed.session.pkg.project.config.cursor.click_effect.clone();
    let idx = match effect {
        None => 0,
        Some(sf_project::v4::ClickEffect::Circle) => 1,
        Some(sf_project::v4::ClickEffect::Ripple { .. }) => 2,
    };
    if let Some(i) = widgets::segmented(ui, idx, &["None", "Circle", "Ripple"]) {
        ed.edit(move |p| {
            p.config.cursor.click_effect = match i {
                1 => Some(sf_project::v4::ClickEffect::Circle),
                2 => Some(sf_project::v4::ClickEffect::Ripple { pull_size01: None }),
                _ => None,
            }
        });
    }
    widgets::title(ui, "Click sound");
    let sounds = [None, Some("apple-magic-mouse"), Some("logitech-mx-master")];
    let cur = ed.session.pkg.project.config.audio.click_sound_effect.clone();
    let idx = sounds.iter().position(|s| s.map(str::to_string) == cur).unwrap_or(0);
    if let Some(i) = widgets::segmented(ui, idx, &["None", "Magic Mouse™", "MX Master™"]) {
        let v = sounds[i].map(str::to_string);
        ed.edit(move |p| p.config.audio.click_sound_effect = v);
    }
    cfg_slider(ed, ui, "Click sound volume", |p| p.config.audio.click_sound_effect_volume, |p, v| p.config.audio.click_sound_effect_volume = v, 0.0..=1.0, Some(0.25), "");
    widgets::title(ui, "Rotation");
    cfg_slider(ed, ui, "Rotate cursor", |p| p.config.cursor.base_rotation, |p, v| p.config.cursor.base_rotation = v, -180.0..=180.0, Some(0.0), "°");
    let velocity = ed.session.pkg.project.config.cursor.rotation_mode == "velocity";
    if let Some(i) = widgets::segmented(ui, if velocity { 0 } else { 1 }, &["Tilt", "Follow"]) {
        ed.edit(move |p| p.config.cursor.rotation_mode = if i == 0 { "velocity" } else { "directional" }.into());
    }
    cfg_slider(ed, ui, "Tilt amount", |p| p.config.cursor.rotate_on_x_movement_ratio, |p, v| p.config.cursor.rotate_on_x_movement_ratio = v, 0.0..=2.0, Some(0.5), "");
    egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
        cfg_slider(ed, ui, "Stop cursor movement at the end of the video", |p| p.config.cursor.stop_movement_in_last_part_ms, |p, v| p.config.cursor.stop_movement_in_last_part_ms = v, 0.0..=5000.0, Some(0.0), " ms");
        cfg_slider(ed, ui, "Remove cursor shakes", |p| p.config.cursor.remove_shake_treshold, |p, v| p.config.cursor.remove_shake_treshold = v, 100.0..=2000.0, Some(500.0), "");
        let mut opt = ed.session.pkg.project.config.cursor.optimize_original_types;
        if widgets::toggle_row(ui, "Optimize cursor changes", "", &mut opt) {
            ed.edit(move |p| p.config.cursor.optimize_original_types = opt);
        }
    });
}

fn camera_layout_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Default layout");
    let kinds = ["screen-only", "camera-overlay", "split-screen", "fullscreen-camera", "cutout-camera"];
    let cur = ed.session.pkg.project.config.default_layout.kind.clone();
    let idx = kinds.iter().position(|k| *k == cur).unwrap_or(1);
    if let Some(i) = widgets::segmented(ui, idx, &["Only screen", "Camera overlay", "Split screen", "Fullscreen camera", "Camera cutout"]) {
        let k = kinds[i].to_string();
        ed.edit(move |p| p.config.default_layout.kind = k);
    }
    if ed.session.source.camera.is_empty() {
        widgets::hint(ui, "No camera recorded");
    }
    let mut hide = ed.session.pkg.project.config.camera.hide;
    if widgets::toggle_row(ui, "Hide camera", "", &mut hide) {
        ed.edit(move |p| p.config.camera.hide = hide);
    }
    let mut avoid = ed.session.pkg.project.config.output.avoid_empty_zoom_area;
    if widgets::toggle_row(ui, "Avoid empty area", "Scale the screen so no background shows while zoomed.", &mut avoid) {
        ed.edit(move |p| p.config.output.avoid_empty_zoom_area = avoid);
    }
    cfg_slider(ed, ui, "Camera rounded corners", |p| p.config.styles.camera_border_radius_ratio01, |p, v| p.config.styles.camera_border_radius_ratio01 = v, 0.0..=0.5, Some(0.15), "");
    widgets::title(ui, "Camera overlay");
    cfg_slider(ed, ui, "Camera position X", |p| p.config.default_layout.camera_overlay.overlay_camera_position01.x, |p, v| p.config.default_layout.camera_overlay.overlay_camera_position01.x = v, 0.0..=1.0, Some(1.0), "");
    cfg_slider(ed, ui, "Camera position Y", |p| p.config.default_layout.camera_overlay.overlay_camera_position01.y, |p, v| p.config.default_layout.camera_overlay.overlay_camera_position01.y = v, 0.0..=1.0, Some(1.0), "");
    cfg_slider(ed, ui, "Camera Size", |p| p.config.default_layout.camera_overlay.overlay_camera_size_ratio01, |p, v| p.config.default_layout.camera_overlay.overlay_camera_size_ratio01 = v, 0.1..=0.75, Some(0.35), "");
    cfg_slider(ed, ui, "Size during zoom", |p| p.config.default_layout.camera_overlay.overlay_zoomed_camera_scale, |p, v| p.config.default_layout.camera_overlay.overlay_zoomed_camera_scale = v, 0.25..=1.0, Some(0.8), "");
    let aspects = [16.0 / 9.0, 4.0 / 3.0, 1.0, 3.0 / 4.0, 9.0 / 16.0];
    let cur = ed.session.pkg.project.config.default_layout.camera_overlay.overlay_camera_aspect_ratio;
    let idx = aspects.iter().position(|a| (a - cur).abs() < 1e-3).unwrap_or(2);
    widgets::title(ui, "Camera aspect ratio");
    if let Some(i) = widgets::segmented(ui, idx, &["16:9", "4:3", "1:1", "3:4", "9:16"]) {
        let a = aspects[i];
        ed.edit(move |p| {
            p.config.default_layout.camera_overlay.overlay_camera_aspect_ratio = a;
            p.config.camera.aspect_ratio = a;
        });
    }
}

fn camera_style_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Camera Style");
    if ed.session.source.camera.is_empty() {
        widgets::hint(ui, "No camera recorded");
    }
    let mut mirror = ed.session.pkg.project.config.camera.mirror;
    if widgets::toggle_row(ui, "Mirror horizontally", "", &mut mirror) {
        ed.edit(move |p| p.config.camera.mirror = mirror);
    }
    cfg_slider(ed, ui, "Sharpen", |p| p.config.camera.sharpen01, |p, v| p.config.camera.sharpen01 = v, 0.0..=1.0, Some(0.0), "");
    cfg_slider(ed, ui, "Grain", |p| p.config.camera.grain, |p, v| p.config.camera.grain = v, 0.0..=1.0, Some(0.0), "");
    let mut face = ed.session.pkg.project.config.camera.enable_face_tracking;
    widgets::roadmap(ui, |ui| widgets::toggle_row(ui, "Face tracking", "Center the camera on your face.", &mut face));
    widgets::hint(ui, "Color grading looks and LUTs are on the roadmap.");
}

fn captions_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Captions");
    widgets::hint(ui, "Captions are on the roadmap: transcription and caption rendering are not built yet.");
    ui.disable();
    let mut on = ed.session.pkg.project.config.captions.enable_transcript;
    if widgets::toggle_row(ui, "Transcript", "", &mut on) {
        ed.edit(move |p| p.config.captions.enable_transcript = on);
    }
    cfg_slider(ed, ui, "Transcript size", |p| p.config.captions.size_ratio, |p, v| p.config.captions.size_ratio = v, 0.05..=0.125, Some(0.05), "");
    let fonts = ["sans-serif", "serif", "mono"];
    let cur = ed.session.pkg.project.config.captions.font.clone();
    let idx = fonts.iter().position(|f| *f == cur).unwrap_or(0);
    widgets::title(ui, "Font style");
    if let Some(i) = widgets::segmented(ui, idx, &["Sans", "Serif", "Mono"]) {
        let f = fonts[i].to_string();
        ed.edit(move |p| p.config.captions.font = f);
    }
    let reveals = ["line-by-line", "word-by-word"];
    let cur = ed.session.pkg.project.config.captions.words_reveal.clone();
    widgets::title(ui, "Word reveal");
    if let Some(i) = widgets::segmented(ui, reveals.iter().position(|r| *r == cur).unwrap_or(0), &["Line by line", "Word by word"]) {
        let r = reveals[i].to_string();
        ed.edit(move |p| p.config.captions.words_reveal = r);
    }
    let entr = ["appear", "fade-in", "slide-in"];
    let cur = ed.session.pkg.project.config.captions.word_entrance.clone();
    widgets::title(ui, "Word animation");
    if let Some(i) = widgets::segmented(ui, entr.iter().position(|r| *r == cur).unwrap_or(0), &["Appear", "Fade in", "Slide in"]) {
        let r = entr[i].to_string();
        ed.edit(move |p| p.config.captions.word_entrance = r);
    }
}

pub fn audio_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    use sf_project::SceneAudioExt;
    let Some(id) = ed.selected_audio.clone() else { return };
    let Some(clip) = ed.session.pkg.project.scenes[0].audio_clips().and_then(|cs| cs.into_iter().find(|c| c.id == id)) else {
        ed.selected_audio = None;
        return;
    };
    widgets::title(ui, "Audio clip");
    widgets::hint(ui, &format!("{} · {} – {} on the timeline", if clip.track == sf_project::AudioTrack::SystemAudio { "System audio" } else { "Microphone" }, crate::editor::fmt_clock(clip.output_start_ms), crate::editor::fmt_clock(clip.output_end_ms())));
    let idc = id.clone();
    cfg_slider(ed, ui, "Volume", move |p| p.scenes[0].audio_clips().and_then(|cs| cs.into_iter().find(|c| c.id == idc)).map(|c| c.volume).unwrap_or(1.0), {
        let idc = id.clone();
        move |p, v| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                    c.volume = v;
                }
                scene.set_audio_clips(Some(cs));
            }
        }
    }, 0.0..=2.0, Some(1.0), "");
    widgets::title(ui, "Speed");
    let speeds = crate::timeline::SPEEDS;
    let labels: Vec<String> = speeds.iter().map(|s| format!("{s}×")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let cur = speeds.iter().position(|s| (*s - clip.speed).abs() < 1e-6).unwrap_or(2);
    if let Some(i) = widgets::pills(ui, cur, &refs) {
        ed.set_audio_clip_speed(&id, speeds[i]);
    }
    widgets::hint(ui, "Audio speed is independent of the video clip below it; pitch is preserved.");
    let mut muted = clip.muted;
    if widgets::toggle_row(ui, "Mute", "", &mut muted) {
        let idc = id.clone();
        ed.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                    c.muted = muted;
                }
                scene.set_audio_clips(Some(cs));
            }
        });
    }
    ui.horizontal(|ui| {
        if ui.button("Split at playhead").clicked() {
            ed.split_at_playhead();
        }
        if ui.button("Delete").clicked() {
            ed.delete_selected();
        }
    });
}

fn audio_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    use sf_project::SceneAudioExt;
    widgets::title(ui, "Audio track");
    let detached = ed.session.pkg.project.scenes[0].is_audio_detached();
    if crate::audio::AudioPlayer::available_tracks(&ed.session).is_empty() {
        widgets::hint(ui, "This recording has no microphone or system audio. Add background music below.");
    } else if detached {
        widgets::hint(ui, "The audio track is edited separately from the video: select a clip to split (S), move, trim or mute it.");
        if widgets::secondary_button(ui, "Re-link audio to video").on_hover_text("Discard separate audio edits; audio follows the video clips again").clicked() {
            ed.selected_audio = None;
            ed.edit(|p| p.scenes[0].set_audio_clips(None));
        }
    } else {
        widgets::hint(ui, "Audio follows the video clips. Click or drag a clip on the audio track to edit it separately: split, move, trim or mute.");
    }
    widgets::title(ui, "Microphone");
    if ed.session.source.microphone.is_empty() {
        widgets::hint(ui, "No microphone recorded");
    }
    let mut mute = ed.session.pkg.project.config.audio.mute_microphone;
    if widgets::toggle_row(ui, "Mute microphone", "", &mut mute) {
        ed.edit(move |p| p.config.audio.mute_microphone = mute);
        ed.reload_audio();
    }
    cfg_slider(ed, ui, "Microphone volume", |p| p.config.audio.volume, |p, v| p.config.audio.volume = v, 0.0..=2.0, Some(1.0), "");
    let presets = ["voice", "noise-reduction", "podcast"];
    let cur = ed.session.pkg.project.config.audio.microphone_audio_preset.clone();
    widgets::title(ui, "Microphone audio mode");
    if let Some(i) = widgets::segmented(ui, presets.iter().position(|p| *p == cur).unwrap_or(0), &["Standard", "Noise reduction", "Podcast"]) {
        let v = presets[i].to_string();
        ed.edit(move |p| p.config.audio.microphone_audio_preset = v);
        ed.reload_audio();
    }
    widgets::hint(ui, "Noise reduction removes steady background hiss and hum. Podcast also evens out and boosts your voice. Applied in preview and export.");
    widgets::title(ui, "System audio");
    if ed.session.source.system_audio.is_empty() {
        widgets::hint(ui, "No system audio recorded");
    }
    let mut mute_sys = ed.session.pkg.project.config.audio.mute_system_audio;
    if widgets::toggle_row(ui, "Mute system audio", "", &mut mute_sys) {
        ed.edit(move |p| p.config.audio.mute_system_audio = mute_sys);
        ed.reload_audio();
    }
    widgets::title(ui, "Background audio");
    let current = ed.session.pkg.project.config.audio.background_audio_file_name.clone();
    let library = ed.session.assets.list_background_audio();
    if library.is_empty() {
        widgets::hint(ui, "No music library on this Mac yet. Add your own track with \"Add background audio…\" below.");
    }
    let mut categories: Vec<String> = library.iter().filter_map(|n| n.split('/').next().map(str::to_string)).collect();
    categories.dedup();
    let none_sel = current.is_none();
    if ui.selectable_label(none_sel, "None").clicked() && !none_sel {
        ed.edit(|p| p.config.audio.background_audio_file_name = None);
        ed.reload_audio();
    }
    for cat in categories {
        let title = {
            let mut c = cat.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => cat.clone(),
            }
        };
        let cur_key = current.as_deref().map(sf_session::assets::background_audio_key);
        let open_cat = library.iter().any(|n| n.starts_with(&format!("{cat}/")) && cur_key.as_deref() == Some(sf_session::assets::background_audio_key(n).as_str()));
        egui::CollapsingHeader::new(title).default_open(open_cat).show(ui, |ui| {
            for track in library.iter().filter(|n| n.starts_with(&format!("{cat}/"))) {
                let label = sf_session::assets::background_audio_label(track);
                let sel = current.as_deref().is_some_and(|c| sf_session::assets::background_audio_key(c) == sf_session::assets::background_audio_key(track));
                if ui.selectable_label(sel, label).clicked() && !sel {
                    let t = track.clone();
                    ed.edit(move |p| {
                        p.config.audio.background_audio_file_name = Some(t);
                        p.config.audio.mute_background_audio = false;
                        // Screen Studio's 5 % default is near-silent; start at an audible level
                        if p.config.audio.background_audio_volume < 0.2 {
                            p.config.audio.background_audio_volume = 0.3;
                        }
                    });
                    ed.reload_audio();
                    ed.preview_background_audio();
                }
            }
        });
    }
    if ui.button("Add background audio…").clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("Audio", &["mp3", "m4a", "wav", "aac", "mp4"]).pick_file() {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let dest = ed.session.pkg.path.join("assets").join(&name);
            let _ = std::fs::create_dir_all(dest.parent().unwrap());
            if std::fs::copy(&p, &dest).is_ok() {
                ed.edit(move |pr| {
                    pr.config.audio.background_audio_file_name = Some(name);
                    pr.config.audio.mute_background_audio = false;
                    if pr.config.audio.background_audio_volume < 0.2 {
                        pr.config.audio.background_audio_volume = 0.3;
                    }
                });
                ed.reload_audio();
                ed.preview_background_audio();
            }
        }
    }
    let mut mute_bg = ed.session.pkg.project.config.audio.mute_background_audio;
    if widgets::toggle_row(ui, "Mute background audio", "", &mut mute_bg) {
        ed.edit(move |p| p.config.audio.mute_background_audio = mute_bg);
    }
    cfg_slider(ed, ui, "Background audio volume", |p| p.config.audio.background_audio_volume, |p, v| p.config.audio.background_audio_volume = v, 0.0..=1.0, Some(0.05), "");
}

fn shortcuts_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Keyboard Shortcuts");
    widgets::hint(ui, "Keyboard shortcut overlays are on the roadmap: key caps are not rendered yet.");
    ui.disable();
    if ed.session.cursor_track.key_downs.is_empty() {
        widgets::hint(ui, "No keyboard shortcuts during recording");
    }
    let mut show = ed.session.pkg.project.config.captions.enable_shortcuts;
    if widgets::toggle_row(ui, "Show shortcuts", "Show pressed shortcut labels in the video.", &mut show) {
        ed.edit(move |p| p.config.captions.enable_shortcuts = show);
    }
    cfg_slider(ed, ui, "Shortcut labels size", |p| p.config.captions.shortcuts_size_ratio, |p, v| p.config.captions.shortcuts_size_ratio = v, 0.05..=0.2, Some(0.1), "");
    let mut single = ed.session.pkg.project.config.captions.show_shortcuts_with_single_letters;
    if widgets::toggle_row(ui, "Show single key shortcuts", "", &mut single) {
        ed.edit(move |p| p.config.captions.show_shortcuts_with_single_letters = single);
    }
}

fn animations_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    use sf_engine::spring::SpringParams;
    use sf_project::schema::common::Spring;
    let to_spring = |p: SpringParams| Spring { stiffness: p.stiffness, damping: p.damping, mass: p.mass, clamp: Some(true), precision: Some(0.002) };
    widgets::title(ui, "Cursor animation style");
    let cur = ed.session.pkg.project.config.animations.mouse_movement_spring;
    let presets = [Some(SpringParams::CURSOR_SMOOTH), Some(SpringParams::CURSOR_MEDIUM), Some(SpringParams::CURSOR_RAPID), None];
    let idx = presets.iter().position(|p| match (p, cur) { (None, None) => true, (Some(a), Some(b)) => (a.stiffness - b.stiffness).abs() < 1e-6 && (a.damping - b.damping).abs() < 1e-6, _ => false }).unwrap_or(4);
    if let Some(i) = widgets::segmented(ui, idx, &["Smooth", "Medium", "Rapid", "None"]) {
        let v = presets[i].map(to_spring);
        ed.edit(move |p| p.config.animations.mouse_movement_spring = v);
        ed.reload_audio();
    }
    if cur.is_some() {
        cfg_slider(ed, ui, "Rigidity", |p| p.config.animations.mouse_movement_spring.map(|s| s.stiffness).unwrap_or(0.0), |p, v| { if let Some(s) = &mut p.config.animations.mouse_movement_spring { s.stiffness = v } }, 5.0..=600.0, None, "");
        cfg_slider(ed, ui, "Smoothness", |p| p.config.animations.mouse_movement_spring.map(|s| s.damping).unwrap_or(0.0), |p, v| { if let Some(s) = &mut p.config.animations.mouse_movement_spring { s.damping = v } }, 5.0..=200.0, None, "");
        cfg_slider(ed, ui, "Momentum", |p| p.config.animations.mouse_movement_spring.map(|s| s.mass).unwrap_or(0.0), |p, v| { if let Some(s) = &mut p.config.animations.mouse_movement_spring { s.mass = v } }, 0.1..=15.0, None, "");
    }
    widgets::title(ui, "Screen animation style");
    let cur = ed.session.pkg.project.config.animations.screen_movement_spring;
    let presets = [SpringParams::SCREEN_FOCUSED, SpringParams::SCREEN_SMOOTH];
    let idx = presets.iter().position(|a| (a.stiffness - cur.stiffness).abs() < 1e-6 && (a.damping - cur.damping).abs() < 1e-6).unwrap_or(2);
    if let Some(i) = widgets::segmented(ui, idx, &["Focused", "Smooth"]) {
        let v = to_spring(presets[i]);
        ed.edit(move |p| p.config.animations.screen_movement_spring = v);
    }
    cfg_slider(ed, ui, "Rigidity", |p| p.config.animations.screen_movement_spring.stiffness, |p, v| p.config.animations.screen_movement_spring.stiffness = v, 5.0..=600.0, Some(125.0), "");
    cfg_slider(ed, ui, "Smoothness", |p| p.config.animations.screen_movement_spring.damping, |p, v| p.config.animations.screen_movement_spring.damping = v, 5.0..=200.0, Some(12.0), "");
    cfg_slider(ed, ui, "Momentum", |p| p.config.animations.screen_movement_spring.mass, |p, v| p.config.animations.screen_movement_spring.mass = v, 0.1..=15.0, Some(1.5), "");
    cfg_slider(ed, ui, "Motion blur", |p| p.config.animations.motion_blur_amount, |p, v| p.config.animations.motion_blur_amount = v, 0.0..=1.0, Some(1.0), "");
    let mut early = ed.session.pkg.project.config.zooms.start_animation_early;
    if widgets::toggle_row(ui, "Start zoom animation early", "", &mut early) {
        ed.edit(move |p| p.config.zooms.start_animation_early = early);
    }
    let mut typing = ed.session.pkg.project.config.zooms.zoom_on_typing();
    if widgets::toggle_row(ui, "Zoom while typing", "Automatic zooms also follow keystrokes, not only clicks.", &mut typing) {
        ed.edit(move |p| p.config.zooms.zoom_on_typing = Some(typing));
    }
}

fn device_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Device frame");
    widgets::hint(ui, "Device frames are on the roadmap: mockups are not rendered yet.");
    let mut on = ed.session.pkg.project.config.device.enable_mockup;
    widgets::roadmap(ui, |ui| widgets::toggle_row(ui, "Use device mockup", "Mockups apply to iPhone/iPad recordings.", &mut on));
    widgets::title(ui, "Project");
    let p = &ed.session.pkg;
    widgets::hint(ui, &p.path.display().to_string());
    widgets::hint(ui, &format!("format v{} · recorded {}x{} @ {:.0} fps", p.project.version, ed.session.source.native_width, ed.session.source.native_height, ed.session.source.fps));
    if p.migrated {
        widgets::hint(ui, "Migrated from an older project format; saving writes v4.");
    }
}

fn random_hex() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let h = (n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 33) % 360;
    // pleasant saturated colour from a hue
    let (r, g, b) = hsl_to_rgb(h as f64, 0.6, 0.5);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 { 0..=59 => (c, x, 0.0), 60..=119 => (x, c, 0.0), 120..=179 => (0.0, c, x), 180..=239 => (0.0, x, c), 240..=299 => (x, 0.0, c), _ => (c, 0.0, x) };
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}


/// A square, centre-cropped wallpaper thumbnail.
fn wallpaper_tile(ui: &mut egui::Ui, path: Option<&std::path::Path>, name: &str, size: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let painter = ui.painter();
    let radius = 9.0;
    let mut drawn = false;
    if let Some(p) = path {
        let uri = format!("file://{}", p.display());
        if let Ok(egui::load::TexturePoll::Ready { texture }) = ui.ctx().try_load_texture(&uri, egui::TextureOptions::LINEAR, egui::load::SizeHint::Size { width: 256, height: 256, maintain_aspect_ratio: true }) {
            let (w, h) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
            // centre crop to a square in UV space
            let uv = if w > h {
                let f = h / w;
                egui::Rect::from_min_max(egui::pos2((1.0 - f) / 2.0, 0.0), egui::pos2((1.0 + f) / 2.0, 1.0))
            } else {
                let f = w / h;
                egui::Rect::from_min_max(egui::pos2(0.0, (1.0 - f) / 2.0), egui::pos2(1.0, (1.0 + f) / 2.0))
            };
            let mut mesh = egui::Mesh::with_texture(texture.id);
            mesh.add_rect_with_uv(rect, uv, egui::Color32::WHITE);
            // rounded corners: paint the mesh, then mask the corners with the card colour
            painter.add(egui::Shape::mesh(mesh));
            let corner = egui::Shape::Path(egui::epaint::PathShape::convex_polygon(vec![], egui::Color32::TRANSPARENT, egui::Stroke::NONE));
            let _ = corner;
            painter.rect_stroke(rect, radius, egui::Stroke::new(3.0, theme::p().card), egui::StrokeKind::Outside);
            drawn = true;
        }
    }
    if !drawn {
        painter.rect_filled(rect, radius, theme::p().card_2);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, name.rsplit('/').next().unwrap_or(name).chars().take(8).collect::<String>(), egui::FontId::proportional(9.0), theme::p().text_dim);
    }
    if selected {
        painter.rect_stroke(rect.expand(1.0), radius, egui::Stroke::new(2.0, theme::p().text), egui::StrokeKind::Outside);
    } else if resp.hovered() {
        painter.rect_stroke(rect, radius, egui::Stroke::new(1.5, theme::p().text_dim), egui::StrokeKind::Inside);
    }
    let pretty = name.rsplit('/').next().unwrap_or(name).trim_end_matches(".jpg").trim_end_matches(".png").replace('-', " ");
    resp.on_hover_text(pretty)
}
