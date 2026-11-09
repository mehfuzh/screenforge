//! Tools for chapters, the list a YouTube description shows (`00:00 Intro`). Times are
//! output ms, as in [`crate::tools`]; each chapter is stored at its moment of the recording (or
//! its title card), so it follows later cuts.

use serde_json::{json, Value};
use sf_engine::chapters::{self as ch, Placed};
use sf_project::{Chapter, SceneChaptersExt};

use crate::tools::{clock, num, opt_num, Outcome, Workspace};

pub fn definitions() -> Vec<Value> {
    let chapter = json!({
        "type": "object",
        "properties": {
            "start_ms": { "type": "number", "description": "Where the chapter starts on the timeline, ms" },
            "title": { "type": "string", "description": "Short title, as it will read on YouTube" }
        },
        "required": ["start_ms", "title"]
    });
    vec![
        json!({
            "name": "get_chapters",
            "description": "The chapters (id, title, start_ms on the timeline) and the chapter list for a YouTube description, with what YouTube would object to (first chapter at 0:00, at least 3, each at least 10 s).",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "set_chapters",
            "description": "Replace all the chapters with this list (an empty list removes them). Use it to draft chapters for the whole video.",
            "input_schema": {
                "type": "object",
                "properties": { "chapters": { "type": "array", "items": chapter } },
                "required": ["chapters"]
            }
        }),
        json!({
            "name": "add_chapter",
            "description": "Add one chapter.",
            "input_schema": chapter
        }),
        json!({
            "name": "update_chapter",
            "description": "Rename a chapter or move its start.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "start_ms": { "type": "number" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "remove_chapter",
            "description": "Remove a chapter.",
            "input_schema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        }),
    ]
}

fn title(input: &Value) -> Result<String, String> {
    input.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(String::from).ok_or_else(|| "`title` is required".to_string())
}

impl Workspace {
    pub(crate) fn run_chapters(&mut self, name: &str, input: &Value) -> Option<Result<Outcome, String>> {
        Some(match name {
            "get_chapters" => Ok(self.get_chapters()),
            "set_chapters" => self.set_chapters(input),
            "add_chapter" => self.add_chapter(input),
            "update_chapter" => self.update_chapter(input),
            "remove_chapter" => self.remove_chapter(input),
            _ => return None,
        })
    }

    fn placed_chapters(&self) -> Vec<Placed> {
        ch::placed(&self.project.scenes[0], &self.timeline())
    }

    fn get_chapters(&self) -> Outcome {
        let tl = self.timeline();
        let placed = self.placed_chapters();
        let cut = self.project.scenes[0].chapters().len() - placed.len();
        let yt = ch::youtube(&placed, tl.duration_ms());
        let list: Vec<Value> = placed.iter().map(|c| json!({ "id": c.id, "title": c.title, "start_ms": c.start_ms.round() })).collect();
        let mut out = json!({ "chapters": list, "youtube": yt.text, "warnings": yt.warnings });
        if cut > 0 {
            out["cut_chapters"] = json!(cut);
        }
        Outcome::read(out, format!("Read {} chapters", placed.len()))
    }

    /// A new chapter starting at output time `start_ms`.
    fn chapter_at(&self, title: String, start_ms: f64) -> Result<Chapter, String> {
        let tl = self.timeline();
        let duration = tl.duration_ms();
        if !(0.0..duration).contains(&start_ms) {
            return Err(format!("start_ms must be on the timeline, 0 to {}", duration.round()));
        }
        let (source_ms, card) = ch::anchor_at(&self.project.scenes[0], &tl, start_ms);
        Ok(Chapter::new(title, source_ms, card))
    }

    /// Store `chapters`, then report where they landed and what YouTube would say.
    fn store_chapters(&mut self, chapters: Vec<Chapter>, summary: String) -> Outcome {
        self.project.scenes[0].set_chapters(chapters);
        let placed = self.placed_chapters();
        let yt = ch::youtube(&placed, self.timeline().duration_ms());
        let list: Vec<Value> = placed.iter().map(|c| json!({ "id": c.id, "title": c.title, "start_ms": c.start_ms.round() })).collect();
        Outcome::edit(json!({ "chapters": list, "warnings": yt.warnings }), summary)
    }

    fn set_chapters(&mut self, input: &Value) -> Result<Outcome, String> {
        let items = input.get("chapters").and_then(Value::as_array).ok_or("`chapters` must be a list")?;
        let list = items.iter().map(|c| self.chapter_at(title(c)?, num(c, "start_ms")?)).collect::<Result<Vec<_>, _>>()?;
        let n = list.len();
        Ok(self.store_chapters(list, if n == 0 { "Removed the chapters".into() } else { format!("Set {n} chapters") }))
    }

    fn add_chapter(&mut self, input: &Value) -> Result<Outcome, String> {
        let start = num(input, "start_ms")?;
        let c = self.chapter_at(title(input)?, start)?;
        let summary = format!("Chapter \u{201c}{}\u{201d} at {}", c.title, clock(start));
        let mut list = self.project.scenes[0].chapters();
        list.push(c);
        Ok(self.store_chapters(list, summary))
    }

    fn update_chapter(&mut self, input: &Value) -> Result<Outcome, String> {
        let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
        let mut list = self.project.scenes[0].chapters();
        let i = list.iter().position(|c| c.id == id).ok_or_else(|| format!("no chapter {id}"))?;
        if input.get("title").is_some() {
            list[i].title = title(input)?;
        }
        if let Some(start) = opt_num(input, "start_ms")? {
            let moved = self.chapter_at(list[i].title.clone(), start)?;
            list[i].source_ms = moved.source_ms;
            list[i].card = moved.card;
        }
        let summary = format!("Updated chapter \u{201c}{}\u{201d}", list[i].title);
        Ok(self.store_chapters(list, summary))
    }

    fn remove_chapter(&mut self, input: &Value) -> Result<Outcome, String> {
        let id = input.get("id").and_then(Value::as_str).ok_or("`id` is required")?;
        let mut list = self.project.scenes[0].chapters();
        let before = list.len();
        list.retain(|c| c.id != id);
        if list.len() == before {
            return Err(format!("no chapter {id}"));
        }
        Ok(self.store_chapters(list, "Removed a chapter".into()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sf_project::v4::Slice;
    use sf_project::Project;

    use crate::tools::Workspace;

    fn workspace() -> Workspace {
        let mut p = Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 120_000.0)];
        Workspace::new(p)
    }

    #[test]
    fn chapters_are_set_listed_for_youtube_and_follow_a_cut() {
        let mut ws = workspace();
        let set = ws.run("set_chapters", &json!({ "chapters": [{ "start_ms": 0, "title": "Intro" }, { "start_ms": 30_000, "title": "Setup" }, { "start_ms": 90_000, "title": "Wrap-up" }] })).unwrap();
        assert!(set.changed);
        let got: serde_json::Value = serde_json::from_str(&ws.run("get_chapters", &json!({})).unwrap().result).unwrap();
        assert_eq!(got["youtube"], "00:00 Intro\n00:30 Setup\n01:30 Wrap-up");
        assert_eq!(got["warnings"], json!([]));

        ws.run("cut", &json!({ "ranges": [{ "start_ms": 10_000, "end_ms": 20_000 }] })).unwrap();
        let got: serde_json::Value = serde_json::from_str(&ws.run("get_chapters", &json!({})).unwrap().result).unwrap();
        assert_eq!(got["youtube"], "00:00 Intro\n00:20 Setup\n01:20 Wrap-up");

        let id = got["chapters"][1]["id"].as_str().unwrap().to_string();
        ws.run("update_chapter", &json!({ "id": id, "title": "Getting set up", "start_ms": 25_000 })).unwrap();
        ws.run("remove_chapter", &json!({ "id": got["chapters"][2]["id"] })).unwrap();
        let got: serde_json::Value = serde_json::from_str(&ws.run("get_chapters", &json!({})).unwrap().result).unwrap();
        assert_eq!(got["youtube"], "00:00 Intro\n00:25 Getting set up");
        assert!(got["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("at least 3")));

        assert!(ws.run("add_chapter", &json!({ "start_ms": 999_999, "title": "Past the end" })).is_err());
        assert!(ws.run("add_chapter", &json!({ "start_ms": 1000, "title": "  " })).is_err());
    }
}
