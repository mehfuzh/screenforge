//! Chapters: the Chapters page in the sidebar (list, rename, remove, copy for YouTube) and the
//! edits the timeline's chapter flags make.

use egui::RichText;
use sf_engine::chapters as ch;
use sf_project::{Chapter, SceneChaptersExt};

use crate::editor::Editor;
use crate::{theme, widgets};

/// Words of the transcript a new chapter is named after.
const TITLE_WORDS: usize = 5;

/// A title for a chapter starting at output time `t`: the first words said from there (within
/// a few seconds), else "Chapter n".
fn suggested_title(ed: &Editor, t: f64) -> String {
    let n = ed.session.pkg.project.scenes[0].chapters().len() + 1;
    let Some(src) = ed.session.timeline.output_to_source(t) else { return format!("Chapter {n}") };
    let words: Vec<&str> = ed.session.captions.lines.iter().flat_map(|l| &l.words).filter(|w| w.start_ms >= src - 200.0 && w.start_ms < src + 8000.0).take(TITLE_WORDS).map(|w| w.text.trim()).collect();
    if words.is_empty() {
        return format!("Chapter {n}");
    }
    let mut title = words.join(" ").trim_end_matches(['.', ',', '?', '!', ';', ':']).to_string();
    if let Some(first) = title.get(..1) {
        title = first.to_uppercase() + &title[1..];
    }
    title
}

/// Add a chapter at output time `t`, select it and show it in the sidebar.
pub fn add_at(ed: &mut Editor, t: f64) {
    let t = t.clamp(0.0, (ed.duration_ms() - 1.0).max(0.0));
    let (source_ms, card) = ch::anchor_at(&ed.session.pkg.project.scenes[0], &ed.session.timeline, t);
    let c = Chapter::new(suggested_title(ed, t), source_ms, card);
    let id = c.id.clone();
    ed.edit(move |p| {
        let mut list = p.scenes[0].chapters();
        list.push(c);
        p.scenes[0].set_chapters(list);
    });
    select(ed, &id);
}

/// Select chapter `id` and open the Chapters page.
pub fn select(ed: &mut Editor, id: &str) {
    ed.selected_chapter = Some(id.to_string());
    ed.section = crate::sidebar::Section::Chapters;
    ed.sidebar_open = true;
}

/// Change chapter `id` without an undo entry (a drag or typing in progress; the caller commits
/// it with `commit_live_edit`).
fn live(ed: &mut Editor, id: &str, f: impl FnOnce(&mut Chapter)) {
    let scene = &mut ed.session.pkg.project.scenes[0];
    let mut list = scene.chapters();
    if let Some(c) = list.iter_mut().find(|c| c.id == id) {
        f(c);
        scene.set_chapters(list);
        ed.invalidate_preview();
    }
}

/// Move chapter `id` to output time `t` (live).
pub fn move_live(ed: &mut Editor, id: &str, t: f64) {
    let t = t.clamp(0.0, (ed.duration_ms() - 1.0).max(0.0));
    let (source_ms, card) = ch::anchor_at(&ed.session.pkg.project.scenes[0], &ed.session.timeline, t);
    live(ed, id, |c| {
        c.source_ms = source_ms;
        c.card = card;
    });
}

pub fn remove(ed: &mut Editor, id: &str) {
    let id = id.to_string();
    if ed.selected_chapter.as_deref() == Some(id.as_str()) {
        ed.selected_chapter = None;
    }
    ed.edit(move |p| {
        let mut list = p.scenes[0].chapters();
        list.retain(|c| c.id != id);
        p.scenes[0].set_chapters(list);
    });
}

/// The Chapters page.
pub fn ui(ed: &mut Editor, ui: &mut egui::Ui) {
    widgets::title(ui, "Chapters");
    widgets::hint(ui, "Chapters for a YouTube description. Each one stays with its moment of the recording when you cut, and the list uses the times of the edited video.");
    ui.add_space(8.0);
    if widgets::secondary_button(ui, "Add chapter at playhead").clicked() {
        add_at(ed, ed.playhead_ms);
    }
    ui.add_space(10.0);

    let duration = ed.duration_ms();
    let placed = ch::placed(&ed.session.pkg.project.scenes[0], &ed.session.timeline);
    let pal = theme::p();
    if placed.is_empty() {
        widgets::hint(ui, "No chapters yet. Add one here, double-click the row above the clips, or ask the agent to add chapters from the transcript.");
    }
    let mut seek = None;
    let mut remove_id = None;
    let mut focused = false;
    for c in &placed {
        let selected = ed.selected_chapter.as_deref() == Some(c.id.as_str());
        ui.horizontal(|ui| {
            let time = ui.add(egui::Button::new(RichText::new(ch::timestamp(c.start_ms, duration)).size(13.0).monospace().color(if selected { pal.accent } else { pal.text_dim })).frame(false)).on_hover_text("Go to this chapter").on_hover_cursor(egui::CursorIcon::PointingHand);
            if time.clicked() {
                seek = Some((c.id.clone(), c.start_ms));
            }
            let trash = ui.available_width() - 30.0;
            let mut title = c.title.clone();
            let field = ui.scope(|ui| {
                ui.set_max_width(trash.max(60.0));
                widgets::text_field(ui, &mut title, "Chapter title")
            });
            let field = field.inner;
            focused |= field.has_focus();
            if field.gained_focus() {
                ed.selected_chapter = Some(c.id.clone());
            }
            if field.changed() {
                if ed.chapter_snapshot.is_none() {
                    ed.chapter_snapshot = Some(ed.session.pkg.project.clone());
                }
                live(ed, &c.id, |ch| ch.title = title.clone());
            }
            if widgets::icon_button(ui, widgets::Icon::Trash, 13.0, "Remove this chapter").clicked() {
                remove_id = Some(c.id.clone());
            }
        });
        ui.add_space(2.0);
    }
    // typing is one undo step once the field is left
    if !focused {
        if let Some(s) = ed.chapter_snapshot.take() {
            ed.commit_live_edit(s);
        }
    }
    if let Some((id, t)) = seek {
        ed.selected_chapter = Some(id);
        ed.pause();
        ed.seek(t);
    }
    if let Some(id) = remove_id {
        remove(ed, &id);
    }
    let cut = ed.session.pkg.project.scenes[0].chapters().len().saturating_sub(placed.len());
    if cut > 0 {
        widgets::hint(ui, &format!("{cut} more {} inside a cut and left out until that part is restored.", if cut == 1 { "chapter is" } else { "chapters are" }));
    }

    if !placed.is_empty() {
        ui.add_space(14.0);
        let yt = ch::youtube(&placed, duration);
        ui.label(RichText::new("YouTube").size(15.0).color(pal.text));
        ui.add_space(4.0);
        let mut text = yt.text.clone();
        ui.add(egui::TextEdit::multiline(&mut text).font(egui::FontId::monospace(13.0)).desired_width(f32::INFINITY).desired_rows(placed.len().min(12)).interactive(false));
        ui.add_space(6.0);
        if widgets::accent_button(ui, "Copy YouTube chapters").on_hover_text("Paste them into the video's description on YouTube").clicked() {
            ui.ctx().copy_text(yt.text.clone());
            ed.set_status("Copied the chapters for YouTube");
        }
        for w in &yt.warnings {
            ui.add_space(4.0);
            widgets::hint(ui, w);
        }
    }
}
