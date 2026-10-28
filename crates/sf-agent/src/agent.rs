//! One user turn of the editing agent: send the conversation to Claude, run the tools it asks
//! for on the [`Workspace`], send the results back, and repeat until it answers in words.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use sf_project::Project;

use crate::client::{self, Route, Settings, Usage};
use crate::tools::Workspace;

/// Tool rounds per user turn before the agent is stopped (a runaway loop costs money).
pub const MAX_STEPS: usize = 30;

const SYSTEM: &str = "You are the editing assistant inside ScreenForge, a screen recording editor. \
The user asks for changes to the open project in plain words and you make them with your tools; the timeline updates as you work.

How the project works:
- The video is a recording laid out as clips on a timeline. Cutting removes time; speed changes make parts play faster or slower; zooms magnify the screen; settings (background, padding, rounded corners, shadow, cursor, camera, captions, audio, output aspect ratio) live under /config in the project file.
- Tool times are milliseconds on the timeline, exactly what the user sees under the playhead. Each message from the user ends with the playhead position and the current selection, so \"here\" or \"this clip\" can be resolved.
- What was said is in get_transcript (only if the recording was transcribed). Use it to find moments by their words, filler words (um, uh, like), long pauses and retakes.

How to work:
- Look before you edit: call get_project (and get_transcript or get_config when relevant) at the start of every request, since the user may have edited by hand in between.
- Make the edit the user asked for, completely, then stop. Do not change things they did not ask about.
- When a request is ambiguous in a way that changes the result (which of several moments, how much to cut), ask a short question instead of guessing.
- When you remove speech, leave about 100 ms before and after the neighbouring words so the cut does not clip them.
- Zooms, masks and camera layouts each sit on one lane and cannot overlap: to change what happens at a moment, update or remove the item already there.
- Zoom gently: about 1.3–1.5× where the user types or reads text (the camera follows the caret, and a closer zoom cuts lines off), about 2× only for a small button or detail. Keep each zoom to the moment it is for, a few seconds, rather than one long zoom over a whole passage. If the user says a zoom is too close, lower its level.
- You can do anything the user can do by hand: settings (get_options lists the values and where each setting lives), backgrounds including Unsplash photos (search_photos, use_photo), music including Creative Commons tracks from Jamendo (the user's favourites in get_options favoriteMusic first, then search_music; use_music) and audio links (import_file), voice-overs, masks, camera layouts, presets, transcribing, opening the export dialog.
- Titles, overlays and transitions make a product film out of a recording: add_title for a short headline over the video (star the key words for the accent colour, a smaller subtitle under it), add_overlay for an image or video the user gives you (a UI card, a screenshot, b-roll) with backdrop_blur so the recording behind goes out of focus, and set_transition for a crossfade, dip to black or blur at a cut (fade-black on clip 0 fades the video in). Keep titles short and on screen 2–4 s, and keep them pinned (the default) so they move with their moment when footage is cut.
- Find moments by what happened: get_activity (typing, clicks, idle, from the mouse and keyboard log), get_motion (still, small or large changes on screen, from the video itself, for every recording), and view_frames to look at the screen and be sure. A recording without a log or a transcript can still be edited this way.
- To fit a target length (\"keep it to 1 minute\"): read durationMs, then speed up idle and typing parts and cut long pauses before cutting anything that shows content; check the new duration with get_project and adjust until it fits.
- \"Auto edit\" means tighten the whole video: with a transcript, cut long pauses and filler words; speed up stretches where nothing is said or clicked (2–4×); add zooms around the clicks if there are none; then summarise what you did.
- Every turn of yours is undone in one step with Cmd+Z, so the user can try things freely. When the user asks you to undo your changes (\"undo that\", \"undo everything you did\"), use undo_answers rather than reversing the edits yourself.
- Finish with one or two plain sentences saying what changed, with times as m:ss. No markdown, headings or lists.";

/// The messages so far, exactly as sent to the API (responses are appended unchanged).
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub messages: Vec<Value>,
}

impl Conversation {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Add user content. A request that failed or was stopped can leave a user message last
    /// (tool results, or the question itself): the new content joins it, since the API wants
    /// user and assistant to alternate.
    fn push_user(&mut self, mut blocks: Vec<Value>) {
        if let Some(last) = self.messages.last_mut().filter(|m| m["role"] == "user") {
            if let Some(content) = last["content"].as_array_mut() {
                content.append(&mut blocks);
                return;
            }
        }
        self.messages.push(json!({ "role": "user", "content": blocks }));
    }
}

/// Progress of a turn, for the chat panel.
#[derive(Debug, Clone)]
pub enum Event {
    /// Claude wrote something to the user.
    Text(String),
    /// A tool ran: its one-line summary, or why it failed.
    Step { summary: String, failed: bool },
    /// The project after a round of edits.
    Changed(Box<Project>),
    /// Move the playhead here (output ms).
    Seek(f64),
    /// Something only the editor can do (transcribe, open export).
    Action(crate::host::Action),
    /// One request's tokens and cost.
    Usage(Usage),
    /// The ScreenForge account's credit balance after a request (hosted only; see the
    /// backend's `_shared/credits.ts` summary).
    Credits(Value),
    /// The turn ended early; the text is for the user.
    Error(String),
}

/// Run one user turn: `text` is the request, `context` describes the editor (playhead,
/// selection). Returns the conversation to continue from. Stops after the current request
/// when `cancel` is set.
pub fn run_turn(route: &Route, settings: &Settings, conversation: Conversation, workspace: &mut Workspace, text: &str, context: &str, cancel: &AtomicBool, emit: impl FnMut(Event)) -> Conversation {
    let model = settings.pick();
    run_turn_with(|body| client::send(route, model, body), conversation, workspace, text, context, cancel, emit)
}

/// [`run_turn`] with the request function passed in (tests answer without the network).
pub fn run_turn_with(mut send: impl FnMut(Value) -> Result<Value, String>, mut conv: Conversation, ws: &mut Workspace, text: &str, context: &str, cancel: &AtomicBool, mut emit: impl FnMut(Event)) -> Conversation {
    conv.push_user(vec![json!({ "type": "text", "text": text }), json!({ "type": "text", "text": format!("<editor>{context}</editor>") })]);
    for _ in 0..MAX_STEPS {
        if cancel.load(Ordering::Relaxed) {
            emit(Event::Error("Stopped.".into()));
            return conv;
        }
        let body = json!({
            "max_tokens": client::MAX_TOKENS,
            "system": SYSTEM,
            "tools": crate::tools::definitions(),
            "messages": conv.messages,
            "output_config": { "effort": "medium" },
            "cache_control": { "type": "ephemeral" },
        });
        let resp = match send(body) {
            Ok(r) => r,
            Err(e) => {
                emit(Event::Error(e));
                return conv;
            }
        };
        emit(Event::Usage(Usage::from_response(&resp)));
        if resp["sf_credits"].is_object() {
            emit(Event::Credits(resp["sf_credits"].clone()));
        }
        let content = resp["content"].as_array().cloned().unwrap_or_default();
        // appended unchanged (thinking blocks included) so the next request continues it
        conv.messages.push(json!({ "role": "assistant", "content": content }));
        for block in &content {
            if let Some(t) = block["text"].as_str().filter(|_| block["type"] == "text").map(str::trim).filter(|t| !t.is_empty()) {
                emit(Event::Text(t.to_string()));
            }
        }
        let calls: Vec<&Value> = content.iter().filter(|b| b["type"] == "tool_use").collect();
        match resp["stop_reason"].as_str() {
            Some("tool_use") if !calls.is_empty() => {}
            Some("refusal") => {
                emit(Event::Error("Claude declined this request. Try rewording it.".into()));
                return conv;
            }
            Some("max_tokens") => {
                emit(Event::Error("The reply was too long and was cut off. Try a smaller request.".into()));
                close_calls(&mut conv, &calls, "The reply was cut off before this call could run.");
                return conv;
            }
            _ => return conv,
        }
        if cancel.load(Ordering::Relaxed) {
            close_calls(&mut conv, &calls, "Stopped by the user before this call ran.");
            emit(Event::Error("Stopped.".into()));
            return conv;
        }
        let mut results = vec![];
        let mut changed = false;
        for call in calls {
            let (id, name, input) = (call["id"].clone(), call["name"].as_str().unwrap_or(""), &call["input"]);
            match ws.run(name, input) {
                Ok(o) => {
                    changed |= o.changed;
                    emit(Event::Step { summary: o.summary, failed: false });
                    let content = if o.images.is_empty() {
                        Value::from(o.result)
                    } else {
                        use base64::Engine;
                        let mut blocks = vec![json!({ "type": "text", "text": o.result })];
                        blocks.extend(o.images.iter().map(|jpg| json!({ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(jpg) } })));
                        Value::Array(blocks)
                    };
                    results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": content }));
                }
                Err(e) => {
                    emit(Event::Step { summary: format!("{name}: {e}"), failed: true });
                    results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": e, "is_error": true }));
                }
            }
        }
        if changed {
            emit(Event::Changed(Box::new(ws.project.clone())));
        }
        if let Some(t) = ws.seek_to.take() {
            emit(Event::Seek(t));
        }
        for a in std::mem::take(&mut ws.actions) {
            emit(Event::Action(a));
        }
        // every result of one reply goes back in one message
        conv.push_user(results);
    }
    emit(Event::Error(format!("Stopped after {MAX_STEPS} steps. Ask again to continue.")));
    conv
}

/// Answer tool calls that will not run, so the conversation stays valid for the next turn.
fn close_calls(conv: &mut Conversation, calls: &[&Value], why: &str) {
    if !calls.is_empty() {
        conv.push_user(calls.iter().map(|c| json!({ "type": "tool_result", "tool_use_id": c["id"], "content": why, "is_error": true })).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::v4::Slice;

    fn workspace() -> Workspace {
        let mut p = Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 10_000.0)];
        Workspace::new(p)
    }

    fn reply(stop: &str, content: Value) -> Value {
        json!({ "model": "claude-sonnet-5-5", "stop_reason": stop, "content": content, "usage": { "input_tokens": 10, "output_tokens": 5 } })
    }

    #[test]
    fn runs_tools_until_claude_answers() {
        let mut replies = vec![
            reply("tool_use", json!([{ "type": "text", "text": "Cutting the intro." }, { "type": "tool_use", "id": "a", "name": "cut", "input": { "ranges": [{ "start_ms": 0, "end_ms": 2000 }] } }, { "type": "tool_use", "id": "b", "name": "seek", "input": { "at_ms": 0 } }])),
            reply("end_turn", json!([{ "type": "text", "text": "Removed 0:00 – 0:02." }])),
        ]
        .into_iter();
        let mut sent = vec![];
        let mut events = vec![];
        let mut ws = workspace();
        let conv = run_turn_with(
            |body| {
                sent.push(body);
                Ok(replies.next().unwrap())
            },
            Conversation::default(),
            &mut ws,
            "cut the first two seconds",
            "playhead 0:05.0",
            &AtomicBool::new(false),
            |e| events.push(e),
        );
        assert_eq!(ws.project.scenes[0].output_duration_ms(), 8000.0);
        // user, assistant (tools), user (both results in one message), assistant
        assert_eq!(conv.messages.len(), 4);
        let results = conv.messages[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "a");
        assert_eq!(sent[1]["messages"].as_array().unwrap().len(), 3);
        assert!(sent[0]["messages"][0]["content"][1]["text"].as_str().unwrap().contains("playhead 0:05.0"));
        assert!(events.iter().any(|e| matches!(e, Event::Changed(p) if p.scenes[0].slices[0].source_start_ms == 2000.0)));
        assert!(events.iter().any(|e| matches!(e, Event::Seek(t) if *t == 0.0)));
        assert!(matches!(events.last(), Some(Event::Text(t)) if t.starts_with("Removed")));
    }

    #[test]
    fn a_failed_tool_is_reported_back_and_a_failed_request_keeps_the_conversation_valid() {
        let mut n = 0;
        let mut ws = workspace();
        let mut events = vec![];
        let conv = run_turn_with(
            |_| {
                n += 1;
                match n {
                    1 => Ok(reply("tool_use", json!([{ "type": "tool_use", "id": "x", "name": "cut", "input": {} }]))),
                    _ => Err("Claude is overloaded right now.".into()),
                }
            },
            Conversation::default(),
            &mut ws,
            "cut",
            "",
            &AtomicBool::new(false),
            |e| events.push(e),
        );
        assert!(events.iter().any(|e| matches!(e, Event::Step { failed: true, .. })));
        assert!(matches!(events.last(), Some(Event::Error(_))));
        assert_eq!(conv.messages[2]["content"][0]["is_error"], true);
        // the next question joins the pending tool results instead of breaking alternation
        let conv = run_turn_with(|_| Ok(reply("end_turn", json!([{ "type": "text", "text": "ok" }]))), conv, &mut ws, "again", "", &AtomicBool::new(false), |_| {});
        let roles: Vec<&str> = conv.messages.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(conv.messages[2]["content"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn stopping_answers_pending_tool_calls() {
        let cancel = AtomicBool::new(false);
        let mut ws = workspace();
        let conv = run_turn_with(
            |_| {
                cancel.store(true, Ordering::Relaxed);
                Ok(reply("tool_use", json!([{ "type": "tool_use", "id": "z", "name": "cut", "input": { "ranges": [{ "start_ms": 0, "end_ms": 1000 }] } }])))
            },
            Conversation::default(),
            &mut ws,
            "cut",
            "",
            &cancel,
            |_| {},
        );
        assert_eq!(ws.project.scenes[0].output_duration_ms(), 10_000.0, "nothing runs after Stop");
        assert_eq!(conv.messages.last().unwrap()["content"][0]["tool_use_id"], "z");
    }
}
