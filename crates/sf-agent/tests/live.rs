//! A real turn against the Claude API (costs a few cents). Needs `ANTHROPIC_API_KEY` in the
//! environment or the repository's `.env`:
//! `cargo test -p sf-agent --test live -- --ignored --nocapture`
//! (`SF_AGENT_MODEL` picks another model; Claude Haiku 5.5 by default.)

use std::sync::atomic::AtomicBool;

use sf_agent::{Conversation, Event, Route, Settings, Workspace};
use sf_project::v4::Slice;
use sf_project::Project;

#[test]
#[ignore = "calls the Claude API"]
fn cuts_and_zooms_from_plain_words() {
    let mut p = Project::new("Live");
    p.scenes[0].slices = vec![Slice::new(0.0, 30_000.0)];
    let mut ws = Workspace::new(p);
    let route = Route::Direct { api_key: sf_agent::client::local_key().expect("ANTHROPIC_API_KEY in the environment or .env") };
    let settings = Settings { model: std::env::var("SF_AGENT_MODEL").unwrap_or_else(|_| "claude-haiku-5-5".into()) };
    let mut errors = vec![];
    let conv = sf_agent::run_turn(&route, &settings, Conversation::default(), &mut ws, "Cut the first 5 seconds, then add a 2x zoom from 0:10 to 0:15 of the result.", "Playhead at 0:00.0. Selected: nothing.", &AtomicBool::new(false), |e| {
        println!("{e:?}");
        if let Event::Error(m) = e {
            errors.push(m);
        }
    });
    assert!(errors.is_empty(), "{errors:?}");
    let scene = &ws.project.scenes[0];
    assert!((scene.output_duration_ms() - 25_000.0).abs() < 50.0, "duration {}", scene.output_duration_ms());
    assert_eq!(scene.zooms.len(), 1);
    // timeline 0:10 is source 0:15 after the cut
    assert!((scene.zooms[0].source_start_ms - 15_000.0).abs() < 100.0, "{:?}", scene.zooms[0]);
    assert!(conv.messages.len() >= 4);
}
