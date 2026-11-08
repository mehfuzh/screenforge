//! Chapters on the edited timeline, and the chapter list YouTube reads from a video's
//! description (`0:00 Intro` lines).

use sf_project::chapters::{Chapter, SceneChaptersExt};
use sf_project::v4::Scene;

use crate::timeline::Timeline;

/// YouTube's rules for description chapters: at least this many, each at least this long.
pub const YOUTUBE_MIN_CHAPTERS: usize = 3;
pub const YOUTUBE_MIN_MS: f64 = 10_000.0;

/// A chapter at its place on the edited timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub id: String,
    pub title: String,
    pub start_ms: f64,
}

/// Start of `chapter` on the edited timeline: at its title card, or where its moment of the
/// recording plays. `None` while that moment is cut (or the card is gone).
pub fn start(chapter: &Chapter, scene: &Scene, timeline: &Timeline) -> Option<f64> {
    if let Some(card) = &chapter.card {
        if let Some(i) = scene.slices.iter().position(|s| &s.id == card && s.is_card()) {
            return timeline.spans().get(i).map(|s| s.output_start_ms);
        }
    }
    timeline.source_to_output(chapter.source_ms)
}

/// What to store for a chapter starting at output time `t`: the recording time there, and
/// the title card when `t` is on one.
pub fn anchor_at(scene: &Scene, timeline: &Timeline, t: f64) -> (f64, Option<String>) {
    match timeline.span_at_output(t) {
        Some(span) if span.card => (span.source_start_ms, scene.slices.get(span.index).map(|s| s.id.clone())),
        _ => (timeline.output_to_source(t).unwrap_or(0.0), None),
    }
}

/// The scene's chapters that are not cut, in timeline order (on a tie, the one listed first).
pub fn placed(scene: &Scene, timeline: &Timeline) -> Vec<Placed> {
    let mut list: Vec<Placed> = scene.chapters().iter().filter_map(|c| start(c, scene, timeline).map(|s| Placed { id: c.id.clone(), title: c.title.clone(), start_ms: s })).collect();
    list.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
    list
}

/// `m:ss`, or `h:mm:ss` when the video is an hour or longer (whole seconds, rounded down, as
/// YouTube links to them).
pub fn timestamp(ms: f64, video_ms: f64) -> String {
    let s = (ms.max(0.0) / 1000.0).floor() as u64;
    if video_ms >= 3_600_000.0 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// The chapter list for a YouTube description, and what YouTube would object to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct YouTube {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Lines of `00:00 Title` for `chapters` (from [`placed`]) of a video `video_ms` long. YouTube
/// needs the first chapter at 0:00, so the first is always listed there.
pub fn youtube(chapters: &[Placed], video_ms: f64) -> YouTube {
    let mut out = YouTube::default();
    if chapters.is_empty() {
        out.warnings.push("There are no chapters yet.".into());
        return out;
    }
    if chapters[0].start_ms >= 1000.0 {
        out.warnings.push(format!("YouTube starts the first chapter at 0:00, so \u{201c}{}\u{201d} is listed there instead of {}.", chapters[0].title, timestamp(chapters[0].start_ms, video_ms)));
    }
    if chapters.len() < YOUTUBE_MIN_CHAPTERS {
        out.warnings.push(format!("YouTube shows chapters only when there are at least {YOUTUBE_MIN_CHAPTERS}."));
    }
    let lines: Vec<String> = chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let at = if i == 0 { 0.0 } else { c.start_ms };
            let end = chapters.get(i + 1).map_or(video_ms, |n| n.start_ms);
            if end - at < YOUTUBE_MIN_MS {
                out.warnings.push(format!("\u{201c}{}\u{201d} is shorter than 10 s; YouTube needs every chapter to be at least that long.", c.title));
            }
            let title = c.title.trim();
            format!("{} {}", timestamp(at, video_ms), if title.is_empty() { "Untitled" } else { title })
        })
        .collect();
    out.text = lines.join("\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::v4::Slice;
    use sf_project::Project;

    fn scene() -> Scene {
        let mut p = Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 60_000.0)];
        p.scenes.remove(0)
    }

    fn placed_at(starts: &[(f64, &str)]) -> Vec<Placed> {
        starts.iter().map(|(s, t)| Placed { id: t.to_string(), title: t.to_string(), start_ms: *s }).collect()
    }

    #[test]
    fn chapters_follow_cuts_and_vanish_while_cut() {
        let mut s = scene();
        s.set_chapters(vec![Chapter::new("Late", 40_000.0, None), Chapter::new("Gone", 15_000.0, None), Chapter::new("Start", 0.0, None)]);
        // cut 10–20 s
        s.slices = vec![Slice::new(0.0, 10_000.0), Slice::new(20_000.0, 60_000.0)];
        let tl = Timeline::from_scene(&s);
        let list = placed(&s, &tl);
        assert_eq!(list.iter().map(|c| (c.title.as_str(), c.start_ms)).collect::<Vec<_>>(), vec![("Start", 0.0), ("Late", 30_000.0)]);
    }

    #[test]
    fn a_chapter_on_a_title_card_stays_on_it() {
        let mut s = scene();
        crate::tracks::add_title_card(&mut s, 0, "Welcome", 3000.0);
        let tl = Timeline::from_scene(&s);
        let (src, card) = anchor_at(&s, &tl, 1000.0);
        assert_eq!(card.as_deref(), Some(s.slices[0].id.as_str()));
        let (src2, none) = anchor_at(&s, &tl, 5000.0);
        assert!(none.is_none());
        assert_eq!(src2, 2000.0);
        s.set_chapters(vec![Chapter::new("Welcome", src, card), Chapter::new("Demo", src2, None)]);
        let list = placed(&s, &tl);
        assert_eq!(list.iter().map(|c| c.start_ms).collect::<Vec<_>>(), vec![0.0, 5000.0]);
    }

    #[test]
    fn youtube_lists_the_first_chapter_at_zero_and_warns_about_its_rules() {
        let y = youtube(&placed_at(&[(0.0, "Intro"), (102_400.0, "Setting up"), (310_000.0, "Auth")]), 400_000.0);
        assert_eq!(y.text, "00:00 Intro\n01:42 Setting up\n05:10 Auth");
        assert!(y.warnings.is_empty());

        let y = youtube(&placed_at(&[(4000.0, "Intro"), (9000.0, "Short")]), 3_700_000.0);
        assert_eq!(y.text, "0:00:00 Intro\n0:00:09 Short");
        assert_eq!(y.warnings.len(), 3, "{:?}", y.warnings);
        assert!(y.warnings[0].contains("0:00"));
        assert!(y.warnings.iter().any(|w| w.contains("at least 3")));
        assert!(y.warnings.iter().any(|w| w.contains("Intro") && w.contains("10 s")));
    }
}
