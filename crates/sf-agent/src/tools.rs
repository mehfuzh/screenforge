//! The tools Claude edits a project with.
//!
//! Every time a tool takes or returns is **output time** in ms: the position on the edited
//! timeline, as the user sees it under the playhead. The project stores zooms, masks and
//! layouts in source time (ms into the recording); the tools convert both ways so Claude never
//! has to reason about cuts and speed changes itself.

use std::sync::Arc;

use serde_json::{json, Value};
use sf_engine::captions::Word;
use sf_engine::timeline::{edit, Timeline};
use sf_project::schema::common::Point;
use sf_project::audio_clips::AudioTrack;
use sf_project::v4::{Slice, Zoom};
use sf_project::Project;

use crate::activity::Activity;
use crate::host::{Action, Host};

/// Pieces of a clip shorter than this (one frame at 60 fps) are dropped after a cut.
const MIN_PIECE_MS: f64 = 1000.0 / 60.0;
/// Gaps between spoken words at least this long are listed as pauses.
const PAUSE_MS: f64 = 700.0;
/// Shortest zoom, mask or camera layout (as when adding one by hand).
const MIN_ITEM_MS: f64 = 500.0;
/// The agent's zoom level when none is asked for. The editor's 2× click zoom is too close for
/// typing: a line of text runs off both sides while the camera follows the caret.
pub const AGENT_ZOOM_LEVEL: f64 = 1.5;

/// What the agent works on: a copy of the project plus what it can read but not change.
#[derive(Clone)]
pub struct Workspace {
    pub project: Project,
    /// Transcript words in source time (empty when the recording was not transcribed).
    pub words: Vec<Word>,
    /// Automatic zooms from the recording's clicks (source time), for `add_auto_zooms`.
    pub auto_zooms: Vec<Zoom>,
    /// Typing, clicks and idle stretches from the mouse and keyboard log; `None` when the
    /// recording has none (imported videos, iPhone recordings).
    pub activity: Option<Activity>,
    /// Set by the `seek` tool: where the editor should move the playhead.
    pub seek_to: Option<f64>,
    /// Pixel size of the cropped recording (mask areas are stored in it).
    pub content_px: (f64, f64),
    /// Audio recorded with the screen, for detaching it from the video.
    pub audio_tracks: Vec<AudioTrack>,
    /// Catalogs and app actions from the editor; `None` when there is no editor (tests, CLI).
    pub host: Option<Arc<dyn Host>>,
    /// App actions asked for since the editor last took them.
    pub actions: Vec<Action>,
    /// The project before each of the agent's earlier answers in this chat that changed it,
    /// oldest first, for `undo_answers`.
    pub earlier: Vec<Project>,
}

/// Result of a tool that ran.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Sent back to Claude.
    pub result: String,
    /// One line for the chat ("Cut 0:03.2 – 0:05.0").
    pub summary: String,
    /// The project changed.
    pub changed: bool,
    /// JPEG images sent back with the result (`view_frames`).
    pub images: Vec<Vec<u8>>,
}

impl Outcome {
    pub(crate) fn read(result: Value, summary: impl Into<String>) -> Self {
        Outcome { result: result.to_string(), summary: summary.into(), changed: false, images: vec![] }
    }
    pub(crate) fn edit(result: Value, summary: impl Into<String>) -> Self {
        Outcome { result: result.to_string(), summary: summary.into(), changed: true, images: vec![] }
    }
}

/// `m:ss.s` for the chat.
pub fn clock(ms: f64) -> String {
    let s = (ms.max(0.0) / 1000.0 * 10.0).round() / 10.0;
    format!("{}:{:04.1}", (s / 60.0).floor() as u64, s % 60.0)
}

/// Tool definitions for the Messages API `tools` field.
pub fn definitions() -> Value {
    let mut tools = timeline_definitions();
    if let Value::Array(list) = &mut tools {
        list.extend(crate::items::definitions());
    }
    tools
}

fn timeline_definitions() -> Value {
    let range = json!({
        "type": "object",
        "properties": {
            "start_ms": { "type": "number", "description": "Start on the timeline, ms" },
            "end_ms": { "type": "number", "description": "End on the timeline, ms" }
        },
        "required": ["start_ms", "end_ms"]
    });
    json!([
        {
            "name": "get_project",
            "description": "The project as it is now: duration, clips (with speed), zooms, masks, camera layouts, voice-overs, and whether there is a transcript. Times are ms on the timeline; items entirely inside a cut have null times. Call this before editing: the user may have changed the project by hand since your last turn.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_transcript",
            "description": "What was said, word by word, as [start_ms, end_ms, text] on the timeline, plus pauses (gaps of 0.7 s or more between words). Words that were cut are left out. Use it to find a moment by what is said, filler words, silences or retakes. Empty when the recording has no transcript (the user can create one in the Captions panel).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start_ms": { "type": "number", "description": "Only words from here (optional)" },
                    "end_ms": { "type": "number", "description": "Only words up to here (optional)" }
                }
            }
        },
        {
            "name": "get_activity",
            "description": "What happened in the recording, from its mouse and keyboard log: typing sections [start_ms, end_ms, keys], clicks, and idle stretches (no typing, clicking or mouse movement for 2.5 s or more), on the timeline. Use it for \"the typing part\", \"where nothing happens\" or \"the clicks\". Imported videos and iPhone recordings have no log: then use get_motion and view_frames.",
            "input_schema": { "type": "object", "properties": { "start_ms": { "type": "number" }, "end_ms": { "type": "number" } } }
        },
        {
            "name": "get_motion",
            "description": "What the picture does over time, measured from the video itself (works for every recording): sections that are still (nothing changes), small (a few pixels change in one place, like typing), or large (scrolling, switching windows, video). A cheap measure: confirm what a section shows with view_frames before acting on it.",
            "input_schema": { "type": "object", "properties": { "start_ms": { "type": "number" }, "end_ms": { "type": "number" } } }
        },
        {
            "name": "view_frames",
            "description": "Look at the video: stills of the screen at timeline times (at most 8 per call), returned as images. Use them to see what is on screen, find where something happens, or check a section from get_motion. Pick times that tell the sections apart; a few frames are usually enough.",
            "input_schema": {
                "type": "object",
                "properties": { "at_ms": { "type": "array", "items": { "type": "number" }, "minItems": 1, "maxItems": 8 } },
                "required": ["at_ms"]
            }
        },
        {
            "name": "get_config",
            "description": "Project settings as JSON (background, padding, corners, shadow, cursor, camera, captions, audio, animations, output aspect ratio …). `path` is a JSON pointer inside the project, default \"/config\". Read the current value before changing a setting with patch_project.",
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "JSON pointer, e.g. /config/background" } }
            }
        },
        {
            "name": "cut",
            "description": "Remove time ranges from the video (and its recorded audio). All ranges refer to the timeline before this call, so pass every range of one edit together instead of calling cut repeatedly. Later material moves earlier.",
            "input_schema": {
                "type": "object",
                "properties": { "ranges": { "type": "array", "items": range, "minItems": 1 } },
                "required": ["ranges"]
            }
        },
        {
            "name": "set_speed",
            "description": "Play a range at a different speed (2 = twice as fast, 0.5 = slow motion; 0.25 to 16). The clips are split at the range's edges. Use start_ms 0 and the duration for the whole video.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "speed": { "type": "number" }
                },
                "required": ["start_ms", "end_ms", "speed"]
            }
        },
        {
            "name": "split",
            "description": "Split in two at a time: the video clip there (a cut point without removing anything), or with id an audio clip, music part or voice-over.",
            "input_schema": { "type": "object", "properties": { "at_ms": { "type": "number" }, "id": { "type": "string" } }, "required": ["at_ms"] }
        },
        {
            "name": "add_zoom",
            "description": "Zoom into the screen over a range (at least 0.5 s; zooms cannot overlap, so change or remove one that is in the way). Without a target the zoom follows the cursor and clicks (best for most moments); with target {x, y} (0–1 across the recording, 0,0 top left) it stays on that point.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "level": { "type": "number", "description": "Magnification 1–4, default 1.5. About 1.3–1.5 for typing and text (the camera follows the caret), 1.8–2 only for a small button or detail" },
                    "target": { "type": "object", "properties": { "x": { "type": "number" }, "y": { "type": "number" } }, "required": ["x", "y"] }
                },
                "required": ["start_ms", "end_ms"]
            }
        },
        {
            "name": "update_zoom",
            "description": "Change a zoom from get_project: its range, level, target (null to follow the cursor again), disabled, or instant (no ease in and out).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "level": { "type": "number" },
                    "target": { "type": ["object", "null"], "properties": { "x": { "type": "number" }, "y": { "type": "number" } } },
                    "disabled": { "type": "boolean" },
                    "instant": { "type": "boolean" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "add_auto_zooms",
            "description": "Zoom in automatically around the recording's mouse clicks and typing, like the editor's automatic zoom button. Replaces the existing zooms unless keep_existing is true (then only the automatic zooms that fit between them are added).",
            "input_schema": { "type": "object", "properties": { "keep_existing": { "type": "boolean" }, "level": { "type": "number", "description": "Magnification for all of them, default 1.5" } } }
        },
        {
            "name": "patch_project",
            "description": "Change any value of the project file (project.json) with JSON Patch operations: {op: \"replace\" | \"add\" | \"remove\", path: JSON pointer, value}. Use it for settings (paths under /config) and for anything the other tools do not cover. Times stored in the file are source times (ms into the recording), not timeline times. The whole patch is rejected if the result is not a valid project.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": { "type": "string", "enum": ["replace", "add", "remove"] },
                                "path": { "type": "string" },
                                "value": {}
                            },
                            "required": ["op", "path"]
                        }
                    }
                },
                "required": ["ops"]
            }
        },
        {
            "name": "get_options",
            "description": "What the pickers offer and where every setting lives: wallpapers, gradients, music tracks, cursor sets, camera looks (LUTs), saved presets, recorded audio tracks, and a map of the sidebar's settings to project paths with their ranges. Read it before changing a setting you have not changed before.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "apply_preset",
            "description": "Apply one of the user's saved presets (names from get_options presets) to the project's look.",
            "input_schema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
        },
        {
            "name": "search_photos",
            "description": "Search Unsplash for a background photo. Returns photos with an id, a description when Unsplash has one, the photographer and the main colour; then call use_photo with the best match. An empty query lists popular wallpapers.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        },
        {
            "name": "use_photo",
            "description": "Download an Unsplash photo from search_photos into the project and make it the background.",
            "input_schema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        },
        {
            "name": "import_file",
            "description": "Bring a file the user named (a path on this computer) into the project and use it: as the background image, as music, or as a camera look (.cube LUT).",
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string" }, "use_as": { "type": "string", "enum": ["background", "music", "camera_look"] } },
                "required": ["path", "use_as"]
            }
        },
        {
            "name": "transcribe",
            "description": "Start transcribing the recording on this computer (as Captions → Generate transcript). It runs in the background for a minute or more and replaces an existing transcript; tell the user to ask again when it is done for edits that need the words.",
            "input_schema": { "type": "object", "properties": { "language": { "type": "string", "description": "Code such as en or de; omit to detect" } } }
        },
        {
            "name": "open_export",
            "description": "Open the export dialog so the user can export the video.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "undo_answers",
            "description": "Undo your own earlier answers in this chat: the project goes back to how it was before the last `count` answers that changed it (default 1), or before your first one with all: true. Use it for \"undo that\", \"undo all your changes\" or \"go back to how it was\" instead of reversing edits one by one. Edits the user made by hand after those answers are undone too, so say so when there were any. get_project's answersToUndo says how many there are.",
            "input_schema": { "type": "object", "properties": { "count": { "type": "integer", "minimum": 1 }, "all": { "type": "boolean" } } }
        },
        {
            "name": "seek",
            "description": "Move the editor's playhead, e.g. to show the user a moment you changed.",
            "input_schema": { "type": "object", "properties": { "at_ms": { "type": "number" } }, "required": ["at_ms"] }
        }
    ])
}

pub(crate) fn num(input: &Value, key: &str) -> Result<f64, String> {
    input.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("`{key}` must be a number"))
}

pub(crate) fn opt_num(input: &Value, key: &str) -> Result<Option<f64>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => num(input, key).map(Some),
    }
}

fn point(v: &Value) -> Result<Point, String> {
    let (x, y) = (num(v, "x")?, num(v, "y")?);
    Ok(Point::new(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

/// `[a, b)` minus the (sorted, possibly overlapping) `holes`.
fn subtract(a: f64, b: f64, holes: &mut [(f64, f64)]) -> Vec<(f64, f64)> {
    holes.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out = vec![];
    let mut at = a;
    for &(h0, h1) in holes.iter() {
        if h0 > at {
            out.push((at, h0.min(b)));
        }
        at = at.max(h1);
        if at >= b {
            break;
        }
    }
    if at < b {
        out.push((at, b));
    }
    out.retain(|(s, e)| e - s >= MIN_PIECE_MS);
    out
}

impl Workspace {
    pub fn new(project: Project) -> Self {
        Workspace { project, words: vec![], auto_zooms: vec![], activity: None, seek_to: None, content_px: (1920.0, 1080.0), audio_tracks: vec![], host: None, actions: vec![], earlier: vec![] }
    }

    pub(crate) fn timeline(&self) -> Timeline {
        self.project.scenes.first().map(Timeline::from_scene).unwrap_or_default()
    }

    fn slices_mut(&mut self) -> Result<&mut Vec<Slice>, String> {
        self.project.scenes.first_mut().map(|s| &mut s.slices).ok_or_else(|| "the project has no scene".to_string())
    }

    /// Where the source range `[s0, s1)` shows on the timeline (first to last visible part).
    pub(crate) fn output_range(tl: &Timeline, s0: f64, s1: f64) -> Option<(f64, f64)> {
        let mut out: Option<(f64, f64)> = None;
        for sp in tl.spans() {
            let (a, b) = (s0.max(sp.source_start_ms), s1.min(sp.source_end_ms));
            if b > a {
                let (oa, ob) = (sp.source_to_output(a), sp.source_to_output(b));
                out = Some(out.map_or((oa, ob), |(x, y)| (x.min(oa), y.max(ob))));
            }
        }
        out
    }

    /// Source range for the timeline range `[t0, t1)`.
    pub(crate) fn source_range(tl: &Timeline, t0: f64, t1: f64) -> Result<(f64, f64), String> {
        let dur = tl.duration_ms();
        let (t0, t1) = (t0.clamp(0.0, dur), t1.clamp(0.0, dur));
        if t1 - t0 < 1.0 {
            return Err(format!("the range {}–{} is empty or outside the video ({} long)", clock(t0), clock(t1), clock(dur)));
        }
        let s0 = tl.output_to_source(t0).ok_or("start is outside the video")?;
        let s1 = tl.output_to_source((t1 - 0.5).max(t0)).ok_or("end is outside the video")? + 0.5;
        Ok((s0, s1.max(s0 + 1.0)))
    }

    /// Run tool `name` with `input`. `Err` is reported back to Claude as a failed tool call.
    pub fn run(&mut self, name: &str, input: &Value) -> Result<Outcome, String> {
        if self.project.scenes.is_empty() {
            return Err("the project has no scene".into());
        }
        let outcome = match self.run_item(name, input) {
            Some(r) => r,
            None => self.run_timeline(name, input),
        }?;
        if outcome.changed {
            // as after every edit in the editor: audio items that now overlap move to free lanes
            let tl = self.timeline();
            sf_engine::lanes::settle(&mut self.project, &tl);
        }
        Ok(outcome)
    }

    /// The other side of an item's range that it may not overlap: `(id, start, end)` of the
    /// items of one lane in source ms.
    fn check_free(&self, lane: &[(String, f64, f64)], what: &str, s0: f64, s1: f64, except: Option<&str>) -> Result<(), String> {
        if s1 - s0 < MIN_ITEM_MS {
            return Err(format!("a {what} must be at least 0.5 s long"));
        }
        let tl = self.timeline();
        for (id, a, b) in lane {
            if Some(id.as_str()) != except && s0 < *b && *a < s1 {
                let at = Self::output_range(&tl, *a, *b).map(|(x, y)| format!(" ({} – {})", clock(x), clock(y))).unwrap_or_default();
                return Err(format!("this overlaps {what} {id}{at}; {what}s cannot overlap, so change or remove that one first"));
            }
        }
        Ok(())
    }

    fn zoom_lane(&self) -> Vec<(String, f64, f64)> {
        self.project.scenes[0].zooms.iter().map(|z| (z.id.clone(), z.source_start_ms, z.source_end_ms)).collect()
    }

    pub(crate) fn mask_lane(&self) -> Vec<(String, f64, f64)> {
        self.project.scenes[0].masks.iter().map(|m| (m.id.clone(), m.source_start_ms, m.source_end_ms)).collect()
    }

    pub(crate) fn layout_lane(&self) -> Vec<(String, f64, f64)> {
        self.project.scenes[0].layouts.iter().map(|l| (l.id.clone(), l.source_start_ms, l.source_end_ms)).collect()
    }

    pub(crate) fn free(&self, lane: &[(String, f64, f64)], what: &str, s0: f64, s1: f64, except: Option<&str>) -> Result<(), String> {
        self.check_free(lane, what, s0, s1, except)
    }

    fn host(&self) -> Result<&dyn Host, String> {
        self.host.as_deref().ok_or_else(|| "not available here (no editor)".to_string())
    }

    fn run_timeline(&mut self, name: &str, input: &Value) -> Result<Outcome, String> {
        match name {
            "get_project" => Ok(Outcome::read(self.overview(), "Read the timeline")),
            "get_transcript" => self.transcript(opt_num(input, "start_ms")?, opt_num(input, "end_ms")?),
            "get_config" => {
                let path = input.get("path").and_then(Value::as_str).unwrap_or("/config");
                let doc = serde_json::to_value(&self.project).map_err(|e| e.to_string())?;
                let v = doc.pointer(path).cloned().ok_or_else(|| format!("nothing at {path}"))?;
                Ok(Outcome::read(v, format!("Read settings {path}")))
            }
            "cut" => {
                let ranges = input.get("ranges").and_then(Value::as_array).ok_or("`ranges` must be a list")?;
                let ranges = ranges.iter().map(|r| Ok((num(r, "start_ms")?, num(r, "end_ms")?))).collect::<Result<Vec<_>, String>>()?;
                self.cut(&ranges)
            }
            "set_speed" => self.set_speed(num(input, "start_ms")?, num(input, "end_ms")?, num(input, "speed")?),
            "split" => {
                let t = num(input, "at_ms")?;
                if let Some(id) = input.get("id").and_then(Value::as_str) {
                    return self.split_audio(id, t);
                }
                let slices = self.slices_mut()?;
                let before = slices.len();
                *slices = edit::split_at_output(slices, t);
                if slices.len() == before {
                    return Err(format!("nothing to split at {} (outside the video, or on an existing cut)", clock(t)));
                }
                Ok(Outcome::edit(json!({ "ok": true }), format!("Split at {}", clock(t))))
            }
            "add_zoom" => {
                let (t0, t1) = (num(input, "start_ms")?, num(input, "end_ms")?);
                let (s0, s1) = Self::source_range(&self.timeline(), t0, t1)?;
                self.check_free(&self.zoom_lane(), "zoom", s0, s1, None)?;
                let level = opt_num(input, "level")?.unwrap_or(AGENT_ZOOM_LEVEL).clamp(1.0, 4.0);
                let z = match input.get("target").filter(|v| !v.is_null()) {
                    Some(t) => Zoom::manual(s0, s1, level, point(t)?),
                    None => Zoom::auto(s0, s1, level),
                };
                let id = z.id.clone();
                self.project.scenes[0].zooms.push(z);
                Ok(Outcome::edit(json!({ "id": id }), format!("Zoom {level}× {} – {}", clock(t0), clock(t1))))
            }
            "update_zoom" => self.update_zoom(input),
            "add_auto_zooms" => {
                if self.auto_zooms.is_empty() {
                    return Err("the recording has no clicks to zoom to".into());
                }
                let keep = input.get("keep_existing").and_then(Value::as_bool).unwrap_or(false);
                let lane = if keep { self.zoom_lane() } else { vec![] };
                let level = opt_num(input, "level")?.unwrap_or(AGENT_ZOOM_LEVEL).clamp(1.0, 4.0);
                let fits: Vec<Zoom> = self.auto_zooms.iter().filter(|z| self.check_free(&lane, "zoom", z.source_start_ms, z.source_end_ms, None).is_ok()).map(|z| Zoom { zoom: level, ..z.clone() }).collect();
                let n = fits.len();
                let zooms = &mut self.project.scenes[0].zooms;
                if !keep {
                    zooms.clear();
                }
                zooms.extend(fits);
                zooms.sort_by(|a, b| a.source_start_ms.total_cmp(&b.source_start_ms));
                Ok(Outcome::edit(json!({ "added": n }), format!("Added {n} automatic zooms")))
            }
            "patch_project" => self.patch(input.get("ops").and_then(Value::as_array).ok_or("`ops` must be a list")?),
            "get_options" => Ok(Outcome::read(crate::host::options(self.host.as_deref()), "Read the options")),
            "get_activity" => self.activity_tool(opt_num(input, "start_ms")?, opt_num(input, "end_ms")?),
            "get_motion" => {
                let (from, to) = (opt_num(input, "start_ms")?.unwrap_or(0.0), opt_num(input, "end_ms")?.unwrap_or(f64::INFINITY));
                let tl = self.timeline();
                let sections: Vec<Value> = self
                    .host()?
                    .screen_motion()?
                    .into_iter()
                    .filter_map(|(s0, s1, m)| Self::output_range(&tl, s0, s1).map(|(a, b)| (a, b, m)))
                    .filter(|(a, b, _)| *b > from && *a < to)
                    .map(|(a, b, m)| json!([a.round(), b.round(), m.label()]))
                    .collect();
                let n = sections.len();
                Ok(Outcome::read(json!({ "sections": sections }), format!("Measured motion ({n} sections)")))
            }
            "view_frames" => {
                let times: Vec<f64> = input.get("at_ms").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).take(8).collect()).unwrap_or_default();
                if times.is_empty() {
                    return Err("`at_ms` needs at least one time".into());
                }
                let tl = self.timeline();
                let dur = tl.duration_ms();
                let source: Vec<f64> = times.iter().map(|t| tl.output_to_source(t.clamp(0.0, (dur - 1.0).max(0.0))).ok_or_else(|| format!("{} is outside the video", clock(*t)))).collect::<Result<_, _>>()?;
                let images = self.host()?.screen_frames(&source)?;
                let labels: Vec<Value> = times.iter().map(|t| json!(clock(*t))).collect();
                let n = images.len();
                Ok(Outcome { result: json!({ "frames": labels, "note": "The images follow, in the same order." }).to_string(), summary: format!("Looked at {n} frames"), changed: false, images })
            }
            "apply_preset" => {
                let name = input.get("name").and_then(Value::as_str).ok_or("`name` is required")?;
                let host = self.host.clone().ok_or("not available here (no editor)")?;
                let mut p = self.project.clone();
                host.apply_preset(name, &mut p)?;
                self.project = p;
                Ok(Outcome::edit(json!({ "ok": true }), format!("Applied preset {name}")))
            }
            "search_photos" => {
                let q = input.get("query").and_then(Value::as_str).unwrap_or("");
                let photos = self.host()?.search_photos(q)?;
                let n = photos.as_array().map_or(0, Vec::len);
                Ok(Outcome::read(json!({ "photos": photos }), format!("Searched Unsplash for \"{q}\" ({n} photos)")))
            }
            "use_photo" => {
                let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
                let value = self.host()?.download_photo(id)?;
                let author = value.get("author").and_then(Value::as_str).unwrap_or("Unsplash").to_string();
                let bg = &mut self.project.config.styles.background;
                bg.kind = sf_project::v4::background_type::UNSPLASH.into();
                bg.unsplash = Some(value);
                Ok(Outcome::edit(json!({ "ok": true }), format!("Background: photo by {author}")))
            }
            "import_file" => {
                let path = input.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
                let use_as = input.get("use_as").and_then(Value::as_str).ok_or("`use_as` is required")?;
                if !["background", "music", "camera_look"].contains(&use_as) {
                    return Err(format!("unknown use_as {use_as}"));
                }
                let name = self.host()?.import_file(path)?;
                let cfg = &mut self.project.config;
                match use_as {
                    "background" => {
                        cfg.styles.background.kind = sf_project::v4::background_type::IMAGE.into();
                        cfg.styles.background.image = Some(Value::String(name.clone()));
                    }
                    "music" => {
                        cfg.audio.background_audio_file_name = Some(name.clone());
                        cfg.audio.mute_background_audio = false;
                        if cfg.audio.background_audio_volume < 0.2 {
                            cfg.audio.background_audio_volume = 0.3;
                        }
                    }
                    _ => cfg.camera.lut = Some(name.clone()),
                }
                Ok(Outcome::edit(json!({ "fileName": name }), format!("Imported {name}")))
            }
            "transcribe" => {
                self.host()?;
                let language = input.get("language").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
                self.actions.push(Action::Transcribe { language });
                Ok(Outcome::read(json!({ "started": true, "note": "Transcribing runs in the background; the transcript is not available in this turn." }), "Started transcribing"))
            }
            "open_export" => {
                self.host()?;
                self.actions.push(Action::OpenExport);
                Ok(Outcome::read(json!({ "ok": true }), "Opened the export dialog"))
            }
            "undo_answers" => {
                let n = self.earlier.len();
                if n == 0 {
                    return Err("there is nothing of yours to undo in this chat".into());
                }
                let count = if input.get("all").and_then(Value::as_bool) == Some(true) { n } else { opt_num(input, "count")?.map_or(1, |c| c.max(1.0) as usize).min(n) };
                // a further undo in the same turn goes back from here
                self.project = self.earlier[n - count].clone();
                self.earlier.truncate(n - count);
                let what = if count == 1 { "your last answer".to_string() } else { format!("your last {count} answers") };
                Ok(Outcome::edit(json!({ "undone": count, "durationMs": self.timeline().duration_ms().round(), "answersToUndo": self.earlier.len() }), format!("Undid {what}")))
            }
            "seek" => {
                let t = num(input, "at_ms")?.clamp(0.0, self.timeline().duration_ms());
                self.seek_to = Some(t);
                Ok(Outcome::read(json!({ "ok": true }), format!("Moved the playhead to {}", clock(t))))
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }

    fn activity_tool(&self, from: Option<f64>, to: Option<f64>) -> Result<Outcome, String> {
        let Some(a) = self.activity.as_ref().filter(|a| !a.is_empty()) else {
            return Ok(Outcome::read(json!({ "note": "No mouse or keyboard log for this recording (an imported video or an iPhone recording). Use get_motion and view_frames to find what happens when." }), "No input log"));
        };
        let tl = self.timeline();
        let (from, to) = (from.unwrap_or(0.0), to.unwrap_or(f64::INFINITY));
        let within = |a: f64, b: f64| b > from && a < to;
        let typing: Vec<Value> = a.typing.iter().filter_map(|(s0, s1, n)| Self::output_range(&tl, *s0, *s1).filter(|(x, y)| within(*x, *y)).map(|(x, y)| json!([x.round(), y.round(), n]))).collect();
        let idle: Vec<Value> = a.idle.iter().filter_map(|(s0, s1)| Self::output_range(&tl, *s0, *s1).filter(|(x, y)| within(*x, *y)).map(|(x, y)| json!([x.round(), y.round()]))).collect();
        let clicks: Vec<Value> = a.clicks.iter().filter_map(|t| tl.source_to_output(*t)).filter(|t| within(*t, *t)).take(300).map(|t| json!(t.round())).collect();
        let summary = format!("Read the activity ({} typing, {} idle)", typing.len(), idle.len());
        Ok(Outcome::read(json!({ "typing": typing, "clicks": clicks, "idle": idle }), summary))
    }

    fn overview(&self) -> Value {
        let tl = self.timeline();
        let ms = |v: f64| (v * 10.0).round() / 10.0;
        let range = |s0: f64, s1: f64| match Self::output_range(&tl, s0, s1) {
            Some((a, b)) => (json!(ms(a)), json!(ms(b))),
            None => (Value::Null, Value::Null),
        };
        let Some(scene) = self.project.scenes.first() else { return json!({ "error": "the project has no scene" }) };
        let clips: Vec<Value> = tl
            .spans()
            .iter()
            .map(|s| json!({ "index": s.index, "startMs": ms(s.output_start_ms), "endMs": ms(s.output_end_ms), "sourceStartMs": ms(s.source_start_ms), "sourceEndMs": ms(s.source_end_ms), "speed": ms(1.0 / s.time_scale) }))
            .collect();
        let zooms: Vec<Value> = scene
            .zooms
            .iter()
            .map(|z| {
                let (a, b) = range(z.source_start_ms, z.source_end_ms);
                json!({ "id": z.id, "startMs": a, "endMs": b, "level": z.zoom, "followsCursor": z.is_auto(), "target": z.manual_target_point.filter(|_| !z.is_auto()).map(|p| json!({ "x": p.x, "y": p.y })), "disabled": z.is_disabled })
            })
            .collect();
        let masks: Vec<Value> = scene
            .masks
            .iter()
            .map(|m| {
                let (a, b) = range(m.source_start_ms, m.source_end_ms);
                json!({ "id": m.id, "startMs": a, "endMs": b, "kind": if m.kind == sf_project::v4::Mask::HIGHLIGHT { "highlight" } else { "blur" }, "disabled": m.is_disabled })
            })
            .collect();
        let layouts: Vec<Value> = scene
            .layouts
            .iter()
            .map(|l| {
                let (a, b) = range(l.source_start_ms, l.source_end_ms);
                json!({ "id": l.id, "startMs": a, "endMs": b, "type": l.kind, "disabled": l.is_disabled })
            })
            .collect();
        json!({
            "name": self.project.name,
            "durationMs": ms(tl.duration_ms()),
            "clips": clips,
            "zooms": zooms,
            "masks": masks,
            "cameraLayouts": layouts,
            "audio": self.audio_overview(&tl),
            "hasTranscript": !self.words.is_empty(),
            "hasClicksForAutoZoom": !self.auto_zooms.is_empty(),
            "answersToUndo": self.earlier.len(),
        })
    }

    fn transcript(&self, from: Option<f64>, to: Option<f64>) -> Result<Outcome, String> {
        if self.words.is_empty() {
            return Ok(Outcome::read(json!({ "words": [], "note": "No transcript. The user can transcribe the recording in the Captions panel." }), "No transcript yet"));
        }
        let tl = self.timeline();
        let (from, to) = (from.unwrap_or(0.0), to.unwrap_or(f64::INFINITY));
        let mut words = vec![];
        let mut pauses = vec![];
        let mut prev_end: Option<f64> = None;
        for w in &self.words {
            let (Some(a), Some(b)) = (tl.source_to_output(w.start_ms), tl.source_to_output((w.end_ms - 0.5).max(w.start_ms)).map(|b| b + 0.5)) else { continue };
            if b < from || a > to {
                continue;
            }
            if let Some(p) = prev_end.filter(|p| a - p >= PAUSE_MS) {
                pauses.push(json!([p.round(), a.round()]));
            }
            prev_end = Some(b);
            words.push(json!([a.round(), b.max(a).round(), w.text]));
        }
        let n = words.len();
        Ok(Outcome::read(json!({ "words": words, "pauses": pauses }), format!("Read the transcript ({n} words)")))
    }

    fn cut(&mut self, ranges: &[(f64, f64)]) -> Result<Outcome, String> {
        let tl = self.timeline();
        let dur = tl.duration_ms();
        let slices = self.slices_mut()?;
        let mut holes: Vec<Vec<(f64, f64)>> = vec![vec![]; slices.len()];
        for &(a, b) in ranges {
            let (a, b) = (a.clamp(0.0, dur), b.clamp(0.0, dur));
            for sp in tl.spans() {
                let (s, e) = (a.max(sp.output_start_ms), b.min(sp.output_end_ms));
                if e > s {
                    holes[sp.index].push((sp.output_to_source(s), sp.output_to_source(e)));
                }
            }
        }
        if holes.iter().all(Vec::is_empty) {
            return Err(format!("nothing to cut: the ranges are outside the video (0:00.0 – {})", clock(dur)));
        }
        let mut out = vec![];
        for (slice, mut h) in slices.iter().zip(holes) {
            if h.is_empty() {
                out.push(slice.clone());
                continue;
            }
            for (i, (s0, s1)) in subtract(slice.source_start_ms, slice.source_end_ms, &mut h).into_iter().enumerate() {
                let mut piece = slice.clone();
                if i > 0 {
                    piece.id = sf_project::ids::new_id();
                }
                piece.source_start_ms = s0;
                piece.source_end_ms = s1;
                out.push(piece);
            }
        }
        if out.is_empty() {
            return Err("that would remove the whole video".into());
        }
        *slices = out;
        let removed = dur - self.timeline().duration_ms();
        let summary = match ranges {
            [(a, b)] => format!("Cut {} – {}", clock(*a), clock(*b)),
            _ => format!("Cut {} parts ({:.1} s)", ranges.len(), removed / 1000.0),
        };
        Ok(Outcome::edit(json!({ "removedMs": removed.round(), "durationMs": self.timeline().duration_ms().round() }), summary))
    }

    fn set_speed(&mut self, t0: f64, t1: f64, speed: f64) -> Result<Outcome, String> {
        let dur = self.timeline().duration_ms();
        let (t0, t1) = (t0.clamp(0.0, dur), t1.clamp(0.0, dur));
        if t1 - t0 < 1.0 {
            return Err(format!("the range is empty or outside the video ({} long)", clock(dur)));
        }
        let speed = speed.clamp(0.25, 16.0);
        let slices = self.slices_mut()?;
        let mut s = edit::split_at_output(slices, t1);
        s = edit::split_at_output(&s, t0);
        let tl = Timeline::from_slices(&s);
        let inside: Vec<usize> = tl.spans().iter().filter(|sp| sp.output_start_ms >= t0 - 1.0 && sp.output_end_ms <= t1 + 1.0).map(|sp| sp.index).collect();
        for i in inside {
            s = edit::set_speed(&s, i, speed);
        }
        *slices = s;
        Ok(Outcome::edit(json!({ "durationMs": self.timeline().duration_ms().round() }), format!("Speed {speed}× for {} – {}", clock(t0), clock(t1))))
    }

    fn update_zoom(&mut self, input: &Value) -> Result<Outcome, String> {
        let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
        let tl = self.timeline();
        let range = match (opt_num(input, "start_ms")?, opt_num(input, "end_ms")?) {
            (None, None) => None,
            (a, b) => {
                let z = self.project.scenes.first().and_then(|s| s.zooms.iter().find(|z| z.id == id)).ok_or_else(|| format!("no zoom {id}"))?;
                let cur = Self::output_range(&tl, z.source_start_ms, z.source_end_ms).unwrap_or((0.0, 0.0));
                Some(Self::source_range(&tl, a.unwrap_or(cur.0), b.unwrap_or(cur.1))?)
            }
        };
        if let Some((s0, s1)) = range {
            self.check_free(&self.zoom_lane(), "zoom", s0, s1, Some(id))?;
        }
        let z = self.project.scenes.first_mut().and_then(|s| s.zooms.iter_mut().find(|z| z.id == id)).ok_or_else(|| format!("no zoom {id}"))?;
        if let Some((s0, s1)) = range {
            z.source_start_ms = s0.floor();
            z.source_end_ms = s1.floor();
        }
        if let Some(l) = opt_num(input, "level")? {
            z.zoom = l.clamp(1.0, 4.0);
        }
        match input.get("target") {
            Some(Value::Null) => {
                z.kind = sf_project::v4::zoom_type::FOLLOW_CLICK_GROUPS.into();
            }
            Some(t) => {
                z.kind = sf_project::v4::zoom_type::MANUAL.into();
                z.manual_target_point = Some(point(t)?);
            }
            None => {}
        }
        if let Some(d) = input.get("disabled").and_then(Value::as_bool) {
            z.is_disabled = d;
        }
        if let Some(i) = input.get("instant").and_then(Value::as_bool) {
            z.has_instant_animation = i;
        }
        Ok(Outcome::edit(json!({ "ok": true }), "Changed a zoom"))
    }

    fn patch(&mut self, ops: &[Value]) -> Result<Outcome, String> {
        let mut doc = serde_json::to_value(&self.project).map_err(|e| e.to_string())?;
        let mut paths = vec![];
        for op in ops {
            let kind = op.get("op").and_then(Value::as_str).ok_or("each op needs `op`")?;
            let path = op.get("path").and_then(Value::as_str).ok_or("each op needs `path`")?;
            if ["/id", "/version", "/createdAt"].contains(&path) {
                return Err(format!("{path} cannot be changed"));
            }
            apply_op(&mut doc, kind, path, op.get("value").cloned())?;
            paths.push(path.to_string());
        }
        let project: Project = serde_json::from_value(doc).map_err(|e| format!("the patched project is not valid: {e}"))?;
        if project.scenes.is_empty() || project.scenes[0].slices.is_empty() {
            return Err("the project needs a scene with at least one clip".into());
        }
        self.project = project;
        let summary = match paths.as_slice() {
            [p] => format!("Changed {}", p.trim_start_matches("/config/").trim_start_matches('/')),
            _ => format!("Changed {} settings", paths.len()),
        };
        Ok(Outcome::edit(json!({ "ok": true }), summary))
    }
}

fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// One JSON Patch (RFC 6902) `add` / `replace` / `remove` on `doc`.
fn apply_op(doc: &mut Value, op: &str, path: &str, value: Option<Value>) -> Result<(), String> {
    let (parent, last) = path.rsplit_once('/').ok_or_else(|| format!("`{path}` is not a JSON pointer (it starts with /)"))?;
    let key = unescape(last);
    let target = doc.pointer_mut(parent).ok_or_else(|| format!("nothing at {}", if parent.is_empty() { "/" } else { parent }))?;
    let need_value = || value.clone().ok_or_else(|| format!("`{op}` at {path} needs a value"));
    match (op, target) {
        ("replace", t) => {
            let slot = t.pointer_mut(&format!("/{last}")).ok_or_else(|| format!("nothing at {path} to replace (use add)"))?;
            *slot = need_value()?;
        }
        ("add", Value::Object(m)) => {
            m.insert(key, need_value()?);
        }
        ("add", Value::Array(a)) => {
            let v = need_value()?;
            if key == "-" {
                a.push(v);
            } else {
                let i: usize = key.parse().map_err(|_| format!("bad array index in {path}"))?;
                if i > a.len() {
                    return Err(format!("index {i} is past the end of {parent}"));
                }
                a.insert(i, v);
            }
        }
        ("remove", Value::Object(m)) => {
            m.remove(&key).ok_or_else(|| format!("nothing at {path}"))?;
        }
        ("remove", Value::Array(a)) => {
            let i: usize = key.parse().map_err(|_| format!("bad array index in {path}"))?;
            if i >= a.len() {
                return Err(format!("nothing at {path}"));
            }
            a.remove(i);
        }
        ("add" | "remove", _) => return Err(format!("{parent} is not an object or a list")),
        _ => return Err(format!("unsupported op `{op}` (use replace, add or remove)")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 s recording in one clip, with words at 1 s, 2 s and 6 s.
    fn ws() -> Workspace {
        let mut p = Project::new("Test");
        p.scenes[0].slices = vec![Slice::new(0.0, 10_000.0)];
        let mut w = Workspace::new(p);
        let word = |a: f64, b: f64, t: &str| Word { start_ms: a, end_ms: b, text: t.into() };
        w.words = vec![word(1000.0, 1400.0, "hello"), word(2000.0, 2300.0, "um"), word(6000.0, 6500.0, "world")];
        w
    }

    fn duration(w: &Workspace) -> f64 {
        w.timeline().duration_ms()
    }

    #[test]
    fn cut_several_ranges_at_once() {
        let mut w = ws();
        let o = w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 1000 }, { "start_ms": 5000, "end_ms": 6000 }] })).unwrap();
        assert!(o.changed);
        assert_eq!(duration(&w), 8000.0);
        let s = &w.project.scenes[0].slices;
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].source_start_ms, s[0].source_end_ms), (1000.0, 5000.0));
        assert_eq!((s[1].source_start_ms, s[1].source_end_ms), (6000.0, 10_000.0));
        assert_ne!(s[0].id, s[1].id);
    }

    #[test]
    fn cut_follows_speed_and_existing_cuts() {
        let mut w = ws();
        w.run("set_speed", &json!({ "start_ms": 0, "end_ms": 4000, "speed": 2 })).unwrap();
        assert_eq!(duration(&w), 8000.0); // 4 s of source plays in 2 s
        // timeline 1000–2000 is source 2000–4000 (the 2× part)
        w.run("cut", &json!({ "ranges": [{ "start_ms": 1000, "end_ms": 2000 }] })).unwrap();
        assert_eq!(duration(&w), 7000.0);
        let s = &w.project.scenes[0].slices;
        assert_eq!((s[0].source_start_ms, s[0].source_end_ms), (0.0, 2000.0));
        assert_eq!(s[0].time_scale, 0.5);
        assert_eq!(s[1].source_start_ms, 4000.0);
    }

    #[test]
    fn cutting_everything_is_refused() {
        let mut w = ws();
        assert!(w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 10_000 }] })).is_err());
        assert!(w.run("cut", &json!({ "ranges": [{ "start_ms": 20_000, "end_ms": 30_000 }] })).is_err());
        assert_eq!(duration(&w), 10_000.0);
    }

    #[test]
    fn transcript_is_in_timeline_time_and_skips_cut_words() {
        let mut w = ws();
        w.run("cut", &json!({ "ranges": [{ "start_ms": 1900, "end_ms": 2400 }] })).unwrap();
        let o = w.run("get_transcript", &json!({})).unwrap();
        let v: Value = serde_json::from_str(&o.result).unwrap();
        let words = v["words"].as_array().unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[1], json!([5500.0, 6000.0, "world"]));
        assert_eq!(v["pauses"], json!([[1400.0, 5500.0]]));
    }

    #[test]
    fn zoom_is_stored_in_source_time() {
        let mut w = ws();
        w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 2000 }] })).unwrap();
        let o = w.run("add_zoom", &json!({ "start_ms": 1000, "end_ms": 3000, "level": 9, "target": { "x": 0.2, "y": 0.3 } })).unwrap();
        let id: String = serde_json::from_str::<Value>(&o.result).unwrap()["id"].as_str().unwrap().into();
        let z = &w.project.scenes[0].zooms[0];
        assert_eq!((z.source_start_ms, z.source_end_ms, z.zoom), (3000.0, 5000.0, 4.0));
        assert!(!z.is_auto());
        w.run("update_zoom", &json!({ "id": id, "target": null, "end_ms": 4000 })).unwrap();
        let z = &w.project.scenes[0].zooms[0];
        assert!(z.is_auto());
        assert_eq!(z.source_end_ms, 6000.0);
        let v: Value = serde_json::from_str(&w.run("get_project", &json!({})).unwrap().result).unwrap();
        assert_eq!(v["zooms"][0]["startMs"], json!(1000.0));
        w.run("remove", &json!({ "all": "zooms" })).unwrap();
        assert!(w.project.scenes[0].zooms.is_empty());
    }

    #[test]
    fn patch_changes_settings_and_rejects_invalid_projects() {
        let mut w = ws();
        let before = w.project.clone();
        let o = w.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/name", "value": "Renamed" }] })).unwrap();
        assert!(o.changed);
        assert_eq!(w.project.name, "Renamed");
        // a number where a string belongs: rejected, nothing changes
        let mut bad = ws();
        bad.project = before.clone();
        assert!(bad.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/name", "value": 3 }] })).is_err());
        assert_eq!(bad.project, before);
        assert!(bad.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/id", "value": "x" }] })).is_err());
        assert!(bad.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/nope/deeper", "value": 1 }] })).is_err());
        // add then remove an unknown key round-trips through `extra`
        w.run("patch_project", &json!({ "ops": [{ "op": "add", "path": "/data/note", "value": "hi" }] })).unwrap();
        assert_eq!(w.project.data["note"], json!("hi"));
        w.run("patch_project", &json!({ "ops": [{ "op": "remove", "path": "/data/note" }] })).unwrap();
        assert!(!w.project.data.contains_key("note"));
    }

    #[test]
    fn every_config_value_round_trips_through_get_and_patch() {
        let mut w = ws();
        let cfg: Value = serde_json::from_str(&w.run("get_config", &json!({})).unwrap().result).unwrap();
        let first = cfg.as_object().unwrap().keys().next().unwrap().clone();
        let path = format!("/config/{first}");
        let o = w.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": path, "value": cfg[&first] }] }));
        assert!(o.is_ok(), "{o:?}");
    }

    #[test]
    fn split_and_seek() {
        let mut w = ws();
        w.run("split", &json!({ "at_ms": 4000 })).unwrap();
        assert_eq!(w.project.scenes[0].slices.len(), 2);
        assert!(w.run("split", &json!({ "at_ms": 4000 })).is_err());
        w.run("seek", &json!({ "at_ms": 99_000 })).unwrap();
        assert_eq!(w.seek_to, Some(10_000.0));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let mut w = ws();
        assert!(w.run("cut", &json!({})).is_err());
        assert!(w.run("set_speed", &json!({ "start_ms": "a" })).is_err());
        assert!(w.run("add_zoom", &json!({ "start_ms": 5, "end_ms": 5 })).is_err());
        assert!(w.run("teleport", &json!({})).is_err());
    }

    #[test]
    fn zooms_cannot_overlap_or_be_slivers() {
        let mut w = ws();
        let id = serde_json::from_str::<Value>(&w.run("add_zoom", &json!({ "start_ms": 2000, "end_ms": 5000 })).unwrap().result).unwrap()["id"].as_str().unwrap().to_string();
        let e = w.run("add_zoom", &json!({ "start_ms": 4000, "end_ms": 7000, "level": 1.2 })).unwrap_err();
        assert!(e.contains(&id) && e.contains("0:02.0"), "{e}");
        assert!(w.run("add_zoom", &json!({ "start_ms": 0, "end_ms": 300 })).unwrap_err().contains("0.5 s"));
        // next to it is fine, and a zoom may be moved over its own old range
        w.run("add_zoom", &json!({ "start_ms": 5000, "end_ms": 7000 })).unwrap();
        w.run("update_zoom", &json!({ "id": id, "start_ms": 1000, "end_ms": 4000 })).unwrap();
        assert!(w.run("update_zoom", &json!({ "id": id, "end_ms": 6000 })).is_err());
        assert_eq!(w.project.scenes[0].zooms.len(), 2);
    }

    #[test]
    fn auto_zooms_keep_existing_only_fill_the_gaps() {
        let mut w = ws();
        w.auto_zooms = vec![Zoom::auto(1000.0, 3000.0, 2.0), Zoom::auto(6000.0, 8000.0, 2.0)];
        w.run("add_zoom", &json!({ "start_ms": 2000, "end_ms": 4000 })).unwrap();
        let o = w.run("add_auto_zooms", &json!({ "keep_existing": true })).unwrap();
        assert_eq!(o.summary, "Added 1 automatic zooms");
        assert_eq!(w.project.scenes[0].zooms.len(), 2);
        assert!(w.project.scenes[0].zooms.iter().all(|z| z.zoom == AGENT_ZOOM_LEVEL), "the agent's zooms default to 1.5×");
        w.run("add_auto_zooms", &json!({})).unwrap();
        assert_eq!(w.project.scenes[0].zooms.len(), 2);
        assert_eq!(w.project.scenes[0].zooms[0].source_start_ms, 1000.0);
    }

    #[test]
    fn activity_is_reported_on_the_timeline() {
        let mut w = ws();
        let o = w.run("get_activity", &json!({})).unwrap();
        assert!(o.result.contains("No mouse or keyboard log"));
        w.activity = Some(Activity { typing: vec![(4000.0, 8000.0, 40)], clicks: vec![1000.0, 9000.0], idle: vec![(0.0, 3000.0)] });
        w.run("set_speed", &json!({ "start_ms": 4000, "end_ms": 8000, "speed": 4 })).unwrap();
        let v: Value = serde_json::from_str(&w.run("get_activity", &json!({})).unwrap().result).unwrap();
        // the typing now plays in 1 s; the click after it moved 3 s earlier
        assert_eq!(v["typing"], json!([[4000.0, 5000.0, 40]]));
        assert_eq!(v["clicks"], json!([1000.0, 6000.0]));
        assert_eq!(v["idle"], json!([[0.0, 3000.0]]));
    }

    #[test]
    fn subtract_drops_slivers() {
        assert_eq!(subtract(0.0, 1000.0, &mut [(150.0, 300.0), (100.0, 200.0)]), vec![(0.0, 100.0), (300.0, 1000.0)]);
        // a piece shorter than a frame is not kept
        assert_eq!(subtract(0.0, 1000.0, &mut [(5.0, 1000.0)]), vec![]);
    }

    #[test]
    fn camera_and_output_settings_as_documented() {
        // the neutral grade the guide gives (`key value`), warmed up, is a valid sfColorGrade
        let neutral = crate::host::SETTINGS.split("Neutral: ").nth(1).and_then(|r| r.split(".\n").next()).unwrap();
        let (mut quoted, mut word) = (String::new(), String::new());
        for c in neutral.chars() {
            if c.is_ascii_alphabetic() {
                word.push(c);
                continue;
            }
            if !word.is_empty() {
                quoted.push_str(&format!("\"{}\":", std::mem::take(&mut word)));
            }
            quoted.push(c);
        }
        let mut grade: Value = serde_json::from_str(&quoted).unwrap();
        grade["temperature"] = json!(30);
        let mut w = ws();
        // the output aspect ratio is a number, as the guide says, not "9:16"
        w.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/config/output/aspectRatio", "value": 0.5625 }] })).unwrap();
        assert_eq!(w.project.config.output.aspect_ratio.as_f64(), Some(0.5625));
        assert!(w.run("patch_project", &json!({ "ops": [{ "op": "replace", "path": "/config/output/aspectRatio", "value": "9:16" }] })).is_err());
        w.run("patch_project", &json!({ "ops": [
            { "op": "add", "path": "/config/camera/sfColorGrade", "value": grade },
            { "op": "add", "path": "/config/camera/sfFaceTrackingZoom", "value": 1.4 }
        ] })).unwrap();
        let cam = &w.project.config.camera;
        assert_eq!(cam.grade().temperature, 30.0);
        assert!(!cam.grade().is_neutral());
        assert_eq!(cam.face_tracking_zoom(), 1.4);
        w.run("patch_project", &json!({ "ops": [{ "op": "remove", "path": "/config/camera/sfColorGrade" }] })).unwrap();
        assert!(w.project.config.camera.color_grade.is_none());
    }

    #[test]
    fn undo_answers_goes_back_to_before_them() {
        let mut w = ws();
        // three earlier answers: cut 1 s, cut another, sped up; each saved the project before it
        let start = w.project.clone();
        w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 1000 }] })).unwrap();
        let after_one = w.project.clone();
        w.run("cut", &json!({ "ranges": [{ "start_ms": 0, "end_ms": 1000 }] })).unwrap();
        let after_two = w.project.clone();
        w.run("set_speed", &json!({ "start_ms": 0, "end_ms": 8000, "speed": 2 })).unwrap();
        w.earlier = vec![start.clone(), after_one.clone(), after_two.clone()];
        let v: Value = serde_json::from_str(&w.run("get_project", &json!({})).unwrap().result).unwrap();
        assert_eq!(v["answersToUndo"], 3);

        let o = w.run("undo_answers", &json!({})).unwrap();
        assert!(o.changed && w.project == after_two && duration(&w) == 8000.0);
        // the rest in one go
        w.run("undo_answers", &json!({ "all": true })).unwrap();
        assert!(w.project == start && duration(&w) == 10_000.0);
        assert!(w.run("undo_answers", &json!({})).is_err(), "nothing left");
        // more than there are: as far back as it goes
        w.earlier = vec![start.clone(), after_one];
        w.run("undo_answers", &json!({ "count": 5 })).unwrap();
        assert!(w.project == start && w.earlier.is_empty());
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(3240.0), "0:03.2");
        assert_eq!(clock(65_000.0), "1:05.0");
    }
}
