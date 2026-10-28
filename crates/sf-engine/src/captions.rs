//! Captions: transcript words grouped into lines, what is visible at a source time (line by
//! line or word by word, with the appear / fade-in / slide-in entrances), and SRT export
//! that follows the edited timeline.
//!
//! All times here are source time (ms into the recording session); callers shift the
//! transcript from microphone-file time first.

use sf_project::transcript::TranscriptItem;

use crate::timeline::Timeline;

/// A line is closed before it would exceed this many characters.
pub const MAX_LINE_CHARS: usize = 42;
/// Line limits for the highlight style: a few big words at a time.
pub const SHORT_LINE_CHARS: usize = 24;
pub const SHORT_LINE_WORDS: usize = 4;
/// A pause longer than this starts a new line.
pub const LINE_GAP_MS: f64 = 1200.0;
/// How long a line stays up after its last word (unless the next line starts earlier).
pub const HOLD_MS: f64 = 700.0;
/// Duration of the fade-in / slide-in entrance.
pub const ENTRANCE_MS: f64 = 160.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptionLine {
    pub words: Vec<Word>,
    /// Visible from the first word's start until `end_ms`.
    pub start_ms: f64,
    pub end_ms: f64,
}

impl CaptionLine {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    LineByLine,
    WordByWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrance {
    Appear,
    FadeIn,
    SlideIn,
}

/// How the caption box grows while words are revealed one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    /// The box has the full line's size from the start.
    EntireLine,
    /// The box starts where the full line would start and grows to the right.
    LeftAnchored,
    /// The box wraps the revealed words and stays centred.
    Centered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptionStyle {
    pub reveal: Reveal,
    pub entrance: Entrance,
    pub growth: Growth,
}

impl CaptionStyle {
    /// From the `captions` config strings (`wordsReveal`, `wordEntrance`, `lineGrowth`).
    pub fn from_config(words_reveal: &str, word_entrance: &str, line_growth: &str) -> Self {
        CaptionStyle {
            reveal: if words_reveal == "word-by-word" { Reveal::WordByWord } else { Reveal::LineByLine },
            entrance: match word_entrance {
                "fade-in" | "fade" => Entrance::FadeIn,
                "slide-in" | "slide" => Entrance::SlideIn,
                _ => Entrance::Appear,
            },
            growth: match line_growth {
                "entire-line" => Growth::EntireLine,
                "centered" => Growth::Centered,
                _ => Growth::LeftAnchored,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleWord {
    pub text: String,
    /// Shown at all yet (word by word); unrevealed words keep their space only with `EntireLine`.
    pub revealed: bool,
    pub alpha: f32,
    /// Slide-in progress: 0 = settled, 1 = fully lowered.
    pub drop01: f32,
    /// The word being spoken: the last one of the line that has started.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptionFrame {
    /// Every word of the current line, in order.
    pub words: Vec<VisibleWord>,
    pub growth: Growth,
}

/// Transcript items (microphone time) → words in source time. Sound labels are kept only
/// when `sound_labels` is on; empty items are dropped.
pub fn words_from_transcript(items: &[TranscriptItem], offset_ms: f64, sound_labels: bool) -> Vec<Word> {
    let mut words: Vec<Word> = items
        .iter()
        .filter(|i| sound_labels || !i.is_sound_label())
        .filter_map(|i| {
            let text = i.text.trim();
            (!text.is_empty()).then(|| Word { start_ms: i.start_ms + offset_ms, end_ms: i.end_ms.max(i.start_ms) + offset_ms, text: text.to_string() })
        })
        .collect();
    words.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
    words
}

fn ends_sentence(text: &str) -> bool {
    text.ends_with(['.', '?', '!'])
}

/// Group words into caption lines: a line ends at a sentence end, a long pause or the
/// character limit.
pub fn build_lines(words: &[Word]) -> Vec<CaptionLine> {
    build_lines_with(words, MAX_LINE_CHARS, usize::MAX)
}

/// Short lines for the highlight style ([`SHORT_LINE_CHARS`], [`SHORT_LINE_WORDS`]).
pub fn build_short_lines(words: &[Word]) -> Vec<CaptionLine> {
    build_lines_with(words, SHORT_LINE_CHARS, SHORT_LINE_WORDS)
}

fn build_lines_with(words: &[Word], max_chars: usize, max_words: usize) -> Vec<CaptionLine> {
    let mut lines: Vec<CaptionLine> = vec![];
    let mut cur: Vec<Word> = vec![];
    let mut chars = 0usize;
    for w in words {
        let len = w.text.chars().count();
        if let Some(last) = cur.last() {
            let gap = w.start_ms - last.end_ms;
            if chars + 1 + len > max_chars || cur.len() >= max_words || gap > LINE_GAP_MS || ends_sentence(&last.text) {
                lines.push(CaptionLine { start_ms: cur[0].start_ms, end_ms: last.end_ms, words: std::mem::take(&mut cur) });
                chars = 0;
            }
        }
        chars += if cur.is_empty() { len } else { len + 1 };
        cur.push(w.clone());
    }
    if let Some(last) = cur.last() {
        lines.push(CaptionLine { start_ms: cur[0].start_ms, end_ms: last.end_ms, words: cur.clone() });
    }
    // hold each line a little, without overlapping the next one
    for i in 0..lines.len() {
        let next = lines.get(i + 1).map(|l| l.start_ms).unwrap_or(f64::INFINITY);
        lines[i].end_ms = (lines[i].end_ms + HOLD_MS).min(next);
    }
    lines
}

fn entrance(e: Entrance, since_ms: f64) -> (f32, f32) {
    let k = (since_ms / ENTRANCE_MS).clamp(0.0, 1.0) as f32;
    let ease = 1.0 - (1.0 - k) * (1.0 - k);
    match e {
        Entrance::Appear => (1.0, 0.0),
        Entrance::FadeIn => (ease, 0.0),
        Entrance::SlideIn => (ease, 1.0 - ease),
    }
}

/// What to draw at source time `t`, if a line is up.
pub fn frame_at(lines: &[CaptionLine], t: f64, style: CaptionStyle) -> Option<CaptionFrame> {
    let i = lines.partition_point(|l| l.start_ms <= t).checked_sub(1)?;
    let line = &lines[i];
    if t >= line.end_ms {
        return None;
    }
    let active = line.words.partition_point(|w| w.start_ms <= t).checked_sub(1);
    let words = line
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let since = match style.reveal {
                Reveal::LineByLine => t - line.start_ms,
                Reveal::WordByWord => t - w.start_ms,
            };
            let revealed = since >= 0.0;
            let (alpha, drop01) = if revealed { entrance(style.entrance, since) } else { (0.0, 0.0) };
            VisibleWord { text: w.text.clone(), revealed, alpha, drop01, active: active == Some(i) }
        })
        .collect();
    Some(CaptionFrame { words, growth: style.growth })
}

fn srt_time(ms: f64) -> String {
    let ms = ms.max(0.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Caption cues for the edited video `(start, end, words)` in output time: words in cut
/// ranges are dropped, times follow the slices (speed changes included), and a line split by
/// a cut becomes two cues.
fn cues<'a>(lines: &'a [CaptionLine], timeline: &Timeline) -> Vec<(f64, f64, Vec<&'a str>)> {
    let mut cues: Vec<(f64, f64, Vec<&str>)> = vec![];
    for line in lines {
        let mut cue: Option<(f64, f64, Vec<&str>, usize)> = None;
        for w in &line.words {
            let Some(start) = timeline.source_to_output(w.start_ms) else { continue };
            let end = timeline.source_to_output(w.end_ms.min(line.end_ms)).unwrap_or(start);
            let span = timeline.spans().iter().position(|s| s.contains_source(w.start_ms)).unwrap_or(0);
            match &mut cue {
                Some(c) if c.3 == span => {
                    c.1 = c.1.max(end);
                    c.2.push(&w.text);
                }
                _ => {
                    if let Some(c) = cue.take() {
                        cues.push((c.0, c.1, c.2));
                    }
                    cue = Some((start, end.max(start), vec![&w.text], span));
                }
            }
        }
        if let Some(c) = cue {
            // keep the hold after the last word when it is still inside the same slice
            let hold_end = timeline.source_to_output(line.end_ms - 1.0).filter(|e| *e >= c.1).unwrap_or(c.1);
            cues.push((c.0, hold_end, c.2));
        }
    }
    for c in &mut cues {
        if c.1 <= c.0 {
            c.1 = c.0 + 1.0;
        }
    }
    cues
}

/// SubRip (.srt) text for the edited video (see [`cues`]).
pub fn to_srt(lines: &[CaptionLine], timeline: &Timeline) -> String {
    let mut out = String::new();
    for (n, (start, end, words)) in cues(lines, timeline).iter().enumerate() {
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", n + 1, srt_time(*start), srt_time(*end), words.join(" ")));
    }
    out
}

/// WebVTT (.vtt) text for the edited video: the same cues as [`to_srt`].
pub fn to_vtt(lines: &[CaptionLine], timeline: &Timeline) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (start, end, words) in cues(lines, timeline) {
        // VTT separates the milliseconds with a dot
        out.push_str(&format!("{} --> {}\n{}\n\n", srt_time(start).replace(',', "."), srt_time(end).replace(',', "."), words.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::v4::Slice;

    fn words(spec: &[(f64, f64, &str)]) -> Vec<Word> {
        spec.iter().map(|(s, e, t)| Word { start_ms: *s, end_ms: *e, text: t.to_string() }).collect()
    }

    const STYLE: CaptionStyle = CaptionStyle { reveal: Reveal::LineByLine, entrance: Entrance::Appear, growth: Growth::LeftAnchored };

    #[test]
    fn lines_break_on_sentences_pauses_and_length() {
        let w = words(&[(0.0, 300.0, "Hello"), (300.0, 600.0, "there."), (700.0, 900.0, "Next"), (3000.0, 3200.0, "after"), (3200.0, 3500.0, "pause")]);
        let lines = build_lines(&w);
        let texts: Vec<String> = lines.iter().map(|l| l.text()).collect();
        assert_eq!(texts, ["Hello there.", "Next", "after pause"]);
        assert_eq!(lines[0].end_ms, 700.0, "held until the next line starts");
        assert_eq!(lines[1].end_ms, 900.0 + HOLD_MS);

        let long: Vec<Word> = (0..20).map(|i| Word { start_ms: i as f64 * 200.0, end_ms: i as f64 * 200.0 + 150.0, text: "word".into() }).collect();
        for l in build_lines(&long) {
            assert!(l.text().chars().count() <= MAX_LINE_CHARS, "{}", l.text());
        }
    }

    #[test]
    fn short_lines_keep_a_few_words() {
        let w: Vec<Word> = ["This", "is", "the", "strangest", "laptop", "ever"].iter().enumerate().map(|(i, t)| Word { start_ms: i as f64 * 300.0, end_ms: i as f64 * 300.0 + 250.0, text: t.to_string() }).collect();
        let texts: Vec<String> = build_short_lines(&w).iter().map(|l| l.text()).collect();
        assert_eq!(texts, ["This is the strangest", "laptop ever"]);
    }

    #[test]
    fn the_spoken_word_is_active_until_the_next_one() {
        let lines = build_lines(&words(&[(1000.0, 1200.0, "one"), (1500.0, 1800.0, "two")]));
        let active = |t: f64| frame_at(&lines, t, STYLE).unwrap().words.iter().map(|w| w.active).collect::<Vec<_>>();
        assert_eq!(active(1100.0), [true, false]);
        assert_eq!(active(1300.0), [true, false], "held through the pause");
        assert_eq!(active(1600.0), [false, true]);
        assert_eq!(active(1900.0), [false, true]);
    }

    #[test]
    fn sound_labels_are_optional() {
        let mut label = TranscriptItem::new(1, 500.0, 900.0, "[music]");
        label.kind = Some("sound-label".into());
        let items = vec![TranscriptItem::new(0, 0.0, 400.0, " Hi "), label, TranscriptItem::new(2, 950.0, 1000.0, "  ")];
        let w = words_from_transcript(&items, 1000.0, false);
        assert_eq!(w, words(&[(1000.0, 1400.0, "Hi")]));
        assert_eq!(words_from_transcript(&items, 0.0, true).len(), 2);
    }

    #[test]
    fn line_by_line_shows_the_whole_line() {
        let lines = build_lines(&words(&[(1000.0, 1300.0, "one"), (1300.0, 1600.0, "two")]));
        assert!(frame_at(&lines, 999.0, STYLE).is_none());
        let f = frame_at(&lines, 1050.0, STYLE).unwrap();
        assert!(f.words.iter().all(|w| w.revealed && w.alpha == 1.0));
        assert!(frame_at(&lines, 1600.0 + HOLD_MS + 1.0, STYLE).is_none());
    }

    #[test]
    fn word_by_word_reveals_and_fades_in() {
        let lines = build_lines(&words(&[(1000.0, 1300.0, "one"), (1300.0, 1600.0, "two")]));
        let style = CaptionStyle { reveal: Reveal::WordByWord, entrance: Entrance::FadeIn, growth: Growth::Centered };
        let f = frame_at(&lines, 1100.0, style).unwrap();
        assert!(f.words[0].revealed && !f.words[1].revealed);
        assert_eq!(f.words[1].alpha, 0.0);
        let mid = frame_at(&lines, 1300.0 + ENTRANCE_MS / 2.0, style).unwrap();
        assert!(mid.words[1].alpha > 0.3 && mid.words[1].alpha < 1.0, "{}", mid.words[1].alpha);
        let slide = CaptionStyle { entrance: Entrance::SlideIn, ..style };
        assert!(frame_at(&lines, 1310.0, slide).unwrap().words[1].drop01 > 0.5);
        assert_eq!(frame_at(&lines, 1290.0 + ENTRANCE_MS + 20.0, slide).unwrap().words[1].drop01, 0.0);
    }

    #[test]
    fn vtt_has_the_srt_cues() {
        // keep 0–1 s, cut 1–2 s, keep 2–4 s
        let w = words(&[(500.0, 900.0, "kept"), (1500.0, 1900.0, "cut"), (2100.0, 2500.0, "after")]);
        let tl = Timeline::from_slices(&[Slice::new(0.0, 1000.0), Slice::new(2000.0, 4000.0)]);
        let lines = build_lines(&w);
        let vtt = to_vtt(&lines, &tl);
        assert!(vtt.starts_with("WEBVTT\n\n00:00:00.500 --> "), "{vtt}");
        assert!(!vtt.contains("cut") && vtt.contains("after"), "{vtt}");
        assert_eq!(vtt.matches(" --> ").count(), to_srt(&lines, &tl).matches(" --> ").count());
    }

    #[test]
    fn srt_follows_cuts_and_speed() {
        // keep 0–2 s, cut 2–3 s, play 3–5 s at 2×
        let mut fast = Slice::new(3000.0, 5000.0);
        fast.time_scale = 0.5;
        let tl = Timeline::from_slices(&[Slice::new(0.0, 2000.0), fast]);
        let w = words(&[(500.0, 900.0, "kept"), (2200.0, 2600.0, "cut"), (3200.0, 3600.0, "fast"), (3600.0, 4000.0, "words.")]);
        let srt = to_srt(&build_lines(&w), &tl);
        let cues: Vec<&str> = srt.split("\n\n").filter(|c| !c.is_empty()).collect();
        assert_eq!(cues.len(), 2, "{srt}");
        assert!(cues[0].starts_with("1\n00:00:00,500 --> "), "{srt}");
        assert!(cues[0].ends_with("\nkept"), "{srt}");
        // 3.2 s source → 2 s + 0.2 s / 2 = 2.1 s output
        assert!(cues[1].starts_with("2\n00:00:02,100 --> 00:00:02,"), "{srt}");
        assert!(cues[1].ends_with("\nfast words."), "{srt}");
        assert!(!srt.contains("cut"));
    }
}
