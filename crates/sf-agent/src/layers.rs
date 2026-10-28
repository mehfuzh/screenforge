//! Tools for what is drawn over the recording and how clips hand over: titles, overlay media
//! (images and videos) and transitions at cuts. Times are output ms, as in [`crate::tools`].

use serde_json::{json, Value};
use sf_engine::timeline::Timeline;
use sf_project::schema::common::{Point, Rect};
use sf_project::tracks::{self, Clip, ClipAnim, Link, MediaKind, SceneTracksExt, TextStyle};
use sf_project::v4::{Card, Transition};

use crate::tools::{clock, num, opt_num, Outcome, Workspace};

const FONTS: [&str; 4] = ["sans-serif", "heavy", "serif", "mono"];
const ALIGNS: [&str; 3] = ["left", "center", "right"];
/// Transition lengths the agent may set.
const TRANSITION_MS: (f64, f64) = (100.0, 2000.0);
/// How close `at_ms` must be to a cut for `set_transition`.
const CUT_SNAP_MS: f64 = 50.0;
/// Longest title card the agent may add.
const CARD_MAX_MS: f64 = 10_000.0;

fn anim_kinds() -> Vec<&'static str> {
    let mut k = vec!["none"];
    k.extend(ClipAnim::KINDS);
    k
}

pub fn definitions() -> Vec<Value> {
    let anim = json!({ "type": "string", "enum": anim_kinds() });
    let position = json!({ "type": "object", "description": "0–1 of the video; x is the left edge, centre or right edge by align, y the vertical centre", "properties": { "x": { "type": "number" }, "y": { "type": "number" } }, "required": ["x", "y"] });
    let rect = json!({ "type": "object", "description": "custom placement, 0–1 of the video (top-left x, y)", "properties": { "x": { "type": "number" }, "y": { "type": "number" }, "width": { "type": "number" }, "height": { "type": "number" } }, "required": ["x", "y", "width", "height"] });
    let title_props = json!({
        "content": { "type": "string", "description": "The title; \\n starts a new line; wrap words in * to draw them in the accent colour (\"The *probability* of success\")" },
        "subtitle": { "type": "string", "description": "A smaller line under the title (empty to remove)" },
        "font": { "type": "string", "enum": FONTS },
        "size_ratio": { "type": "number", "description": "Font size as a fraction of the video height, 0.02–0.2 (default 0.06)" },
        "color": { "type": "string", "description": "#rrggbb" },
        "accent_color": { "type": "string", "description": "#rrggbb for the *starred* words" },
        "align": { "type": "string", "enum": ALIGNS },
        "position": position,
    });
    let overlay_props = json!({
        "placement": { "type": "string", "enum": Clip::PLACEMENTS },
        "rect": rect,
        "corner_ratio": { "type": "number", "description": "Rounded corners, 0–0.5 of the shorter side" },
        "backdrop_blur": { "type": "number", "description": "Blur the recording behind while it shows, 0–1 (shallow focus)" },
        "backdrop_dim": { "type": "number", "description": "Darken the recording behind, 0–1" },
    });
    let mut add_title = json!({
        "start_ms": { "type": "number" },
        "duration_ms": { "type": "number", "description": "Default 3000" },
        "shadow": { "type": "boolean", "description": "A soft shadow under the letters" },
        "anim_in": anim, "anim_out": anim,
        "pinned": { "type": "boolean", "description": "Default true: stays with its moment of the recording when earlier footage is cut. false: fixed time in the final video" },
    });
    merge(&mut add_title, &title_props);
    let mut add_overlay = json!({
        "path": { "type": "string", "description": "An image (png, jpg, webp …) or video (mp4, mov, webm) file the user named" },
        "start_ms": { "type": "number" },
        "duration_ms": { "type": "number", "description": "Default 4000, or the video's length" },
        "shadow": { "type": "number", "description": "Drop shadow, 0–1" },
        "anim_in": anim, "anim_out": anim,
        "pinned": { "type": "boolean", "description": "Default true, as for titles" },
    });
    merge(&mut add_overlay, &overlay_props);
    let mut update = json!({
        "id": { "type": "string" },
        "start_ms": { "type": "number" },
        "duration_ms": { "type": "number" },
        "shadow": { "type": ["boolean", "number"], "description": "Titles: true/false; overlays: 0–1" },
        "anim_in": anim, "anim_out": anim,
        "anim_ms": { "type": "number", "description": "Length of both animations, 100–2000 ms" },
        "pinned": { "type": "boolean" },
        "hidden": { "type": "boolean", "description": "Hide or show the whole lane the clip is on" },
    });
    merge(&mut update, &title_props);
    merge(&mut update, &overlay_props);
    vec![
        json!({
            "name": "add_title",
            "description": "Put a title over the video: large words, some in an accent colour, with an optional smaller subtitle, as in product films (\"Every move, simulated.\"). By default each word blurs in after the previous one and the title blurs out. Titles that overlap in time go on separate lanes.",
            "input_schema": { "type": "object", "properties": add_title, "required": ["content", "start_ms"] }
        }),
        json!({
            "name": "add_overlay",
            "description": "Show an image or video file over the recording for a while (a product card, a screenshot, b-roll): centred by default, full frame, picture-in-picture in a corner, or a custom rect. Videos play silently. backdrop_blur and backdrop_dim put the recording behind out of focus so the overlay stands out.",
            "input_schema": { "type": "object", "properties": add_overlay, "required": ["path", "start_ms"] }
        }),
        json!({
            "name": "update_clip",
            "description": "Change a title or overlay from get_project (titles, overlays): text and style, timing, placement, animations, backdrop, pinned.",
            "input_schema": { "type": "object", "properties": update, "required": ["id"] }
        }),
        json!({
            "name": "remove_clip",
            "description": "Delete a title or overlay by id.",
            "input_schema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        }),
        json!({
            "name": "add_title_card",
            "description": "Insert a title card: a clip with only a background and a centred title (words blurring in), before clip `index` (0 = an intro before the video, the clip count = an outro at the end). The clip after it crossfades in; change that with set_transition. Edit the title with update_clip(title_id). Things at a fixed time move along; pinned titles, zooms and voice-overs stay with the recording.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "Default 0" },
                    "content": { "type": "string", "description": "The title, with *accent* words as for add_title" },
                    "subtitle": { "type": "string" },
                    "duration_ms": { "type": "number", "description": "500–10000, default 3000" },
                    "background": { "type": "string", "description": "#rrggbb fill; default a dark #0f1220. To show the project's wallpaper or gradient instead, set it afterwards with patch_project: /scenes/0/slices/<index>/sfCard/background to null" }
                },
                "required": ["content"]
            }
        }),
        json!({
            "name": "remove_title_card",
            "description": "Delete the title card at clip `index` (from get_project clips with titleCard) and the titles on it.",
            "input_schema": { "type": "object", "properties": { "index": { "type": "integer" } }, "required": ["index"] }
        }),
        json!({
            "name": "set_transition",
            "description": "How one clip hands over to the next at a cut: crossfade (dissolve), fade-black (dip to black), blur (the old shot blurs away) or none (a hard cut). Name the incoming clip by index (from get_project clips) or by at_ms, the time of the cut. Clip 0 with fade-black fades the video in from black. The timeline keeps its length.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "index": { "type": "integer" },
                    "at_ms": { "type": "number" },
                    "kind": { "type": "string", "enum": ["crossfade", "fade-black", "blur", "none"] },
                    "duration_ms": { "type": "number", "description": "100–2000, default 500" }
                },
                "required": ["kind"]
            }
        }),
    ]
}

fn merge(into: &mut Value, from: &Value) {
    if let (Value::Object(a), Value::Object(b)) = (into, from) {
        a.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}

fn str_in<'a>(input: &'a Value, key: &str, allowed: &[&str]) -> Result<Option<&'a str>, String> {
    match input.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(v) if allowed.contains(&v) => Ok(Some(v)),
        Some(v) => Err(format!("unknown {key} {v} ({})", allowed.join(", "))),
    }
}

fn color(input: &Value, key: &str) -> Result<Option<String>, String> {
    match input.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(c) if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()) => Ok(Some(c.to_ascii_lowercase())),
        Some(c) => Err(format!("`{key}` must be #rrggbb, not {c}")),
    }
}

/// `anim_in` / `anim_out`: `Some(None)` for "none".
fn anim(input: &Value, key: &str) -> Result<Option<Option<ClipAnim>>, String> {
    Ok(str_in(input, key, &anim_kinds())?.map(|k| (k != "none").then(|| ClipAnim::new(k))))
}

/// Text fields of `input` applied to `style`.
fn apply_text(style: &mut TextStyle, input: &Value) -> Result<(), String> {
    if let Some(c) = input.get("content").and_then(Value::as_str) {
        if c.trim().is_empty() {
            return Err("`content` is empty".into());
        }
        style.content = c.to_string();
    }
    if let Some(s) = input.get("subtitle").and_then(Value::as_str) {
        style.subtitle = s.to_string();
    }
    if let Some(f) = str_in(input, "font", &FONTS)? {
        style.font = f.into();
    }
    if let Some(s) = opt_num(input, "size_ratio")? {
        style.size_ratio = s.clamp(0.02, 0.2);
    }
    if let Some(c) = color(input, "color")? {
        style.color = c;
    }
    if let Some(c) = color(input, "accent_color")? {
        style.accent_color = c;
    }
    if let Some(a) = str_in(input, "align", &ALIGNS)? {
        style.align = a.into();
    }
    if let Some(p) = input.get("position").filter(|v| !v.is_null()) {
        style.position01 = Point::new(num(p, "x")?.clamp(0.0, 1.0), num(p, "y")?.clamp(0.0, 1.0));
    }
    if let Some(s) = input.get("shadow") {
        style.shadow = s.as_bool().unwrap_or_else(|| s.as_f64().is_some_and(|v| v > 0.0));
    }
    Ok(())
}

/// Overlay fields of `input` applied to `clip`.
fn apply_overlay(clip: &mut Clip, input: &Value) -> Result<(), String> {
    if let Some(p) = str_in(input, "placement", &Clip::PLACEMENTS)? {
        clip.placement = Some(p.into());
    }
    if let Some(r) = input.get("rect").filter(|v| !v.is_null()) {
        let (x, y) = (num(r, "x")?.clamp(0.0, 1.0), num(r, "y")?.clamp(0.0, 1.0));
        let (w, h) = (num(r, "width")?.clamp(0.02, 1.0 - x), num(r, "height")?.clamp(0.02, 1.0 - y));
        clip.rect01 = Some(Rect { x, y, width: w.max(0.02), height: h.max(0.02) });
        clip.placement = Some("custom".into());
    }
    if let Some(c) = opt_num(input, "corner_ratio")? {
        clip.corner_ratio = c.clamp(0.0, 0.5);
    }
    if let Some(s) = input.get("shadow") {
        clip.shadow = s.as_f64().or_else(|| s.as_bool().map(|b| if b { 0.5 } else { 0.0 })).unwrap_or(0.0).clamp(0.0, 1.0);
    }
    if let Some(b) = opt_num(input, "backdrop_blur")? {
        clip.backdrop_blur = b.clamp(0.0, 1.0);
    }
    if let Some(d) = opt_num(input, "backdrop_dim")? {
        clip.backdrop_dim = d.clamp(0.0, 1.0);
    }
    Ok(())
}

fn apply_anims(clip: &mut Clip, input: &Value) -> Result<(), String> {
    if let Some(a) = anim(input, "anim_in")? {
        clip.anim_in = a;
    }
    if let Some(a) = anim(input, "anim_out")? {
        clip.anim_out = a;
    }
    if let Some(ms) = opt_num(input, "anim_ms")? {
        let ms = ms.clamp(100.0, 2000.0);
        clip.anim_in.iter_mut().chain(clip.anim_out.iter_mut()).for_each(|a| a.duration_ms = ms);
    }
    Ok(())
}

impl Workspace {
    /// Start and length for a clip on `tl`: checked against the video, the length cut to fit.
    fn clip_span(tl: &Timeline, start: f64, duration: f64) -> Result<(f64, f64), String> {
        let dur = tl.duration_ms();
        if !(0.0..dur).contains(&start) {
            return Err(format!("start {} is outside the video ({} long)", clock(start), clock(dur)));
        }
        let d = duration.min(dur - start);
        if d < Clip::MIN_DURATION_MS {
            return Err(format!("a title or overlay must be at least {:.1} s long (and inside the video)", Clip::MIN_DURATION_MS / 1000.0));
        }
        Ok((start, d))
    }

    /// Pin `clip` to the moment of the recording at its start, or unpin it. A clip on a title
    /// card stays unpinned: the card has no recording to pin to.
    fn pin(tl: &Timeline, clip: &mut Clip, pinned: bool) {
        let on_card = tl.span_at_output(clip.output_start_ms).is_some_and(|s| s.card);
        clip.link = if pinned && !on_card { tl.output_to_source(clip.output_start_ms).map(|s| Link { source_ms: s }) } else { None };
    }

    fn add_title_card(&mut self, input: &Value) -> Result<Outcome, String> {
        let content = input.get("content").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).ok_or("`content` is required")?;
        let n = self.project.scenes[0].slices.len();
        let index = opt_num(input, "index")?.unwrap_or(0.0);
        if index < 0.0 || index > n as f64 || index.fract() != 0.0 {
            return Err(format!("index must be a clip index from 0 to {n} (the card goes before that clip; {n} = at the end)"));
        }
        let ms = opt_num(input, "duration_ms")?.unwrap_or(Card::DEFAULT_MS);
        if !(Card::MIN_MS..=CARD_MAX_MS).contains(&ms) {
            return Err(format!("a title card lasts {:.1}–{:.0} s", Card::MIN_MS / 1000.0, CARD_MAX_MS / 1000.0));
        }
        let background = color(input, "background")?.or_else(|| Some(Card::DEFAULT_FILL.into()));
        let index = index as usize;
        let scene = &mut self.project.scenes[0];
        let id = sf_engine::tracks::add_title_card(scene, index, content, ms);
        scene.slices[index].card.as_mut().expect("card").background = background;
        if let Some(sub) = input.get("subtitle").and_then(Value::as_str) {
            let mut t = scene.tracks();
            if let Some(style) = tracks::find_clip_mut(&mut t, &id).and_then(|c| c.text.as_mut()) {
                style.subtitle = sub.to_string();
            }
            scene.set_tracks(t);
        }
        Ok(Outcome::edit(json!({ "index": index, "title_id": id }), "Added a title card"))
    }

    fn remove_title_card(&mut self, input: &Value) -> Result<Outcome, String> {
        let index = num(input, "index")? as usize;
        let tl = self.timeline();
        let Some(span) = tl.spans().get(index).copied().filter(|s| s.card) else { return Err(format!("clip {index} is not a title card")) };
        let scene = &mut self.project.scenes[0];
        let mut t = scene.tracks();
        let inside: Vec<String> = t.iter().flat_map(|tr| &tr.clips).filter(|c| c.link.is_none() && c.output_start_ms >= span.output_start_ms - 0.5 && c.output_end_ms() <= span.output_end_ms + 0.5).map(|c| c.id.clone()).collect();
        for id in &inside {
            tracks::remove_clip(&mut t, id);
        }
        scene.set_tracks(t);
        sf_engine::tracks::remove_card(scene, index);
        Ok(Outcome::edit(json!({ "ok": true, "removed_titles": inside }), "Removed a title card"))
    }

    pub(crate) fn run_layer(&mut self, name: &str, input: &Value) -> Option<Result<Outcome, String>> {
        Some(match name {
            "add_title" => self.add_title(input),
            "add_overlay" => self.add_overlay(input),
            "update_clip" => self.update_clip(input),
            "remove_clip" => (|| {
                let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
                let mut t = self.project.scenes[0].tracks();
                if !tracks::remove_clip(&mut t, id) {
                    return Err(format!("no title or overlay {id}"));
                }
                self.project.scenes[0].set_tracks(t);
                Ok(Outcome::edit(json!({ "ok": true }), "Removed a title or overlay"))
            })(),
            "set_transition" => self.set_transition(input),
            "add_title_card" => self.add_title_card(input),
            "remove_title_card" => self.remove_title_card(input),
            _ => return None,
        })
    }

    fn add_title(&mut self, input: &Value) -> Result<Outcome, String> {
        let content = input.get("content").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).ok_or("`content` is required")?;
        let tl = self.timeline();
        let (start, dur) = Self::clip_span(&tl, num(input, "start_ms")?, opt_num(input, "duration_ms")?.unwrap_or(Clip::TITLE_MS))?;
        let mut clip = Clip::title(start, dur, content);
        apply_text(clip.text.as_mut().expect("title"), input)?;
        apply_anims(&mut clip, input)?;
        Self::pin(&tl, &mut clip, input.get("pinned").and_then(Value::as_bool).unwrap_or(true));
        let id = clip.id.clone();
        let mut t = self.project.scenes[0].tracks();
        tracks::add_clip(&mut t, clip);
        self.project.scenes[0].set_tracks(t);
        let line = content.lines().next().unwrap_or("").replace('*', "");
        Ok(Outcome::edit(json!({ "id": id }), format!("Title \"{line}\" {} – {}", clock(start), clock(start + dur))))
    }

    fn add_overlay(&mut self, input: &Value) -> Result<Outcome, String> {
        let path = input.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
        let tl = self.timeline();
        let start = num(input, "start_ms")?;
        let item = self.host.as_deref().ok_or("not available here (no editor)")?.import_media(path)?;
        if item.kind == MediaKind::Audio {
            return Err("that is an audio file: use import_file with use_as music, or add_narration".into());
        }
        let default = item.duration_ms.filter(|_| item.kind == MediaKind::Video).unwrap_or(4000.0);
        let (start, dur) = Self::clip_span(&tl, start, opt_num(input, "duration_ms")?.unwrap_or(default))?;
        let mut clip = Clip::overlay(&item, start, dur);
        apply_overlay(&mut clip, input)?;
        apply_anims(&mut clip, input)?;
        Self::pin(&tl, &mut clip, input.get("pinned").and_then(Value::as_bool).unwrap_or(true));
        let id = clip.id.clone();
        let name = item.path.rsplit('/').next().unwrap_or(&item.path).to_string();
        let scene = &mut self.project.scenes[0];
        scene.add_media(item);
        let mut t = scene.tracks();
        tracks::add_clip(&mut t, clip);
        scene.set_tracks(t);
        Ok(Outcome::edit(json!({ "id": id }), format!("Overlay {name} {} – {}", clock(start), clock(start + dur))))
    }

    fn update_clip(&mut self, input: &Value) -> Result<Outcome, String> {
        let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
        let tl = self.timeline();
        let mut t = self.project.scenes[0].tracks();
        let track = t.iter().position(|tr| tr.clips.iter().any(|c| c.id == id)).ok_or_else(|| format!("no title or overlay {id}"))?;
        if let Some(h) = input.get("hidden").and_then(Value::as_bool) {
            t[track].hidden = h;
        }
        let c = tracks::find_clip_mut(&mut t, id).expect("clip");
        let current = sf_engine::tracks::clip_start(c, &tl).unwrap_or(c.output_start_ms);
        let start = opt_num(input, "start_ms")?.unwrap_or(current);
        let dur = opt_num(input, "duration_ms")?.unwrap_or(c.output_duration_ms());
        if input.get("start_ms").is_some() || input.get("duration_ms").is_some() {
            let (s, d) = Self::clip_span(&tl, start, dur)?;
            c.output_start_ms = s;
            c.set_output_duration(d);
            if c.link.is_some() {
                Self::pin(&tl, c, true);
            }
        }
        if let Some(style) = c.text.as_mut() {
            apply_text(style, input)?;
        } else {
            apply_overlay(c, input)?;
        }
        apply_anims(c, input)?;
        if let Some(p) = input.get("pinned").and_then(Value::as_bool) {
            c.output_start_ms = start;
            Self::pin(&tl, c, p);
        }
        // a clip whose time changed may now overlap another on its lane: give it a lane of its own
        let moved = t[track].clips.iter().position(|o| o.id == id).expect("clip");
        let (s, e) = (t[track].clips[moved].output_start_ms, t[track].clips[moved].output_end_ms());
        if t[track].clips.iter().any(|o| o.id != id && s < o.output_end_ms() && o.output_start_ms < e) {
            let clip = t[track].clips.remove(moved);
            tracks::add_clip(&mut t, clip);
            t.retain(|tr| !tr.clips.is_empty() || !tr.is_visual());
        }
        self.project.scenes[0].set_tracks(t);
        Ok(Outcome::edit(json!({ "ok": true }), "Changed a title or overlay"))
    }

    fn set_transition(&mut self, input: &Value) -> Result<Outcome, String> {
        let kind = str_in(input, "kind", &["crossfade", "fade-black", "blur", "none"])?.ok_or("`kind` is required")?;
        let tl = self.timeline();
        let index = match (input.get("index").and_then(Value::as_u64), opt_num(input, "at_ms")?) {
            (Some(i), _) => i as usize,
            (None, Some(t)) => tl
                .spans()
                .iter()
                .find(|s| (s.output_start_ms - t).abs() <= CUT_SNAP_MS)
                .map(|s| s.index)
                .ok_or_else(|| format!("there is no cut at {}; use a clip's startMs from get_project", clock(t)))?,
            (None, None) => return Err("give the clip `index` or the cut's `at_ms`".into()),
        };
        let slices = &mut self.project.scenes[0].slices;
        let n = slices.len();
        let slice = slices.get_mut(index).ok_or_else(|| format!("there is no clip {index} (the video has {n})"))?;
        if kind == "none" {
            slice.transition = None;
            return Ok(Outcome::edit(json!({ "ok": true }), format!("Hard cut into clip {index}")));
        }
        if index == 0 && kind != "fade-black" {
            return Err("the first clip has nothing before it: only fade-black (fade in from black) works there".into());
        }
        let ms = opt_num(input, "duration_ms")?.unwrap_or(Transition::DEFAULT_MS).clamp(TRANSITION_MS.0, TRANSITION_MS.1);
        slice.transition = Some(Transition { kind: kind.into(), duration_ms: ms });
        let at = tl.spans().get(index).map(|s| s.output_start_ms).unwrap_or(0.0);
        Ok(Outcome::edit(json!({ "ok": true }), format!("{} at {}", kind.replace('-', " "), clock(at))))
    }

    /// Titles and overlays for `get_project`, with where they show now.
    pub(crate) fn layers_overview(&self, tl: &Timeline) -> (Vec<Value>, Vec<Value>) {
        let ms = |v: f64| (v * 10.0).round() / 10.0;
        let scene = &self.project.scenes[0];
        let media = scene.media();
        let (mut titles, mut overlays) = (vec![], vec![]);
        for (lane, track) in scene.tracks().iter().filter(|t| t.is_visual()).enumerate() {
            for c in &track.clips {
                let start = sf_engine::tracks::clip_start(c, tl);
                let common = json!({
                    "id": c.id,
                    "lane": lane,
                    "startMs": start.map(ms),
                    "endMs": start.map(|s| ms(s + c.output_duration_ms())),
                    "pinned": c.link.is_some(),
                    "hiddenBecauseCut": start.is_none(),
                    "laneHidden": track.hidden,
                    "animIn": c.anim_in.as_ref().map(|a| a.kind.clone()),
                    "animOut": c.anim_out.as_ref().map(|a| a.kind.clone()),
                });
                let mut v = common;
                if let Some(s) = &c.text {
                    merge(&mut v, &json!({ "content": s.content, "subtitle": s.subtitle, "font": s.font, "sizeRatio": s.size_ratio, "align": s.align, "position": { "x": s.position01.x, "y": s.position01.y } }));
                    titles.push(v);
                } else {
                    let m = c.media.as_deref().and_then(|id| media.iter().find(|m| m.id == id));
                    merge(
                        &mut v,
                        &json!({
                            "file": m.map(|m| m.path.rsplit('/').next().unwrap_or(&m.path).to_string()),
                            "kind": m.map(|m| if m.kind == MediaKind::Video { "video" } else { "image" }),
                            "placement": c.placement,
                            "backdropBlur": c.backdrop_blur,
                            "backdropDim": c.backdrop_dim,
                        }),
                    );
                    overlays.push(v);
                }
            }
        }
        (titles, overlays)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::tracks::MediaItem;
    use sf_project::v4::Slice;
    use sf_project::Project;
    use std::sync::Arc;

    /// Two clips: 0–4 s and 6–10 s of the recording, so output 4 s is a cut.
    fn ws() -> Workspace {
        let mut p = Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 4000.0), Slice::new(6000.0, 10_000.0)];
        let mut w = Workspace::new(p);
        w.host = Some(Arc::new(Files));
        w
    }

    fn id(o: &Outcome) -> String {
        serde_json::from_str::<Value>(&o.result).unwrap()["id"].as_str().unwrap().to_string()
    }

    /// A host whose files are images unless they end in .mp4 (5 s long) or .mp3.
    struct Files;
    impl crate::Host for Files {
        fn options(&self) -> Value {
            json!({})
        }
        fn apply_preset(&self, _: &str, _: &mut Project) -> Result<(), String> {
            Ok(())
        }
        fn search_photos(&self, _: &str) -> Result<Value, String> {
            Ok(json!([]))
        }
        fn download_photo(&self, _: &str) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn import_file(&self, p: &str) -> Result<String, String> {
            Ok(p.into())
        }
        fn screen_motion(&self) -> Result<Vec<(f64, f64, crate::motion::Motion)>, String> {
            Ok(vec![])
        }
        fn screen_frames(&self, _: &[f64]) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![])
        }
        fn import_media(&self, path: &str) -> Result<MediaItem, String> {
            let kind = MediaItem::kind_of(std::path::Path::new(path)).ok_or("not media")?;
            let name = path.rsplit('/').next().unwrap();
            Ok(MediaItem { id: format!("m_{name}"), path: format!("media/abcd1234-{name}"), kind, duration_ms: (kind == MediaKind::Video).then_some(5000.0), has_audio: false, alpha: false, origin: json!("import"), extra: Default::default() })
        }
    }

    fn overview(w: &mut Workspace) -> Value {
        serde_json::from_str(&w.run("get_project", &json!({})).unwrap().result).unwrap()
    }

    #[test]
    fn titles_are_pinned_styled_and_listed() {
        let mut w = ws();
        let o = w.run("add_title", &json!({ "content": "The *probability* of success.", "subtitle": "simulated", "start_ms": 5000, "duration_ms": 2000, "font": "heavy", "align": "center", "position": { "x": 0.5, "y": 0.5 } })).unwrap();
        let tid = id(&o);
        let t = w.project.scenes[0].tracks();
        let c = &t[0].clips[0];
        // output 5 s is source 7 s (the second clip starts at source 6 s)
        assert_eq!(c.link.unwrap().source_ms, 7000.0);
        let s = c.text.as_ref().unwrap();
        assert_eq!((s.font.as_str(), s.align.as_str(), s.subtitle.as_str()), ("heavy", "center", "simulated"));
        assert_eq!(c.anim_in.as_ref().unwrap().kind, "words");
        let v = overview(&mut w);
        assert_eq!(v["titles"][0]["id"], tid);
        assert_eq!(v["titles"][0]["startMs"], 5000.0);
        assert_eq!(v["titles"][0]["endMs"], 7000.0);

        // a second title at the same time goes on its own lane; unpinned keeps its time
        w.run("add_title", &json!({ "content": "Two", "start_ms": 5500, "pinned": false, "anim_in": "fade", "anim_out": "none" })).unwrap();
        let t = w.project.scenes[0].tracks();
        assert_eq!(t.len(), 2);
        assert!(t[1].clips[0].link.is_none() && t[1].clips[0].anim_out.is_none());
        assert_eq!(t[1].clips[0].output_duration_ms(), 2500.0, "cut to the end of the video");
    }

    #[test]
    fn title_validation() {
        let mut w = ws();
        assert!(w.run("add_title", &json!({ "content": " ", "start_ms": 0 })).unwrap_err().contains("content"));
        assert!(w.run("add_title", &json!({ "content": "x", "start_ms": 9000 })).unwrap_err().contains("outside"));
        assert!(w.run("add_title", &json!({ "content": "x", "start_ms": 7950 })).unwrap_err().contains("at least"));
        assert!(w.run("add_title", &json!({ "content": "x", "start_ms": 0, "font": "comic" })).unwrap_err().contains("font"));
        assert!(w.run("add_title", &json!({ "content": "x", "start_ms": 0, "anim_in": "spin" })).unwrap_err().contains("anim_in"));
        assert!(w.run("add_title", &json!({ "content": "x", "start_ms": 0, "color": "red" })).unwrap_err().contains("#rrggbb"));
        // the length is cut to the end of the video
        w.run("add_title", &json!({ "content": "x", "start_ms": 7000, "duration_ms": 5000 })).unwrap();
        assert_eq!(w.project.scenes[0].tracks()[0].clips[0].output_duration_ms(), 1000.0);
    }

    #[test]
    fn overlays_bring_the_file_in_and_take_a_video_length() {
        let mut w = ws();
        let o = w.run("add_overlay", &json!({ "path": "/tmp/card.png", "start_ms": 1000, "placement": "pip-br", "backdrop_blur": 0.6, "shadow": 0.8 })).unwrap();
        let scene = &w.project.scenes[0];
        assert_eq!(scene.media().len(), 1);
        let c = &scene.tracks()[0].clips[0];
        assert_eq!((c.placement.as_deref(), c.backdrop_blur, c.shadow, c.output_duration_ms()), (Some("pip-br"), 0.6, 0.8, 4000.0));
        assert_eq!(c.id, id(&o));
        w.run("add_overlay", &json!({ "path": "/tmp/demo.mp4", "start_ms": 1000, "rect": { "x": 0.5, "y": 0.1, "width": 0.4, "height": 0.4 } })).unwrap();
        let t = w.project.scenes[0].tracks();
        let v = &t[1].clips[0];
        assert_eq!((v.placement.as_deref(), v.output_duration_ms()), (Some("custom"), 5000.0));
        let ov = overview(&mut w);
        assert_eq!(ov["overlays"][1]["kind"], "video");
        assert_eq!(ov["overlays"][0]["file"], "abcd1234-card.png");
        assert!(w.run("add_overlay", &json!({ "path": "/tmp/song.mp3", "start_ms": 0 })).unwrap_err().contains("audio"));
        assert!(w.run("add_overlay", &json!({ "path": "/tmp/card.png", "start_ms": 0, "placement": "sideways" })).is_err());
        let mut no_host = Workspace::new(w.project.clone());
        assert!(no_host.run("add_overlay", &json!({ "path": "/tmp/card.png", "start_ms": 0 })).is_err());
    }

    #[test]
    fn update_and_remove_clips() {
        let mut w = ws();
        let tid = id(&w.run("add_title", &json!({ "content": "a", "start_ms": 0, "duration_ms": 2000 })).unwrap());
        let oid = id(&w.run("add_overlay", &json!({ "path": "/tmp/card.png", "start_ms": 0 })).unwrap());
        w.run("update_clip", &json!({ "id": tid, "content": "*b*", "start_ms": 5000, "duration_ms": 1000, "anim_in": "rise", "anim_ms": 300, "shadow": true })).unwrap();
        w.run("update_clip", &json!({ "id": oid, "placement": "full", "backdrop_dim": 2 })).unwrap();
        let t = w.project.scenes[0].tracks();
        let title = t.iter().flat_map(|t| &t.clips).find(|c| c.id == tid).unwrap();
        assert_eq!((title.output_start_ms, title.output_duration_ms(), title.link.unwrap().source_ms), (5000.0, 1000.0, 7000.0));
        let s = title.text.as_ref().unwrap();
        assert!(s.shadow && s.content == "*b*");
        assert_eq!(title.anim_in.as_ref().unwrap().duration_ms, 300.0);
        let ov = t.iter().flat_map(|t| &t.clips).find(|c| c.id == oid).unwrap();
        assert_eq!((ov.placement.as_deref(), ov.backdrop_dim), (Some("full"), 1.0));
        w.run("update_clip", &json!({ "id": tid, "pinned": false })).unwrap();
        assert!(w.project.scenes[0].tracks().iter().flat_map(|t| &t.clips).find(|c| c.id == tid).unwrap().link.is_none());

        assert!(w.run("update_clip", &json!({ "id": "nope" })).unwrap_err().contains("no title"));
        assert!(w.run("update_clip", &json!({ "id": tid, "start_ms": 99_000 })).is_err());
        w.run("remove_clip", &json!({ "id": tid })).unwrap();
        assert!(w.run("remove_clip", &json!({ "id": tid })).is_err());
        assert_eq!(w.project.scenes[0].tracks().iter().map(|t| t.clips.len()).sum::<usize>(), 1);
    }

    #[test]
    fn transitions_at_cuts() {
        let mut w = ws();
        w.run("set_transition", &json!({ "at_ms": 4010, "kind": "crossfade", "duration_ms": 5000 })).unwrap();
        let tr = w.project.scenes[0].slices[1].transition.clone().unwrap();
        assert_eq!((tr.kind.as_str(), tr.duration_ms), ("crossfade", 2000.0));
        w.run("set_transition", &json!({ "index": 0, "kind": "fade-black" })).unwrap();
        assert_eq!(w.project.scenes[0].slices[0].transition.as_ref().unwrap().duration_ms, Transition::DEFAULT_MS);
        assert!(w.run("set_transition", &json!({ "index": 0, "kind": "blur" })).unwrap_err().contains("first clip"));
        assert!(w.run("set_transition", &json!({ "at_ms": 2000, "kind": "blur" })).unwrap_err().contains("no cut"));
        assert!(w.run("set_transition", &json!({ "index": 5, "kind": "blur" })).is_err());
        assert!(w.run("set_transition", &json!({ "index": 1, "kind": "wipe" })).is_err());
        let v = overview(&mut w);
        assert_eq!(v["clips"][1]["transition"]["kind"], "crossfade");
        w.run("set_transition", &json!({ "index": 1, "kind": "none" })).unwrap();
        assert!(w.project.scenes[0].slices[1].transition.is_none());
    }
    #[test]
    fn title_cards_go_before_a_clip_and_come_out_with_their_titles() {
        let mut w = ws();
        let o = w.run("add_title_card", &json!({ "content": "Meet *Forge*", "subtitle": "In 60 seconds", "background": "#0F1220" })).unwrap();
        let v: Value = serde_json::from_str(&o.result).unwrap();
        let title = v["title_id"].as_str().unwrap().to_string();
        let scene = &w.project.scenes[0];
        assert!(scene.slices[0].is_card());
        assert_eq!(scene.slices[0].card.as_ref().unwrap().background.as_deref(), Some("#0f1220"));
        assert_eq!(scene.slices[1].transition.as_ref().unwrap().kind, "crossfade");
        let c = scene.tracks().iter().flat_map(|t| t.clips.clone()).find(|c| c.id == title).unwrap();
        assert!(c.link.is_none() && c.text.as_ref().unwrap().subtitle == "In 60 seconds");
        let overview: Value = serde_json::from_str(&w.run("get_project", &json!({})).unwrap().result).unwrap();
        assert!(overview["clips"][0]["titleCard"].is_object());
        // a title added on the card is not pinned to the recording
        let o = w.run("add_title", &json!({ "content": "on the card", "start_ms": 500 })).unwrap();
        let id: Value = serde_json::from_str(&o.result).unwrap();
        assert!(w.project.scenes[0].tracks().iter().flat_map(|t| t.clips.clone()).find(|c| Some(c.id.as_str()) == id["id"].as_str()).unwrap().link.is_none());

        assert!(w.run("add_title_card", &json!({ "content": "x", "index": 99 })).is_err());
        assert!(w.run("add_title_card", &json!({ "content": "x", "duration_ms": 100 })).is_err());
        assert!(w.run("add_title_card", &json!({ "content": "x", "background": "navy" })).is_err());
        assert!(w.run("remove_title_card", &json!({ "index": 1 })).unwrap_err().contains("not a title card"));
        w.run("remove_title_card", &json!({ "index": 0 })).unwrap();
        assert!(!w.project.scenes[0].slices[0].is_card());
        assert!(w.project.scenes[0].tracks().iter().flat_map(|t| &t.clips).all(|c| c.id != title), "its title went with it");
    }

}
