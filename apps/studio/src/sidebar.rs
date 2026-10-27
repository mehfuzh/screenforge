//! Right-hand inspector: project configuration grouped like Screen Studio's sidebar.

use egui::RichText;

use sf_project::schema::common::Point;
use sf_session::cursor_sets;

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

/// A preset tile painted with its gradient (a vertex-coloured grid, so stops in the middle
/// show too); outlined when it is the project's gradient.
fn gradient_swatch(ui: &mut egui::Ui, g: &sf_project::schema::common::Gradient, selected: bool, name: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(64.0, 40.0), egui::Sense::click());
    let colour_at = |t: f32| {
        let stops: Vec<(sf_render::Color, f32)> = g.stops.iter().map(|s| (sf_render::Color::from_hex(&s.color).unwrap_or(sf_render::Color::BLACK), s.at as f32)).collect();
        // before the first stop / after the last: that stop's colour
        let (c0, c1, f) = match stops.iter().position(|(_, at)| *at >= t) {
            Some(0) => (stops[0].0, stops[0].0, 0.0),
            Some(i) => (stops[i - 1].0, stops[i].0, (t - stops[i - 1].1) / (stops[i].1 - stops[i - 1].1).max(1e-6)),
            None => {
                let last = stops.last().map(|s| s.0).unwrap_or(sf_render::Color::BLACK);
                (last, last, 0.0)
            }
        };
        let mix = |a: f32, b: f32| ((a + (b - a) * f) * 255.0).round().clamp(0.0, 255.0) as u8;
        egui::Color32::from_rgb(mix(c0.r, c1.r), mix(c0.g, c1.g), mix(c0.b, c1.b))
    };
    // project each vertex on the start → end axis (in tile fractions)
    let (sx, sy, ex, ey) = (g.start.x as f32, g.start.y as f32, g.end.x as f32, g.end.y as f32);
    let (dx, dy) = (ex - sx, ey - sy);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    const N: u32 = 8;
    let mut mesh = egui::Mesh::default();
    for j in 0..=N {
        for i in 0..=N {
            let (u, v) = (i as f32 / N as f32, j as f32 / N as f32);
            let t = (((u - sx) * dx + (v - sy) * dy) / len2).clamp(0.0, 1.0);
            mesh.colored_vertex(egui::pos2(rect.left() + u * rect.width(), rect.top() + v * rect.height()), colour_at(t));
        }
    }
    for j in 0..N {
        for i in 0..N {
            let a = j * (N + 1) + i;
            mesh.add_triangle(a, a + 1, a + N + 1);
            mesh.add_triangle(a + 1, a + N + 2, a + N + 1);
        }
    }
    let painter = ui.painter();
    painter.add(mesh);
    let stroke = if selected { egui::Stroke::new(2.0, theme::p().accent_hover) } else if resp.hovered() { egui::Stroke::new(1.0, theme::p().text_dim) } else { egui::Stroke::new(1.0, theme::p().border) };
    painter.rect_stroke(rect, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
    resp.on_hover_text(name)
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

/// Inspector for the selected mask (`Editor::selected_mask`).
pub fn mask_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    use sf_project::v4::Mask;
    let Some(id) = ed.selected_mask.clone() else { return };
    let Some(m) = ed.session.pkg.project.scenes[0].masks.iter().find(|m| m.id == id).cloned() else {
        ed.selected_mask = None;
        return;
    };
    let with = |id: &str, p: &mut sf_project::Project, f: &dyn Fn(&mut Mask)| {
        if let Some(m) = p.scenes[0].masks.iter_mut().find(|m| m.id == id) {
            f(m)
        }
    };
    widgets::title(ui, "Mask");
    widgets::hint(ui, "Drag the frame on the preview to move or resize it.");
    let highlight = m.kind == Mask::HIGHLIGHT;
    if let Some(i) = widgets::segmented(ui, highlight as usize, &["Blur", "Highlight"]) {
        let kind = if i == 1 { Mask::HIGHLIGHT } else { Mask::SENSITIVE_DATA };
        let mid = id.clone();
        ed.edit(move |p| with(&mid, p, &|m| m.kind = kind.into()));
    }
    let (gid, sid) = (id.clone(), id.clone());
    if highlight {
        widgets::hint(ui, "Everything outside the areas is dimmed.");
        cfg_slider(ed, ui, "Dim surroundings", move |p| p.scenes[0].masks.iter().find(|m| m.id == gid).map(|m| m.highlight_mask_opacity).unwrap_or(0.5), move |p, v| with(&sid, p, &|m| m.highlight_mask_opacity = v), 0.0..=1.0, Some(0.5), "");
    } else {
        widgets::hint(ui, "The areas are blurred, for passwords, emails and other private details.");
        cfg_slider(ed, ui, "Blur amount", move |p| p.scenes[0].masks.iter().find(|m| m.id == gid).map(|m| m.blur).unwrap_or(32.0), move |p, v| with(&sid, p, &|m| m.blur = v), 4.0..=100.0, Some(32.0), "");
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(match m.bounds.len() { 1 => "1 area".to_string(), n => format!("{n} areas") }).color(theme::p().text_dim));
        if ui.button("Add area").clicked() {
            let r = crate::masks::new_region(ed);
            let mid = id.clone();
            ed.edit(move |p| with(&mid, p, &|m| m.bounds.push(r)));
        }
        if m.bounds.len() > 1 && ui.button("Remove last").clicked() {
            let mid = id.clone();
            ed.edit(move |p| with(&mid, p, &|m| { m.bounds.pop(); }));
        }
    });
    let mut disabled = m.is_disabled;
    if ui.checkbox(&mut disabled, "Disabled").changed() {
        let mid = id.clone();
        ed.edit(move |p| with(&mid, p, &|m| m.is_disabled = disabled));
    }
    if ui.button("Remove mask").clicked() {
        ed.delete_selected();
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
        ui.horizontal_wrapped(|ui| {
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
    Narration,
    Shortcuts,
    Animations,
    Agent,
    DeviceFrame,
}

impl Section {
    pub const ALL: [Section; 11] = [Section::Agent, Section::General, Section::Cursor, Section::CameraLayout, Section::CameraStyle, Section::Captions, Section::Audio, Section::Narration, Section::Shortcuts, Section::Animations, Section::DeviceFrame];

    pub fn icon(self) -> widgets::Icon {
        match self {
            Section::General => widgets::Icon::General,
            Section::Cursor => widgets::Icon::Cursor,
            Section::CameraLayout => widgets::Icon::CameraLayout,
            Section::CameraStyle => widgets::Icon::CameraStyle,
            Section::Captions => widgets::Icon::Captions,
            Section::Audio => widgets::Icon::Audio,
            Section::Narration => widgets::Icon::Narration,
            Section::Shortcuts => widgets::Icon::Keyboard,
            Section::Animations => widgets::Icon::Animations,
            Section::Agent => widgets::Icon::Agent,
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
            Section::Narration => "Narration",
            Section::Shortcuts => "Keyboard Shortcuts",
            Section::Animations => "Animations",
            Section::Agent => "Agent (⌘L)",
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
    // the chat keeps its own scrolling log and a message field pinned to the bottom
    if section == Section::Agent {
        let mut agent = std::mem::take(&mut ed.agent);
        agent.ui(ed, ui);
        ed.agent = agent;
        return;
    }
    // a solid scroll bar takes its own column, so full-width controls (sliders, segmented
    // rows) stop short of it instead of running underneath egui's floating bar
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 6.0;
    scroll.bar_inner_margin = 8.0;
    scroll.bar_outer_margin = 0.0;
    ui.spacing_mut().scroll = scroll;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(8.0);
        if ed.selected_zoom.is_some() {
            zoom_item_ui(ed, ui);
            ui.add_space(12.0);
            ui.separator();
        }
        if ed.selected_mask.is_some() {
            mask_item_ui(ed, ui);
            ui.add_space(12.0);
            ui.separator();
        }
        if ed.selected_audio.is_some() {
            audio_item_ui(ed, ui);
            ui.add_space(12.0);
            ui.separator();
        }
        if ed.selected_voice_over.is_some() {
            voice_over_item_ui(ed, ui);
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
            Section::Narration => narration_ui(ed, ui),
            Section::Shortcuts => shortcuts_ui(ed, ui),
            Section::Animations => animations_ui(ed, ui),
            Section::Agent => {}
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
    // the desktop's own wallpapers first (macOS, or the Linux distribution's)
    if let Some(c) = sf_session::assets::system_wallpapers::category() {
        if present.contains(&c) {
            out.push(c);
        }
    }
    for c in BUNDLED_CATEGORIES.iter().chain(KNOWN_EXTRA_CATEGORIES.iter()) {
        if out.iter().any(|o| o == c) {
            continue;
        }
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
    const RECORDING_SPEEDS: [f64; 6] = [1.0, 1.5, 2.0, 3.0, 4.0, 8.0];
    let labels: Vec<String> = RECORDING_SPEEDS.iter().map(|s| format!("{s}×")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    // a selected clip gets the speed on its own; otherwise the whole recording does
    let selected = ed.selected_slice.filter(|i| *i < ed.session.pkg.project.scenes[0].slices.len());
    match selected {
        Some(i) => {
            widgets::title(ui, "Speed");
            let cur = 1.0 / ed.session.pkg.project.scenes[0].slices[i].time_scale.max(1e-6);
            let idx = RECORDING_SPEEDS.iter().position(|s| (s - cur).abs() < 1e-3).unwrap_or(usize::MAX);
            if let Some(k) = widgets::pills(ui, idx, &refs) {
                let sp = RECORDING_SPEEDS[k];
                ed.edit_speed(move |p| p.scenes[0].slices = sf_engine::timeline::edit::set_speed(&p.scenes[0].slices, i, sp));
                ed.set_status(&format!("Clip {} speed {sp}×", i + 1));
            }
        }
        None => {
            widgets::title(ui, "Speed");
            let cur = ed.recording_speed();
            let idx = cur.and_then(|c| RECORDING_SPEEDS.iter().position(|s| (s - c).abs() < 1e-3)).unwrap_or(usize::MAX);
            if let Some(k) = widgets::pills(ui, idx, &refs) {
                ed.set_recording_speed(RECORDING_SPEEDS[k]);
            }
        }
    }
    ui.add_space(6.0);
    widgets::title(ui, "Background");
    let kinds = ["system", "gradient", "color", "image", "unsplash"];
    let kind = ed.session.pkg.project.config.styles.background.kind.clone();
    let idx = kinds.iter().position(|k| *k == kind).unwrap_or(0);
    if let Some(i) = widgets::segmented_with_disabled(ui, idx, &["Wallpaper", "Gradient", "Color", "Image", "Unsplash"], &[]) {
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
            let current = ed.session.pkg.project.config.styles.background.gradient.clone();
            ui.horizontal_wrapped(|ui| {
                for (name, _) in sf_project::schema::common::Gradient::PRESETS {
                    let Some(g) = sf_project::schema::common::Gradient::preset(name) else { continue };
                    if gradient_swatch(ui, &g, g.stops == current.stops, name).clicked() {
                        ed.edit(move |p| p.config.styles.background.gradient = g);
                    }
                }
            });
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
        "unsplash" => unsplash_ui(ed, ui),
        _ => {}
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
    // tiles: the recorded pointer (what the OS showed) and the drawn sets, previewed from the
    // same sprites the renderer uses
    let cur = ed.session.pkg.project.config.cursor.set.id.clone();
    let idx = cursor_sets::SETS.iter().position(|(s, _)| *s == cur).unwrap_or(0);
    let arrow = ed.session.pkg.recording_dir().join("cursors").join("arrow.png");
    let mut pick = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for (i, (id, label)) in cursor_sets::SETS.iter().enumerate() {
            let preview = if *id == cursor_sets::RECORDED {
                if arrow.is_file() { CursorPreview::Image(crate::os::texture_uri(&arrow)) } else { CursorPreview::Arrow }
            } else {
                cursor_set_preview(ui.ctx(), id)
            };
            if cursor_tile(ui, preview, label, i == idx).clicked() {
                pick = Some(id.to_string());
            }
        }
    });
    if let Some(id) = pick {
        ed.edit(move |p| p.config.cursor.set.id = id);
    }
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

    // ── face tracking ──
    widgets::title(ui, "Face tracking");
    let mut face = ed.session.pkg.project.config.camera.enable_face_tracking;
    if ed.session.face_tracker.is_available() {
        if widgets::toggle_row(ui, "Follow my face", "Keep your face centred in the camera. Detected on this computer, nothing leaves it.", &mut face) {
            ed.edit(move |p| p.config.camera.enable_face_tracking = face);
        }
        if face {
            cfg_slider(ed, ui, "Framing", |p| p.config.camera.face_tracking_zoom(), |p, v| p.config.camera.face_tracking_zoom = if (v - 1.0).abs() < 1e-9 { None } else { Some(v) }, 0.5..=2.0, Some(1.0), "×");
            widgets::hint(ui, "Lower is a wider shot, higher is closer. The window glides after you and holds still for small movements.");
        }
    } else {
        widgets::roadmap(ui, |ui| widgets::toggle_row(ui, "Follow my face", "The face detector failed to load.", &mut face));
    }

    // ── look (LUT) ──
    widgets::title(ui, "Look");
    let bundled = ed.session.assets.list_luts();
    let current = ed.session.pkg.project.config.camera.lut.clone();
    let mut names: Vec<String> = bundled.clone();
    if let Some(c) = &current {
        if !names.contains(c) {
            names.push(c.clone());
        }
    }
    let mut labels: Vec<String> = vec!["None".into()];
    labels.extend(names.iter().map(|n| n.strip_suffix(".cube").unwrap_or(n).to_string()));
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let idx = current.as_ref().and_then(|c| names.iter().position(|n| n == c)).map(|i| i + 1).unwrap_or(0);
    if let Some(i) = widgets::pills(ui, idx, &label_refs) {
        let name = (i > 0).then(|| names[i - 1].clone());
        ed.edit(move |p| p.config.camera.lut = name);
    }
    if ui.button("Import .cube…").clicked() {
        if let Some(path) = rfd::FileDialog::new().add_filter("3D LUT", &["cube", "CUBE"]).pick_file() {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let dest = ed.session.pkg.path.join("assets").join(&name);
            let _ = std::fs::create_dir_all(dest.parent().unwrap());
            if std::fs::copy(&path, &dest).is_ok() {
                ed.edit(move |p| p.config.camera.lut = Some(name));
            }
        }
    }
    widgets::hint(ui, "3D LUTs in DaVinci Resolve's .cube format; the file is copied into the project.");
    if current.is_some() {
        cfg_slider(ed, ui, "Look intensity", |p| p.config.camera.lut_intensity, |p, v| p.config.camera.lut_intensity = v, 0.0..=1.0, Some(1.0), "");
    }

    // ── primaries: colour wheels ──
    widgets::title(ui, "Color Wheels");
    widgets::hint(ui, "DaVinci Resolve primaries: the puck shifts colour, the number under it is the master.");
    ui.add_space(6.0);
    let grade = ed.session.pkg.project.config.camera.grade();
    let wheels: [(&str, fn(&sf_project::v4::ColorGrade) -> [f64; 4], fn(&mut sf_project::v4::ColorGrade) -> &mut [f64; 4], f64, std::ops::RangeInclusive<f64>, f64); 4] = [
        ("Lift", |g| g.lift, |g| &mut g.lift, sf_project::v4::ColorGrade::LIFT_DEFAULT, -1.0..=1.0, 0.0025),
        ("Gamma", |g| g.gamma, |g| &mut g.gamma, sf_project::v4::ColorGrade::GAMMA_DEFAULT, -1.0..=1.0, 0.0025),
        ("Gain", |g| g.gain, |g| &mut g.gain, sf_project::v4::ColorGrade::GAIN_DEFAULT, 0.0..=4.0, 0.0025),
        ("Offset", |g| g.offset, |g| &mut g.offset, sf_project::v4::ColorGrade::OFFSET_DEFAULT, -25.0..=75.0, 0.05),
    ];
    // four square tiles in one aligned row: name, wheel, master value
    let gap = 6.0;
    let tile_w = ((ui.available_width() - gap * 3.0) / 4.0).clamp(56.0, 112.0);
    let tile_h = tile_w + 34.0;
    let diameter = (tile_w - 18.0).min(72.0);
    egui::Grid::new("camera-color-wheels").num_columns(4).spacing(egui::vec2(gap, 0.0)).show(ui, |ui| {
        for (name, get, get_mut, default, range, speed) in wheels.iter() {
            let mut w = get(&grade);
            let (tile, _) = ui.allocate_exact_size(egui::vec2(tile_w, tile_h), egui::Sense::hover());
            ui.painter().rect_filled(tile, egui::CornerRadius::same(2), theme::p().card_2);
            let mut cell = ui.new_child(egui::UiBuilder::new().max_rect(tile.shrink(4.0)).layout(egui::Layout::top_down(egui::Align::Center)));
            cell.spacing_mut().item_spacing.y = 4.0;
            cell.label(RichText::new(*name).size(13.0).color(theme::p().text));
            let mut rgb = [w[0], w[1], w[2]];
            let disc = widgets::color_wheel(&mut cell, &mut rgb, diameter);
            let mut master = w[3];
            let drag = cell.add(egui::DragValue::new(&mut master).speed(*speed).range(range.clone()).fixed_decimals(2));
            if drag.double_clicked() {
                master = *default;
            }
            let changed = disc.changed() || drag.changed() || (drag.double_clicked() && (master - w[3]).abs() > 1e-12);
            if changed {
                w = [rgb[0], rgb[1], rgb[2], master];
                let get_mut = *get_mut;
                set_grade(ed, move |g| *get_mut(g) = w);
            }
            let commit = disc.drag_stopped() || drag.drag_stopped() || ((disc.changed() || drag.changed()) && !disc.dragged() && !drag.dragged());
            if commit {
                push_undo(ed);
            }
        }
        ui.end_row();
    });
    ui.add_space(4.0);

    // ── adjustment controls ──
    widgets::title(ui, "Adjustments");
    grade_slider(ed, ui, "Temperature", |g| g.temperature, |g, v| g.temperature = v, -100.0..=100.0, 0.0, "");
    grade_slider(ed, ui, "Tint", |g| g.tint, |g, v| g.tint = v, -100.0..=100.0, 0.0, "");
    grade_slider(ed, ui, "Contrast", |g| g.contrast, |g, v| g.contrast = v, 0.0..=2.0, 1.0, "");
    grade_slider(ed, ui, "Pivot", |g| g.pivot, |g, v| g.pivot = v, 0.0..=1.0, sf_project::v4::ColorGrade::PIVOT_DEFAULT, "");
    grade_slider(ed, ui, "Color Boost", |g| g.color_boost, |g, v| g.color_boost = v, -100.0..=100.0, 0.0, "");
    grade_slider(ed, ui, "Shadows", |g| g.shadows, |g, v| g.shadows = v, -100.0..=100.0, 0.0, "");
    grade_slider(ed, ui, "Highlights", |g| g.highlights, |g, v| g.highlights = v, -100.0..=100.0, 0.0, "");
    grade_slider(ed, ui, "Saturation", |g| g.saturation, |g, v| g.saturation = v, 0.0..=100.0, 50.0, "");
    grade_slider(ed, ui, "Hue", |g| g.hue, |g, v| g.hue = v, 0.0..=100.0, 50.0, "");
    ui.add_space(6.0);
    let neutral = ed.session.pkg.project.config.camera.color_grade.as_ref().map(|g| g.is_neutral()).unwrap_or(true);
    ui.add_enabled_ui(!neutral, |ui| {
        if widgets::secondary_button(ui, "Reset grade").clicked() {
            ed.edit(|p| p.config.camera.color_grade = None);
        }
    });
}

/// Apply a live change to the camera grade (no undo entry; see [`push_undo`]). A grade left at
/// its defaults is removed from the project again.
fn set_grade(ed: &mut Editor, f: impl FnOnce(&mut sf_project::v4::ColorGrade)) {
    let cam = &mut ed.session.pkg.project.config.camera;
    let g = cam.color_grade.get_or_insert_with(Default::default);
    f(g);
    if g.is_neutral() {
        cam.color_grade = None;
    }
    ed.session.rebuild();
    ed.invalidate_preview();
}

fn push_undo(ed: &mut Editor) {
    let p = ed.session.pkg.project.clone();
    ed.undo.push(p);
    ed.redo.clear();
}

/// `cfg_slider` for one of the grade's adjustment controls (Resolve display units).
fn grade_slider(ed: &mut Editor, ui: &mut egui::Ui, label: &str, get: impl Fn(&sf_project::v4::ColorGrade) -> f64, set: impl Fn(&mut sf_project::v4::ColorGrade, f64) + Copy, range: std::ops::RangeInclusive<f64>, default: f64, suffix: &str) {
    let mut v = get(&ed.session.pkg.project.config.camera.grade());
    let r = widgets::labeled_slider(ui, label, &mut v, range, Some(default), suffix);
    if r.response.changed() || r.reset {
        set_grade(ed, move |g| set(g, v));
    }
    if r.response.drag_stopped() || r.reset || (r.response.changed() && !r.response.dragged()) {
        push_undo(ed);
    }
}

/// Whisper transcription of the recorded speech (microphone and recorded voice-overs).
fn transcript_ui(ed: &mut Editor, ui: &mut egui::Ui, has_transcript: bool) {
    widgets::hint(ui, "Transcribed on this computer with Whisper. The model is downloaded once.");
    ui.add_space(4.0);
    ui.add_enabled_ui(!ed.transcribe.is_running(), |ui| {
        widgets::title(ui, "AI model");
        let models = sf_transcribe::Model::ALL;
        let idx = models.iter().position(|m| *m == ed.transcribe.model).unwrap_or(0);
        let labels: Vec<&str> = models.iter().map(|m| m.label()).collect();
        if let Some(i) = widgets::segmented(ui, idx, &labels) {
            ed.transcribe.model = models[i];
        }
        widgets::title(ui, "Language");
        widgets::text_field(ui, &mut ed.transcribe.language, "Detect automatically (or en, de, fr …)");
    });
    ui.add_space(8.0);
    if ed.transcribe.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            widgets::hint(ui, &ed.transcribe.status());
        });
        if widgets::secondary_button(ui, "Cancel").clicked() {
            ed.transcribe.cancel();
        }
    } else {
        let label = if has_transcript { "Re-generate transcript" } else { "Generate transcript" };
        if widgets::accent_button(ui, label).clicked() {
            ed.save_if_dirty();
            let path = ed.path.clone();
            ed.transcribe.start(path);
        }
        if has_transcript {
            widgets::hint(ui, "Re-generating replaces the current transcript.");
        }
        match &ed.transcribe.result {
            Some(Ok(0)) => widgets::hint(ui, "No speech was found in the microphone recording or voice-overs."),
            Some(Ok(n)) => widgets::hint(ui, &format!("Transcribed {n} words.")),
            Some(Err(e)) if e == "cancelled" => widgets::hint(ui, "Cancelled."),
            Some(Err(e)) => {
                ui.label(egui::RichText::new(format!("Transcription failed: {e}")).size(13.0).color(crate::theme::p().danger));
            }
            None => {}
        }
    }
}

fn captions_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Captions");
    let has_mic = ed.session.pkg.recording.as_ref().and_then(|r| r.microphone()).is_some_and(|m| !m.sessions.is_empty());
    let has_voice_over = ed.session.pkg.project.scenes.iter().any(|s| !s.voice_overs.is_empty());
    if !has_mic && !has_voice_over {
        widgets::hint(ui, "No microphone recorded. Record a voice-over to caption it.");
        return;
    }
    let has_transcript = !ed.session.captions.is_empty();

    // takes read by the AI voice carry their words; only recorded speech is transcribed
    let has_recorded = has_mic || ed.session.pkg.project.scenes.iter().flat_map(|s| &s.voice_overs).any(|v| v.spoken_voice().is_none());
    if has_recorded {
        transcript_ui(ed, ui, has_transcript);
    } else {
        widgets::hint(ui, "The captions come from the narration script, so there is nothing to transcribe. Record a voice-over or a microphone to transcribe it with Whisper.");
    }
    if !has_transcript {
        return;
    }

    // ── appearance ──
    ui.add_space(8.0);
    let c = ed.session.pkg.project.config.captions.clone();
    let mut on = c.enable_transcript;
    if widgets::toggle_row(ui, "Show captions", "", &mut on) {
        ed.edit(move |p| p.config.captions.enable_transcript = on);
    }
    let mut labels = c.show_sound_labels;
    if widgets::toggle_row(ui, "Sound labels", "Show labels such as [music].", &mut labels) {
        ed.edit(move |p| p.config.captions.show_sound_labels = labels);
    }
    cfg_slider(ed, ui, "Caption size", |p| p.config.captions.size_ratio, |p, v| p.config.captions.size_ratio = v, 0.05..=0.125, Some(0.05), "");

    let pick = |ui: &mut egui::Ui, title: &str, values: &[&str], labels: &[&str], cur: &str| -> Option<String> {
        widgets::title(ui, title);
        widgets::segmented(ui, values.iter().position(|v| *v == cur).unwrap_or(0), labels).map(|i| values[i].to_string())
    };
    if let Some(f) = pick(ui, "Font style", &["sans-serif", "serif", "mono"], &["Sans", "Serif", "Mono"], &c.font) {
        ed.edit(move |p| p.config.captions.font = f);
    }
    // light = white text on black, dark = black text on white
    let dark = c.color.eq_ignore_ascii_case("#000000");
    widgets::title(ui, "Color");
    if let Some(i) = widgets::segmented(ui, dark as usize, &["Light", "Dark"]) {
        let (fg, bg) = if i == 1 { ("#000000", "#ffffff") } else { ("#ffffff", "#000000") };
        ed.edit(move |p| {
            p.config.captions.color = fg.into();
            p.config.captions.background_color = bg.into();
        });
    }
    let positions = [0.0, 0.5, 1.0];
    let pos_idx = positions.iter().position(|y| (c.position01.y - y).abs() < 0.01).unwrap_or(2);
    widgets::title(ui, "Position");
    if let Some(i) = widgets::segmented(ui, pos_idx, &["Top", "Center", "Bottom"]) {
        let y = positions[i];
        ed.edit(move |p| p.config.captions.position01 = sf_project::schema::common::Point::new(0.5, y));
    }
    if let Some(r) = pick(ui, "Word reveal", &["line-by-line", "word-by-word"], &["Line by line", "Word by word"], &c.words_reveal) {
        ed.edit(move |p| p.config.captions.words_reveal = r);
    }
    if c.words_reveal == "word-by-word" {
        if let Some(g) = pick(ui, "Line growth", &["entire-line", "left-anchored", "centered"], &["Full line", "Expanding", "Centered"], &c.line_growth) {
            ed.edit(move |p| p.config.captions.line_growth = g);
        }
    }
    if let Some(e) = pick(ui, "Word animation", &["appear", "fade-in", "slide-in"], &["Appear", "Fade in", "Slide in"], &c.word_entrance) {
        ed.edit(move |p| p.config.captions.word_entrance = e);
    }

    // ── export ──
    ui.add_space(10.0);
    if widgets::secondary_button(ui, "Export captions (.srt / .vtt)…").clicked() {
        let name = format!("{}.srt", ed.session.pkg.project.name);
        if let Some(path) = rfd::FileDialog::new().set_file_name(&name).add_filter("SubRip", &["srt"]).add_filter("WebVTT", &["vtt"]).save_file() {
            // the format follows the extension chosen in the dialog
            let vtt = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vtt"));
            let text = if vtt { ed.session.captions_vtt() } else { ed.session.captions_srt() };
            let msg = match std::fs::write(&path, text) {
                Ok(()) => format!("Saved {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                Err(e) => format!("Could not save captions: {e}"),
            };
            ed.status = Some((msg, std::time::Instant::now()));
        }
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
    let boost = clip.boost_db.unwrap_or_else(|| if clip.track == sf_project::AudioTrack::Microphone { ed.session.pkg.project.config.audio.microphone_boost_db() } else { 0.0 });
    if let Some(db) = boost_ui(ui, boost) {
        let idc = id.clone();
        ed.edit(move |p| {
            let scene = &mut p.scenes[0];
            if let Some(mut cs) = scene.audio_clips() {
                if let Some(c) = cs.iter_mut().find(|c| c.id == idc) {
                    c.boost_db = Some(db);
                }
                scene.set_audio_clips(Some(cs));
            }
        });
        ed.reload_audio();
    }
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
    ui.horizontal_wrapped(|ui| {
        if ui.button("Split at playhead").clicked() {
            ed.split_at_playhead();
        }
        if ui.button("Split to new layer").on_hover_text("Cut at the playhead and move the right-hand part one lane down").clicked() {
            let t = ed.playhead_ms;
            ed.split_audio_clip(&id, t, true);
        }
        if ui.button("Delete").clicked() {
            ed.delete_selected();
        }
    });
    widgets::title(ui, &format!("Layer {}", clip.layer + 1));
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(clip.layer > 0, egui::Button::new("Move up")).clicked() {
            ed.move_lane_item(&id, -1);
        }
        if ui.button("Move down").clicked() {
            ed.move_lane_item(&id, 1);
        }
    });
    widgets::hint(ui, "Clips on one layer never overlap; clips on different layers can, and play together. Drag a clip down onto the empty lane to start a new layer (⌥↑ / ⌥↓).");
}

/// Boost of the selected audio item (a clip or a voice-over take): the one boost control,
/// `boost` now; returns the new boost in dB when a pill is picked.
fn boost_ui(ui: &mut egui::Ui, boost: f64) -> Option<f64> {
    widgets::title(ui, "Boost");
    const BOOSTS_DB: [f64; 4] = [0.0, 6.0, 12.0, 18.0];
    let idx = BOOSTS_DB.iter().position(|b| (b - boost).abs() < 0.5).unwrap_or(usize::MAX);
    let picked = widgets::pills(ui, idx, &["Off", "+6 dB", "+12 dB", "+18 dB"]).map(|i| BOOSTS_DB[i]);
    widgets::hint(ui, "Raises this clip only, with a limiter so loud words do not clip; other clips keep their own level. Try +6 dB first; the volume slider is for fine tuning. Applied in preview and export.");
    picked
}

fn voice_over_item_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(id) = ed.selected_voice_over.clone() else { return };
    let Some(span) = ed.session.voice_over_spans().into_iter().find(|v| v.id == id) else {
        ed.selected_voice_over = None;
        return;
    };
    widgets::title(ui, "Voice-over");
    widgets::hint(ui, &format!("{} – {} on the timeline", crate::editor::fmt_clock(span.output_start_ms), crate::editor::fmt_clock(span.output_end_ms)));
    let idc = id.clone();
    cfg_slider(ed, ui, "Volume", move |p| p.scenes[0].voice_overs.iter().find(|v| v.id == idc).map(|v| v.volume).unwrap_or(1.0), {
        let idc = id.clone();
        move |p, v| {
            if let Some(o) = p.scenes[0].voice_overs.iter_mut().find(|o| o.id == idc) {
                o.volume = v;
            }
        }
    }, 0.0..=2.0, Some(1.0), "");
    let mut muted = span.muted;
    if widgets::toggle_row(ui, "Mute", "", &mut muted) {
        ed.edit_voice_over(&id, move |v| v.is_disabled = muted);
    }
    if let Some(db) = boost_ui(ui, span.boost_db) {
        ed.edit_voice_over(&id, move |v| v.boost_db = db);
        ed.reload_audio();
    }
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(span.contains_output(ed.playhead_ms), egui::Button::new("Split at playhead")).on_hover_text("S").clicked() {
            let t = ed.playhead_ms;
            ed.split_voice_over(&id, t);
        }
        if ui.button("Move to playhead").clicked() {
            let src = ed.session.timeline.output_to_source(ed.playhead_ms).unwrap_or(0.0);
            ed.edit_voice_over(&id, move |v| v.source_start_ms = src);
        }
        if ui.button("Delete").clicked() {
            ed.delete_selected();
        }
    });
    widgets::hint(ui, "Drag the take along the voice-over lane to move it, or drag its edges to trim it. Split it (S) to cut out a part: select the piece and press ⌫. It stays with the frame it starts on when you cut or speed up clips before it.");
    spoken_take_ui(ed, ui, &id);
}

/// A take read by the AI voice: its voice and script, read again on a change.
fn spoken_take_ui(ed: &mut Editor, ui: &mut egui::Ui, id: &str) {
    let Some(take) = ed.session.pkg.project.scenes[0].voice_overs.iter().find(|v| v.id == id) else { return };
    let Some(voice) = take.spoken_voice().map(str::to_string) else { return };
    let text = take.source.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let source_ms = take.source_start_ms;
    ui.add_space(8.0);
    widgets::title(ui, "AI voice");
    let busy = ed.narration.working().is_some();
    let speed = ed.session.pkg.project.config.audio.narration_speed();
    let mut read = None;
    let mut online = false;
    ui.add_enabled_ui(!busy, |ui| {
        let (v, shown) = voice_picker(ui, id, &voice, false);
        online = shown;
        if let Some(v) = v {
            read = Some((v.to_string(), text.clone()));
        }
    });
    let locked = (online || sf_tts::cloud::cloud_voice(&voice).is_some()) && cloud_locked_ui(ed, ui);
    widgets::title(ui, "Script");
    if ed.narration.take_draft.as_ref().is_none_or(|(d, _)| d != id) {
        ed.narration.take_draft = Some((id.to_string(), text.clone()));
    }
    if let Some((_, draft)) = ed.narration.take_draft.as_mut() {
        script_box(ui, draft, "What the voice says");
        let changed = draft.trim() != text.trim() && !draft.trim().is_empty();
        let draft = draft.clone();
        ui.add_enabled_ui(changed && !busy && !(locked && sf_tts::cloud::cloud_voice(&voice).is_some()), |ui| {
            if widgets::accent_button(ui, "Read again").on_hover_text("Replace this take with the new script, in the same place").clicked() {
                read = Some((voice.clone(), draft));
            }
        });
    }
    if let Some((voice, text)) = read {
        let req = crate::narration::TakeRequest { text, voice, speed, replaces: Some(id.to_string()), source_ms };
        ed.narration.read(ed.path.clone(), vec![req]);
    }
    // a download it waits for shows under Narration → Voice model
    if ed.narration.rereading() == Some(id) && !ed.narration.is_downloading() {
        narration_status(ed, ui);
    }
}

/// What the narration job is doing, with Cancel.
fn narration_status(ed: &mut Editor, ui: &mut egui::Ui) {
    let Some(status) = ed.narration.status() else { return };
    ui.add_space(4.0);
    // a download with a known size has its bar; the spinner is only for work without one
    let download = match ed.narration.progress.clone().filter(|_| ed.narration.is_downloading()) {
        Some(sf_tts::Progress::Downloading { done, total: Some(t) }) => Some((done, t)),
        _ => None,
    };
    ui.horizontal(|ui| {
        if download.is_none() {
            ui.spinner();
        }
        widgets::hint(ui, &status);
    });
    if let Some((done, t)) = download {
        ui.add(egui::ProgressBar::new((done as f64 / t.max(1) as f64) as f32).desired_height(6.0).corner_radius(2.0).fill(theme::p().accent));
    }
    if widgets::secondary_button(ui, "Cancel").clicked() {
        ed.narration.cancel();
    }
}

/// The AI voices: which engine (on this computer, or ElevenLabs), then that engine's voices in
/// two rows, male and female. Returns the voice picked and whether the ElevenLabs voices are
/// shown. Switching engine picks its first voice when `pick_on_switch` (the panel's voice,
/// nothing is read); otherwise (a take, where a pick reads it again) it only shows the other
/// engine's voices until one is picked. Without Pro the ElevenLabs voices are shown but not
/// picked.
fn voice_picker(ui: &mut egui::Ui, id_salt: &str, current: &str, pick_on_switch: bool) -> (Option<&'static str>, bool) {
    let id = ui.id().with(("voice_engine", id_salt));
    let current_online = sf_tts::cloud::cloud_voice(current).is_some();
    let locked = crate::narration::cloud_locked();
    // the engine shown: the current voice's, unless switched here without picking yet
    let shown: Option<(String, bool)> = ui.data(|d| d.get_temp(id));
    let online = shown.filter(|(v, _)| v == current).map_or(current_online, |(_, o)| o);
    if let Some(i) = widgets::segmented(ui, online as usize, &["On this computer", "ElevenLabs"]) {
        let online = i == 1;
        ui.data_mut(|d| d.insert_temp(id, (current.to_string(), online)));
        let picked = (pick_on_switch && !(online && locked)).then(|| if online { sf_tts::cloud::CLOUD_VOICES[0].id } else { sf_tts::DEFAULT_VOICE });
        return (picked, online);
    }
    let voices: Vec<(&'static str, &'static str, bool)> = if online {
        sf_tts::cloud::CLOUD_VOICES.iter().map(|v| (v.id, v.name, v.female)).collect()
    } else {
        sf_tts::VOICES.iter().map(|v| (v.id, v.name, v.female)).collect()
    };
    let mut picked = None;
    ui.add_enabled_ui(!(online && locked), |ui| {
        for female in [false, true] {
            let row: Vec<_> = voices.iter().filter(|v| v.2 == female).collect();
            let names: Vec<&str> = row.iter().map(|v| v.1).collect();
            ui.add_space(4.0);
            ui.label(RichText::new(if female { "Female" } else { "Male" }).size(13.0).color(theme::p().text_dim));
            let idx = row.iter().position(|v| v.0 == current).unwrap_or(usize::MAX);
            if let Some(i) = widgets::pills(ui, idx, &names) {
                picked = Some(row[i].0);
            }
        }
    });
    (picked, online)
}

/// Under the ElevenLabs voices without Pro: what they need, and the way to Pro (the agent
/// panel's credits menu). True when shown.
fn cloud_locked_ui(ed: &mut Editor, ui: &mut egui::Ui) -> bool {
    if !crate::narration::cloud_locked() {
        return false;
    }
    ui.add_space(4.0);
    widgets::hint(ui, "ElevenLabs voices are part of Pro, with the agent: US$20/month for 1,000 credits a month (about 7 a minute of narration). The voices on this computer stay free.");
    if widgets::accent_button(ui, "Get Pro").on_hover_text("Upgrade or sign in from the agent panel").clicked() {
        ed.section = Section::Agent;
    }
    true
}

/// A few lines of script.
fn script_box(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    let pal = theme::p();
    egui::Frame::NONE.fill(pal.bg).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(egui::CornerRadius::same(2)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(text).hint_text(RichText::new(hint).color(pal.text_faint)).font(egui::FontId::proportional(14.0)).text_color(pal.text).frame(egui::Frame::NONE).desired_rows(5).desired_width(f32::INFINITY));
    });
}

/// Narration: the AI voice (Kokoro, on this computer), its model, and text read into
/// voice-over takes at the playhead.
fn narration_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let pal = theme::p();
    widgets::title(ui, "Narration");
    widgets::hint(ui, "An AI voice reads what you write. Each reading is a voice-over take with captions. The agent narrates with the same voice and speed.");

    let work = ed.narration.working();
    // each run's status shows once, by what started it (a download under the model, even
    // when a reading started it; a take read again in that take's section while it is
    // selected)
    let downloading = ed.narration.is_downloading();
    let reread_shown = ed.narration.rereading().is_some_and(|id| ed.selected_voice_over.as_deref() == Some(id));
    let here = |w: crate::narration::Work| !downloading && !reread_shown && work == Some(w);
    let (by_preview, by_add) = (here(crate::narration::Work::Preview), here(crate::narration::Work::Takes(1)) && ed.narration.rereading().is_none());
    let by_takes = !downloading && !reread_shown && !by_add && matches!(work, Some(crate::narration::Work::Takes(_)));

    // ── engine and voice ──
    let audio = &ed.session.pkg.project.config.audio;
    let voice = audio.narration_voice.clone().filter(|v| sf_tts::voice_name(v).is_some()).unwrap_or_else(|| sf_tts::DEFAULT_VOICE.to_string());
    let online = sf_tts::cloud::cloud_voice(&voice);
    let speed = audio.narration_speed();
    widgets::title(ui, "Voice");
    let (picked, shown_online) = voice_picker(ui, "panel", &voice, true);
    if let Some(v) = picked {
        ed.edit(move |p| p.config.audio.narration_voice = Some(v.to_string()));
    }
    // a voice that cannot be read here: the ElevenLabs one chosen before signing out
    let locked = (shown_online || online.is_some()) && cloud_locked_ui(ed, ui);
    if !locked {
        ui.add_space(4.0);
        match online {
            Some(v) => widgets::hint(ui, &format!("{}, {}. Studio quality, read online: about 8 credits per 1,000 characters (around 7 a minute), from the credits you share with the agent.", v.name, v.style)),
            None => widgets::hint(ui, "Free and private: read on this computer, works offline."),
        }
    }
    let blocked = locked && online.is_some();

    ui.add_space(4.0);
    let playing = ed.narration.is_playing();
    let label = if playing { "Stop" } else { "Preview" };
    if ui.add_enabled(work.is_none() && !blocked, egui::Button::new(label)).on_hover_text("Hear a line in this voice").clicked() {
        if playing {
            ed.narration.stop_preview();
        } else {
            ed.narration.preview(&voice, speed);
        }
    }
    if by_preview {
        narration_status(ed, ui);
    }
    let r = sf_project::v4::AudioConfig::NARRATION_SPEEDS;
    cfg_slider(ed, ui, "Speed", |p| p.config.audio.narration_speed(), |p, v| p.config.audio.narration_speed = Some((v * 20.0).round() / 20.0), r, Some(1.0), "×");

    // ── the on-device model ──
    if online.is_none() || downloading {
        widgets::title(ui, "Voice model");
        let size_mb = sf_tts::download_size() as f64 / 1e6;
        if downloading {
            narration_status(ed, ui);
        } else if ed.narration.is_downloaded() {
            widgets::hint(ui, &format!("Kokoro is downloaded ({size_mb:.0} MB) and works offline."));
            if ui.add_enabled_ui(work.is_none(), |ui| widgets::secondary_button(ui, "Remove download")).inner.on_hover_text("Free the space; it is downloaded again when you next narrate").clicked() {
                ed.narration.remove();
            }
        } else {
            widgets::hint(ui, &format!("Kokoro, about {size_mb:.0} MB, is downloaded once. Without it, the first narration downloads it."));
            if widgets::accent_button(ui, "Download voice model").clicked() {
                ed.narration.download();
            }
        }
    }

    // ── a new take ──
    widgets::title(ui, "Script");
    script_box(ui, &mut ed.narration.draft, "Type what the voice should say…");
    widgets::hint(ui, "A blank line makes a longer pause. Numbers like 2× or 1.5 are read out.");
    ui.add_space(6.0);
    let can = !ed.narration.draft.trim().is_empty() && work.is_none() && !blocked;
    let at = ed.playhead_ms;
    ui.add_enabled_ui(can, |ui| {
        let tip = match online {
            Some(_) => format!("Read the script into a voice-over take starting at the playhead (about {:.0} credits)", sf_tts::cloud::credits_for(ed.narration.draft.trim().chars().count()).ceil()),
            None => "Read the script into a voice-over take starting at the playhead".to_string(),
        };
        if widgets::accent_button(ui, &format!("Add at {}", crate::editor::fmt_clock(at))).on_hover_text(tip).clicked() {
            let source_ms = ed.session.timeline.output_to_source(at).unwrap_or(0.0);
            let req = crate::narration::TakeRequest { text: ed.narration.draft.trim().to_string(), voice: voice.clone(), speed, replaces: None, source_ms };
            ed.narration.read(ed.path.clone(), vec![req]);
        }
    });
    if by_add {
        narration_status(ed, ui);
    }

    // ── takes already read ──
    let takes: Vec<(String, String, String, f64)> = ed.session.pkg.project.scenes[0]
        .voice_overs
        .iter()
        .filter_map(|v| Some((v.id.clone(), v.spoken_voice()?.to_string(), v.source.get("text")?.as_str()?.to_string(), v.source_start_ms)))
        .collect();
    if !takes.is_empty() {
        widgets::title(ui, "Narration takes");
        let other = takes.iter().filter(|t| t.1 != voice).count();
        let name = sf_tts::voice_name(&voice).map_or(voice.clone(), str::to_string);
        widgets::hint(ui, &match (takes.len(), other) {
            (n, 0) => format!("{n} take{} read by {name}. Select one on the timeline to change its script or voice.", if n == 1 { "" } else { "s" }),
            (n, o) => format!("{o} of {n} takes use another voice."),
        });
        ui.add_enabled_ui(work.is_none() && !blocked, |ui| {
            if widgets::secondary_button(ui, &format!("Read all with {name}")).on_hover_text("Read every narration take again with this voice and speed, in the same places").clicked() {
                let reqs = takes.into_iter().map(|(id, _, text, source_ms)| crate::narration::TakeRequest { text, voice: voice.clone(), speed, replaces: Some(id), source_ms }).collect();
                ed.narration.read(ed.path.clone(), reqs);
            }
        });
    }
    if by_takes {
        narration_status(ed, ui);
    }
    if let Some(e) = &ed.narration.error {
        ui.add_space(6.0);
        ui.label(RichText::new(e).size(13.0).color(pal.danger));
    }
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
    widgets::hint(ui, "Too quiet? Select a clip or voice-over on the timeline and use Boost: it raises that clip only.");
    widgets::title(ui, "System audio");
    if ed.session.source.system_audio.is_empty() {
        widgets::hint(ui, "No system audio recorded");
    }
    let mut mute_sys = ed.session.pkg.project.config.audio.mute_system_audio;
    if widgets::toggle_row(ui, "Mute system audio", "", &mut mute_sys) {
        ed.edit(move |p| p.config.audio.mute_system_audio = mute_sys);
        ed.reload_audio();
    }
    // with a music part selected, every control below changes that part only
    let part = ed.selected_music.clone().and_then(|id| sf_engine::lanes::music_clips(&ed.session.pkg.project, &ed.session.timeline).into_iter().find(|c| c.id == id));
    let audio_cfg = ed.session.pkg.project.config.audio.clone();
    if let Some(c) = &part {
        widgets::title(ui, "Background audio · selected part");
        widgets::hint(ui, &format!("Changes apply to the music part at {} – {} only.", crate::editor::fmt_clock(c.output_start_ms), crate::editor::fmt_clock(c.output_end_ms())));
        if widgets::secondary_button(ui, "Change all the music instead").clicked() {
            ed.selected_music = None;
        }
    } else {
        widgets::title(ui, "Background audio");
    }
    let current = match &part {
        Some(c) => c.track(&audio_cfg).map(str::to_string),
        None => audio_cfg.background_audio_file_name.clone(),
    };
    let library = ed.session.assets.list_background_audio();
    let assets = ed.session.pkg.path.join("assets");
    // a track of your own (added, found on Jamendo or imported from a link) is not in the library
    if let Some(cur) = current.as_deref().filter(|c| !library.iter().any(|n| sf_session::assets::background_audio_key(n) == sf_session::assets::background_audio_key(c))) {
        match crate::music::credit(&assets, cur) {
            Some(credit) => music_credit_ui(ui, &credit),
            None => widgets::hint(ui, &format!("Playing your track \"{}\".", sf_session::assets::background_audio_label(cur))),
        }
        ui.add_space(4.0);
    }
    // where the music comes from, as Background has Wallpaper / … / Unsplash
    use crate::music::Source;
    ed.music.ensure_library();
    let sources = [Source::Library, Source::Jamendo, Source::Favorites, Source::Recent, Source::Link];
    let favorites = match ed.music.library.favorites.len() {
        0 => "Favorites".to_string(),
        n => format!("Favorites ({n})"),
    };
    if let Some(i) = widgets::segmented(ui, sources.iter().position(|s| *s == ed.music.source).unwrap_or(0), &["Library", "Jamendo", &favorites, "Recent", "Link"]) {
        ed.music.source = sources[i];
    }
    ui.add_space(8.0);
    let mut picked = music_source_ui(ed, ui, &assets, current.as_deref());
    if ed.music.source == Source::Library {
        if library.is_empty() {
            widgets::hint(ui, "No music library on this Mac yet. Add your own track with \"Add background audio…\" below.");
        }
        let mut categories: Vec<String> = library.iter().filter_map(|n| n.split('/').next().map(str::to_string)).collect();
        categories.dedup();
        let none_sel = current.is_none();
        if ui.selectable_label(none_sel, if part.is_some() { "None (delete this part)" } else { "None" }).clicked() && !none_sel {
            match &part {
                Some(_) => ed.delete_selected(),
                None => ed.remove_music(),
            }
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
                        picked = Some(track.clone());
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
                    picked = Some(name);
                }
            }
        }
    }
    if let Some(track) = picked {
        set_music_track(ed, part.as_ref().map(|c| c.id.as_str()), track);
        ed.preview_background_audio();
    }
    match &part {
        Some(c) => {
            let id = c.id.clone();
            let mut mute = c.muted;
            if widgets::toggle_row(ui, "Mute this part", "", &mut mute) {
                ed.edit_music_clip(&id, move |m| m.muted = mute);
            }
            let (get_id, set_id) = (id.clone(), id.clone());
            cfg_slider(ed, ui, "Volume of this part", move |p| part_of(p, &get_id).map(|m| m.volume.unwrap_or(p.config.audio.background_audio_volume)).unwrap_or(0.0), move |p, v| with_parts(p, |m| if m.id == set_id { m.volume = Some(v) }), 0.0..=1.0, None, "");
        }
        None => {
            let mut mute_bg = audio_cfg.mute_background_audio;
            if widgets::toggle_row(ui, "Mute background audio", "", &mut mute_bg) {
                // all the music: parts muted on their own follow too
                ed.edit(move |p| {
                    p.config.audio.mute_background_audio = mute_bg;
                    with_parts(p, |m| m.muted = false);
                });
            }
            cfg_slider(ed, ui, "Background audio volume", |p| p.config.audio.background_audio_volume, |p, v| {
                p.config.audio.background_audio_volume = v;
                with_parts(p, |m| m.volume = None);
            }, 0.0..=1.0, Some(0.05), "");
        }
    }
}

/// Who made the Jamendo track in use, its licence, and a button to copy the credit line.
fn music_credit_ui(ui: &mut egui::Ui, credit: &crate::music::Credit) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new(format!("\"{}\" by {}", credit.title, credit.artist)).color(theme::p().text));
        if let Some(license) = credit.license() {
            match &credit.license_url {
                Some(url) => browser_link(ui, &license, url),
                None => {
                    ui.label(RichText::new(license).color(theme::p().text_dim));
                }
            }
        }
        browser_link(ui, "on Jamendo", credit.url.as_deref().unwrap_or("https://www.jamendo.com/"));
    });
    if credit.is_noncommercial() {
        ui.label(RichText::new("Non-commercial licence: only for a video that earns nothing. No ads, sponsors, paid courses, or promoting a product or business.").size(13.0).color(theme::p().danger));
    }
    widgets::hint(ui, "Creative Commons music: put this credit in the video's description wherever you publish it.");
    if widgets::secondary_button(ui, "Copy credit").on_hover_text(credit.text()).clicked() {
        ui.ctx().copy_text(credit.text());
    }
}

/// One line of text cut with "…" to fit `max_width`.
fn one_line(ui: &egui::Ui, text: &str, size: f32, color: egui::Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// What happened on a [`track_row`].
struct TrackRow {
    response: egui::Response,
    /// The heart was clicked (the row itself was not).
    heart: bool,
}

/// A Jamendo result, laid out like a music player's track list: album cover, title over
/// "artist · licence", length, a heart for favourites, and a link to the track's page
/// (Jamendo's API terms ask for one on every track). Hovering shows a play mark on the cover; the
/// track in use has its title in the accent colour. `downloading`: `Some(progress)` while this
/// track downloads.
fn track_row(ui: &mut egui::Ui, track: &crate::music::Track, cover: Option<&std::path::Path>, playing: bool, favorite: bool, downloading: Option<Option<f32>>) -> TrackRow {
    let pal = theme::p();
    const H: f32 = 52.0;
    const COVER: f32 = 40.0;
    const PAD: f32 = 6.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), H), egui::Sense::click());
    let link_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - PAD - 10.0, rect.center().y), egui::vec2(24.0, 24.0));
    let link = ui.interact(link_rect, resp.id.with("link"), egui::Sense::click()).on_hover_text("Open on Jamendo").on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if link.clicked() {
        crate::os::open(track.share_url.as_deref().unwrap_or("https://www.jamendo.com/"));
    }
    let heart_rect = link_rect.translate(egui::vec2(-26.0, 0.0));
    let heart = ui.interact(heart_rect, resp.id.with("heart"), egui::Sense::click()).on_hover_text(if favorite { "Remove from favorites" } else { "Add to favorites" }).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered() || link.hovered() || heart.hovered();
    let painter = ui.painter();
    let radius = egui::CornerRadius::same(widgets::BUTTON_RADIUS);
    if playing {
        painter.rect_filled(rect, radius, pal.card_3);
    } else if hovered {
        painter.rect_filled(rect, radius, pal.card_2);
    }
    // cover
    let cover_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + PAD, rect.center().y - COVER / 2.0), egui::vec2(COVER, COVER));
    let texture = cover.and_then(|p| match ui.ctx().try_load_texture(&crate::os::texture_uri(p), egui::TextureOptions::LINEAR, egui::load::SizeHint::Size { width: 100, height: 100, maintain_aspect_ratio: true }) {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
        _ => None,
    });
    match texture {
        Some(t) => {
            egui::Image::from_texture(t).corner_radius(radius).paint_at(ui, cover_rect);
        }
        None => {
            painter.rect_filled(cover_rect, radius, pal.card_2);
            widgets::paint_icon(painter, cover_rect.shrink(11.0), widgets::Icon::Audio, pal.text_faint);
        }
    }
    match downloading {
        Some(progress) => download_ring(ui, cover_rect, progress),
        None if hovered && !playing => {
            painter.rect_filled(cover_rect, radius, egui::Color32::from_black_alpha(140));
            widgets::paint_icon(painter, cover_rect.shrink(12.0), widgets::Icon::Play, egui::Color32::WHITE);
        }
        None if playing => {
            painter.rect_filled(cover_rect, radius, egui::Color32::from_black_alpha(120));
            widgets::paint_icon(painter, cover_rect.shrink(12.0), widgets::Icon::Waveform, pal.accent);
        }
        None => {}
    }
    // link
    widgets::paint_icon(painter, link_rect.shrink(6.0), widgets::Icon::OpenExternal, if link.hovered() { pal.text } else { pal.text_faint });
    // the heart: filled in the accent colour for a favourite, otherwise only while hovering
    if favorite {
        widgets::paint_icon(painter, heart_rect.shrink(5.0), widgets::Icon::HeartFilled, pal.accent);
    } else if hovered {
        widgets::paint_icon(painter, heart_rect.shrink(5.0), widgets::Icon::Heart, if heart.hovered() { pal.text } else { pal.text_faint });
    }
    // length, right-aligned before the heart
    let length = one_line(ui, &crate::music::fmt_duration(track.duration), 12.5, pal.text_dim, 60.0);
    let length_pos = egui::pos2(heart_rect.left() - 6.0 - length.size().x, rect.center().y - length.size().y / 2.0);
    // title over artist · licence
    let text_left = cover_rect.right() + 12.0;
    let text_w = length_pos.x - 12.0 - text_left;
    let credit = crate::music::Credit { title: track.name.clone(), artist: track.artist.clone(), url: None, license_url: track.license_url.clone() };
    let sub = match downloading {
        Some(Some(f)) => format!("Downloading… {:.0} %", f * 100.0),
        Some(None) => "Downloading…".to_string(),
        None => [Some(track.artist.clone()), credit.license()].into_iter().flatten().collect::<Vec<_>>().join(" · "),
    };
    let title = one_line(ui, &track.name, 14.0, if playing { pal.accent } else { pal.text }, text_w);
    let sub = one_line(ui, &sub, 12.5, pal.text_dim, text_w);
    // the full names only when they were cut to fit
    let cut = title.size().x >= text_w - 1.0 || sub.size().x >= text_w - 1.0;
    let (title_h, top) = (title.size().y, rect.center().y - (title.size().y + 2.0 + sub.size().y) / 2.0);
    let painter = ui.painter();
    painter.galley(egui::pos2(text_left, top), title, pal.text);
    painter.galley(egui::pos2(text_left, top + title_h + 2.0), sub, pal.text_dim);
    painter.galley(length_pos, length, pal.text_dim);
    let response = if cut { resp.on_hover_text(format!("\"{}\" by {}{}", track.name, track.artist, credit.license().map(|l| format!(" · {l}")).unwrap_or_default())) } else { resp };
    TrackRow { response, heart: heart.clicked() }
}

/// Audio → Background audio's Jamendo, Favorites, Recent and Link tabs (Library is drawn by
/// [`audio_ui`]). Downloads finish whichever tab is showing. Returns the file name (in the
/// project's `assets/`) of a track that finished downloading.
fn music_source_ui(ed: &mut Editor, ui: &mut egui::Ui, assets: &std::path::Path, current: Option<&str>) -> Option<String> {
    use crate::music::{Job, Source};
    let ctx = ui.ctx().clone();
    let done = ed.music.poll(&ctx).map(|a| a.file_name);
    match ed.music.source {
        Source::Library => {}
        Source::Link => {
            let importing = ed.music.applying == Some(Job::Link);
            let free = ed.music.applying.is_none();
            let import = ui
                .allocate_ui_with_layout(egui::vec2(ui.available_width(), 38.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = ui.add_enabled_ui(free, |ui| widgets::secondary_button(ui, "Import")).inner.clicked();
                    let field = widgets::text_field(ui, &mut ed.music.link, "https://…/track.mp3");
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    button || (enter && free)
                })
                .inner;
            if import && !ed.music.link.trim().is_empty() {
                ed.music.import_link(&ctx, assets.to_path_buf());
            }
            if importing {
                widgets::hint(ui, &ed.music.progress().map_or_else(|| "Downloading…".to_string(), |f| format!("Downloading… {:.0} %", f * 100.0)));
            }
            if let Some(e) = &ed.music.link_error {
                ui.label(RichText::new(e).size(13.0).color(theme::p().danger));
            }
            widgets::hint(ui, "A direct link to an MP3, M4A, AAC, WAV, OGG, FLAC or AIFF file (Dropbox share links work too). Use only music you have the rights to.");
        }
        source => {
            ed.music.ensure_started(&ctx);
            if source == Source::Jamendo {
                // the icon first, right to left, so the field takes the rest of the row
                let searched = ui
                    .allocate_ui_with_layout(egui::vec2(ui.available_width(), 38.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = widgets::square_icon_button(ui, widgets::Icon::Search, "Search").clicked();
                        let field = widgets::text_field(ui, &mut ed.music.query, "Search Jamendo music");
                        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        button || enter
                    })
                    .inner;
                widgets::hint(ui, "Free in any video with credit to the artist. Leave it empty for popular tracks.");
                let mut instrumental = ed.music.instrumental;
                let filtered = widgets::toggle_row(ui, "Instrumental only", "", &mut instrumental);
                ed.music.instrumental = instrumental;
                let mut noncommercial = ed.music.noncommercial;
                let licensed = widgets::toggle_row(ui, "Non-commercial too", "Only for videos that earn nothing.", &mut noncommercial);
                ed.music.noncommercial = noncommercial;
                if searched || filtered || licensed {
                    ed.music.search(&ctx, 1);
                }
                if let Some(e) = &ed.music.error {
                    widgets::hint(ui, &format!("Jamendo: {e}"));
                }
                if ed.music.results.is_empty() && ed.music.error.is_none() {
                    widgets::hint(ui, if ed.music.is_searching() { "Loading music…" } else { "No music found." });
                }
            } else if ed.music.tracks().is_empty() {
                widgets::hint(ui, match source {
                    Source::Favorites => "No favorites yet. Tap the heart on a Jamendo track to keep it here, for every project.",
                    _ => "Jamendo tracks you add to a video show here.",
                });
            }
            let current = current.map(|c| c.rsplit('/').next().unwrap_or(c));
            let (mut pick, mut heart) = (None, None);
            ui.add_space(4.0);
            for track in ed.music.tracks() {
                let playing = current == Some(crate::music::track_file_name(track).as_str());
                let busy = ed.music.applying.as_ref() == Some(&Job::Track(track.id.clone()));
                let row = track_row(ui, track, ed.music.thumb(&track.id), playing, ed.music.library.is_favorite(&track.id), busy.then(|| ed.music.progress()));
                if row.heart {
                    heart = Some(track.clone());
                } else if row.response.clicked() && !playing && ed.music.applying.is_none() {
                    pick = Some(track.clone());
                }
            }
            if let Some(track) = heart {
                ed.music.library.toggle_favorite(&track);
            }
            if let Some(track) = pick {
                ed.music.apply_track(&ctx, track, assets.to_path_buf());
            }
            if source == Source::Jamendo && ed.music.can_load_more() && widgets::secondary_button(ui, "Load more").clicked() {
                ed.music.load_more(&ctx);
            }
        }
    }
    if ed.music.is_busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
    done
}

/// Music part `id` of a project, when the music is saved as parts.
fn part_of(p: &sf_project::Project, id: &str) -> Option<sf_project::MusicClip> {
    use sf_project::SceneMusicExt;
    p.scenes[0].music_clips()?.into_iter().find(|m| m.id == id)
}

/// Change every music part of a project (no-op before the music is saved as parts).
fn with_parts(p: &mut sf_project::Project, mut f: impl FnMut(&mut sf_project::MusicClip)) {
    use sf_project::SceneMusicExt;
    if let Some(mut clips) = p.scenes[0].music_clips() {
        clips.iter_mut().for_each(&mut f);
        p.scenes[0].set_music_clips(Some(clips));
    }
}

/// Play `track` in music part `part`, or in all the music (dropping the parts' own tracks).
fn set_music_track(ed: &mut Editor, part: Option<&str>, track: String) {
    match part {
        Some(id) => ed.edit_music_clip(id, move |m| {
            m.file_name = Some(track);
            m.muted = false;
        }),
        None => ed.edit(move |p| {
            p.config.audio.background_audio_file_name = Some(track);
            p.config.audio.mute_background_audio = false;
            // Screen Studio's 5 % default is near-silent; start at an audible level
            if p.config.audio.background_audio_volume < 0.2 {
                p.config.audio.background_audio_volume = 0.3;
            }
            with_parts(p, |m| m.file_name = None);
        }),
    }
    ed.reload_audio();
}

fn shortcuts_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Keyboard Shortcuts");
    if ed.session.cursor_track.key_presses.is_empty() {
        widgets::hint(ui, "No keyboard shortcuts during recording");
        ui.disable();
    } else {
        let n = ed.session.shortcuts.list.len();
        widgets::hint(ui, &match n {
            0 => "No shortcuts to show: turn on single keys to show plain key presses.".to_string(),
            1 => "1 shortcut in this recording.".to_string(),
            n => format!("{n} shortcuts in this recording."),
        });
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
    use sf_engine::device::{Finish, Frame, FINISHES, KINDS};
    widgets::title(ui, "Device frame");
    widgets::hint(ui, "Show the recording inside a phone or tablet. The frame takes the recording's shape, so it looks right with a phone or tablet recording (the iOS Simulator, iPhone Mirroring, an Android emulator). Tall recordings stand upright; wide ones lie on their side.");
    let dev = &ed.session.pkg.project.config.device;
    let current = dev.frame_key.as_deref().filter(|_| dev.enable_mockup).and_then(Frame::parse);
    let mut labels = vec!["None"];
    labels.extend(KINDS.iter().map(|k| k.2));
    let idx = current.and_then(|f| KINDS.iter().position(|k| k.0 == f.kind)).map_or(0, |i| i + 1);
    if let Some(i) = widgets::segmented(ui, idx, &labels) {
        let key = (i > 0).then(|| Frame { kind: KINDS[i - 1].0, finish: current.map_or(Finish::Black, |f| f.finish) }.key());
        ed.edit(move |p| {
            p.config.device.enable_mockup = true;
            p.config.device.frame_key = key;
        });
    }
    if let Some(f) = current {
        widgets::title(ui, "Finish");
        let names: Vec<&str> = FINISHES.iter().map(|x| x.2).collect();
        let at = FINISHES.iter().position(|x| x.0 == f.finish).unwrap_or(0);
        if let Some(i) = widgets::segmented(ui, at, &names) {
            let key = Frame { finish: FINISHES[i].0, ..f }.key();
            ed.edit(move |p| p.config.device.frame_key = Some(key));
        }
        widgets::hint(ui, "The frame replaces the screen's rounded corners and inset; its shadow follows Shadow in General.");
    }
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


enum CursorPreview {
    /// A texture uri: the recorded pointer image or a drawn set's arrow.
    Image(String),
    /// A drawn arrow icon (no image available).
    Arrow,
}

/// The arrow of a drawn cursor set, encoded once as a PNG under a `bytes://` uri.
fn cursor_set_preview(ctx: &egui::Context, set: &str) -> CursorPreview {
    let uri = format!("bytes://cursor-set-{set}.png");
    if ctx.try_load_bytes(&uri).is_ok() {
        return CursorPreview::Image(uri);
    }
    let Some(s) = cursor_sets::draw(set, "arrow") else { return CursorPreview::Arrow };
    let Some(img) = image::RgbaImage::from_raw(s.width, s.height, s.rgba) else { return CursorPreview::Arrow };
    let mut png = std::io::Cursor::new(Vec::new());
    if img.write_to(&mut png, image::ImageFormat::Png).is_err() {
        return CursorPreview::Arrow;
    }
    ctx.include_bytes(uri.clone(), png.into_inner());
    CursorPreview::Image(uri)
}

/// A cursor-style tile: a preview of the pointer above its label.
fn cursor_tile(ui: &mut egui::Ui, preview: CursorPreview, label: &str, selected: bool) -> egui::Response {
    let size = egui::vec2(72.0, 66.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let pal = theme::p();
    let fill = if selected { pal.card_3 } else if resp.hovered() { pal.card_2 } else { pal.card };
    let stroke = if selected { egui::Stroke::new(1.5, pal.accent) } else { egui::Stroke::new(1.0, pal.border) };
    ui.painter().rect(rect, egui::CornerRadius::same(widgets::BUTTON_RADIUS), fill, stroke, egui::StrokeKind::Inside);
    let icon = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 26.0), egui::vec2(28.0, 28.0));
    match preview {
        CursorPreview::Image(uri) => {
            if let Ok(egui::load::TexturePoll::Ready { texture }) = ui.ctx().try_load_texture(&uri, egui::TextureOptions::LINEAR, egui::load::SizeHint::Size { width: 64, height: 64, maintain_aspect_ratio: true }) {
                let (w, h) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
                let s = (icon.width() / w).min(icon.height() / h);
                let r = egui::Rect::from_center_size(icon.center(), egui::vec2(w * s, h * s));
                ui.painter().image(texture.id, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            } else {
                widgets::paint_icon(ui.painter(), icon.shrink(4.0), widgets::Icon::Cursor, pal.text);
            }
        }
        CursorPreview::Arrow => widgets::paint_icon(ui.painter(), icon.shrink(4.0), widgets::Icon::Cursor, pal.text),
    }
    ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 11.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), if selected { pal.text } else { pal.text_dim });
    resp
}

/// A square, centre-cropped wallpaper thumbnail.
/// A text link that opens `url` in the default browser. eframe is built without egui's own
/// URL handling, so `hyperlink_to` would do nothing; links go through `os::open` like the rest
/// of the app.
fn browser_link(ui: &mut egui::Ui, text: &str, url: &str) {
    if ui.link(text).on_hover_text(url).clicked() {
        crate::os::open(url);
    }
}

/// Background → Unsplash: search, a grid of results, and the credit for the current photo.
fn unsplash_ui(ed: &mut Editor, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ed.unsplash.ensure_started(&ctx);
    if let Some(applied) = ed.unsplash.poll(&ctx) {
        let value = crate::unsplash::background_value(&applied);
        ed.edit(move |p| {
            let bg = &mut p.config.styles.background;
            bg.kind = sf_project::v4::background_type::UNSPLASH.into();
            bg.unsplash = Some(value);
        });
    }
    widgets::title(ui, "Unsplash");
    let current = ed.session.pkg.project.config.styles.background.unsplash.clone();
    let current_id = current.as_ref().and_then(|v| v.get("id")).and_then(|v| v.as_str()).map(str::to_string);
    // credit the photographer, as Unsplash asks (links carry its referral parameters)
    if let Some(v) = &current {
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new("Photo by").color(theme::p().text_dim));
            match (get("author"), get("authorUrl")) {
                (Some(name), Some(url)) => {
                    browser_link(ui, &name, &crate::unsplash::referral(&url));
                }
                (Some(name), None) => {
                    ui.label(name);
                }
                _ => {
                    ui.label("an Unsplash photographer");
                }
            }
            ui.label(RichText::new("on").color(theme::p().text_dim));
            browser_link(ui, "Unsplash", &crate::unsplash::referral(&get("photoUrl").unwrap_or_else(|| "https://unsplash.com/".into())));
        });
        ui.add_space(4.0);
    }
    // the button first, right to left, so the field takes the rest of the row
    let searched = ui
        .allocate_ui_with_layout(egui::vec2(ui.available_width(), 38.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = widgets::secondary_button(ui, "Search").clicked();
            let field = widgets::text_field(ui, &mut ed.unsplash.query, "Search photos");
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            button || enter
        })
        .inner;
    widgets::hint(ui, "Leave it empty to browse Unsplash's wallpapers.");
    if searched {
        ed.unsplash.search(&ctx, 1);
    }
    if let Some(e) = &ed.unsplash.error {
        widgets::hint(ui, &format!("Unsplash: {e}"));
    }
    if ed.unsplash.results.is_empty() && ed.unsplash.error.is_none() {
        widgets::hint(ui, if ed.unsplash.is_busy() { "Loading photos…" } else { "No photos found." });
    }
    let cols = 3;
    let gap = 8.0;
    let tile = ((ui.available_width() - (cols as f32 - 1.0) * gap) / cols as f32).floor();
    let mut pick = None;
    egui::Grid::new("unsplash").spacing([gap, gap]).show(ui, |ui| {
        for (i, photo) in ed.unsplash.results.iter().enumerate() {
            let selected = current_id.as_deref() == Some(photo.id.as_str());
            let author = photo.author.clone().unwrap_or_default();
            let tip = if ed.unsplash.applying.as_deref() == Some(photo.id.as_str()) { "Downloading…".to_string() } else { format!("Photo by {author}") };
            let resp = wallpaper_tile(ui, ed.unsplash.thumb(&photo.id), &tip, tile, selected);
            if ed.unsplash.applying.as_deref() == Some(photo.id.as_str()) {
                download_ring(ui, resp.rect, ed.unsplash.progress());
            } else if resp.clicked() && !selected {
                pick = Some(photo.clone());
            }
            if (i + 1) % cols == 0 {
                ui.end_row();
            }
        }
    });
    if let Some(photo) = pick {
        let assets = ed.session.pkg.path.join("assets");
        ed.unsplash.apply(&ctx, photo, assets);
    }
    if ed.unsplash.can_load_more() && widgets::secondary_button(ui, "Load more").clicked() {
        ed.unsplash.load_more(&ctx);
    }
    if ed.unsplash.is_busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
}

/// Over a tile whose full-size photo is downloading: the tile dimmed, and a ring that fills
/// with `progress` (a spinning arc while the size is unknown).
fn download_ring(ui: &egui::Ui, rect: egui::Rect, progress: Option<f32>) {
    let pal = theme::p();
    let painter = ui.painter();
    painter.rect_filled(rect, 9.0, pal.bg.gamma_multiply(0.6));
    let (c, r) = (rect.center(), (rect.width().min(rect.height()) * 0.18).clamp(10.0, 22.0));
    painter.circle_stroke(c, r, egui::Stroke::new(3.0, pal.text_faint.gamma_multiply(0.6)));
    let t = ui.input(|i| i.time) as f32;
    let (start, sweep) = match progress {
        // from 12 o'clock, clockwise
        Some(f) => (-std::f32::consts::FRAC_PI_2, f.max(0.02) * std::f32::consts::TAU),
        None => (t * 5.0, std::f32::consts::FRAC_PI_2),
    };
    let n = ((sweep / std::f32::consts::TAU) * 48.0).ceil().max(2.0) as usize;
    let points: Vec<egui::Pos2> = (0..=n).map(|i| start + sweep * i as f32 / n as f32).map(|a| c + egui::vec2(a.cos(), a.sin()) * r).collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(3.0, pal.accent)));
    if let Some(f) = progress {
        painter.text(c + egui::vec2(0.0, r + 12.0), egui::Align2::CENTER_CENTER, format!("{:.0}%", f * 100.0), egui::FontId::proportional(11.0), pal.text);
    }
    ui.ctx().request_repaint();
}

fn wallpaper_tile(ui: &mut egui::Ui, path: Option<&std::path::Path>, name: &str, size: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let painter = ui.painter();
    let radius = 9.0;
    let mut drawn = false;
    if let Some(p) = path {
        let uri = crate::os::texture_uri(p);
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
