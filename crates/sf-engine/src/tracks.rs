//! Titles, overlays and transitions in time: where a linked clip sits on the edited timeline,
//! how far its entrance and exit animations are at an output time, and which transition
//! between slices is playing.

use sf_project::tracks::{self as tr, Clip, SceneTracksExt, TextStyle, Track};
use sf_project::v4::{Card, Scene, Slice, Transition};
use sf_project::SceneAudioExt;

use crate::timeline::Timeline;

/// Frames this close before a slice with a transition are kept by the compositor, so the
/// transition can blend the outgoing shot's last frame.
pub const HOLD_LEAD_MS: f64 = 250.0;
/// Longest transition; it is also never longer than its slice.
pub const MAX_TRANSITION_MS: f64 = 2000.0;
/// Of the `words` entrance, the share each word takes to settle (the rest is the stagger).
const WORD_SETTLE: f64 = 0.45;

/// Start of `clip` on the edited timeline: from its link when it has one (`None` while that
/// moment of the recording is cut), else its stored start.
pub fn clip_start(clip: &Clip, timeline: &Timeline) -> Option<f64> {
    match clip.link {
        Some(l) => timeline.source_to_output(l.source_ms),
        None => Some(clip.output_start_ms),
    }
}

/// Refresh the stored start of every linked clip from its link (after the slices changed).
pub fn resolve_links(tracks: &mut [Track], timeline: &Timeline) {
    for c in tracks.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        if let Some(start) = c.link.and_then(|l| timeline.source_to_output(l.source_ms)) {
            c.output_start_ms = start;
        }
    }
}

/// Where a clip is in its entrance and exit at one output time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimState {
    pub opacity: f32,
    /// 0 = sharp, 1 = fully blurred.
    pub blur01: f32,
    /// Offset as fractions of the video size.
    pub dx01: f32,
    pub dy01: f32,
    pub scale: f32,
    /// Progress of a `words` entrance (0..1), `None` otherwise; see [`word_progress`].
    pub words01: Option<f32>,
}

impl AnimState {
    pub const SETTLED: AnimState = AnimState { opacity: 1.0, blur01: 0.0, dx01: 0.0, dy01: 0.0, scale: 1.0, words01: None };
}

fn ease_out(k: f64) -> f64 {
    1.0 - (1.0 - k).powi(3)
}

/// Apply one animation at eased progress `e` (1 = settled); `entering` flips the direction of
/// slides so a clip enters from one side and leaves toward the other.
fn apply(s: &mut AnimState, kind: &str, e: f64, entering: bool) {
    let rest = (1.0 - e) as f32;
    let dir = if entering { 1.0 } else { -1.0 };
    match kind {
        "fade" => s.opacity *= e as f32,
        "blur" | "words" => {
            s.opacity *= e as f32;
            s.blur01 = s.blur01.max(rest);
        }
        "rise" => {
            s.opacity *= e as f32;
            s.dy01 += rest * 0.04 * dir;
        }
        "zoom" => {
            s.opacity *= e as f32;
            s.scale *= 1.0 - rest * 0.08;
        }
        "slide-left" => s.dx01 += rest * 0.3 * dir,
        "slide-right" => s.dx01 -= rest * 0.3 * dir,
        "slide-up" => s.dy01 += rest * 0.3 * dir,
        "slide-down" => s.dy01 -= rest * 0.3 * dir,
        _ => {}
    }
}

/// The animation state of a clip starting at output time `start`, at output time `t`;
/// `None` outside the clip. Each animation takes at most half the clip.
pub fn anim_state(clip: &Clip, start: f64, t: f64) -> Option<AnimState> {
    let dur = clip.output_duration_ms();
    let local = t - start;
    if !(0.0..dur).contains(&local) {
        return None;
    }
    let mut s = AnimState::SETTLED;
    if let Some(a) = &clip.anim_in {
        let d = a.duration_ms.clamp(1.0, dur / 2.0);
        let k = (local / d).clamp(0.0, 1.0);
        if a.kind == "words" {
            // each word blurs in by itself (see word_progress); the block stays put
            s.words01 = Some(k as f32);
        } else {
            apply(&mut s, &a.kind, ease_out(k), true);
        }
    }
    if let Some(a) = &clip.anim_out {
        let d = a.duration_ms.clamp(1.0, dur / 2.0);
        let k = ((dur - local) / d).clamp(0.0, 1.0);
        apply(&mut s, &a.kind, ease_out(k), false);
    }
    Some(s)
}

/// Eased progress (0..1) of word `i` of `n` in a `words` entrance at overall progress
/// `words01`: the words start one after another and each settles in [`WORD_SETTLE`].
pub fn word_progress(words01: f32, i: usize, n: usize) -> f32 {
    let n = n.max(1);
    let start = if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 * (1.0 - WORD_SETTLE) };
    ease_out(((words01 as f64 - start) / WORD_SETTLE).clamp(0.0, 1.0)) as f32
}

/// A transition playing at an output time.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionState {
    /// Index of the incoming slice.
    pub slice: usize,
    pub kind: String,
    /// 0 at the cut, 1 when it has finished.
    pub progress: f32,
}

fn transition_ms(t: &Transition, slice_ms: f64) -> f64 {
    t.duration_ms.clamp(1.0, MAX_TRANSITION_MS).min(slice_ms)
}

/// The transition playing at output time `t`, if any.
pub fn transition_at(slices: &[Slice], timeline: &Timeline, t: f64) -> Option<TransitionState> {
    let span = timeline.span_at_output(t)?;
    let tr = slices.get(span.index)?.transition.as_ref()?;
    let d = transition_ms(tr, span.output_end_ms - span.output_start_ms);
    let k = (t - span.output_start_ms) / d;
    (0.0..1.0).contains(&k).then(|| TransitionState { slice: span.index, kind: tr.kind.clone(), progress: k as f32 })
}

/// The incoming slice whose transition needs the frame at `t` held: `t` is in the last
/// [`HOLD_LEAD_MS`] before a slice (not the first) that has a transition.
pub fn hold_before(slices: &[Slice], timeline: &Timeline, t: f64) -> Option<usize> {
    let span = timeline.span_at_output(t)?;
    let next = span.index + 1;
    let start = timeline.spans().get(next)?.output_start_ms;
    (slices.get(next)?.transition.is_some() && t >= start - HOLD_LEAD_MS && t < start).then_some(next)
}

// ───────────── title cards ─────────────

/// Move what sits at a fixed output time from `at` on by `delta` ms: detached recorded audio
/// and titles or overlays that are not pinned. Pinned clips, voice-overs, zooms and masks
/// follow the recording and need nothing. With a negative `delta` (a card removed or made
/// shorter), what started inside the removed stretch moves to `at`.
pub fn shift_output_items(scene: &mut Scene, at: f64, delta: f64) {
    let moved = |start: f64| if start >= at - 0.5 { (start + delta).max(at.min(start)) } else { start };
    if let Some(mut clips) = scene.audio_clips() {
        clips.iter_mut().for_each(|c| c.output_start_ms = moved(c.output_start_ms));
        scene.set_audio_clips(Some(clips));
    }
    let mut tracks = scene.tracks();
    for c in tracks.iter_mut().flat_map(|t| t.clips.iter_mut()).filter(|c| c.link.is_none()) {
        c.output_start_ms = moved(c.output_start_ms);
    }
    scene.set_tracks(tracks);
}

/// Insert a title card `duration_ms` long before slice `index` (at the end when past it).
/// Returns its output start.
pub fn insert_card(scene: &mut Scene, index: usize, duration_ms: f64) -> f64 {
    let timeline = Timeline::from_scene(scene);
    let index = index.min(scene.slices.len());
    let at = timeline.spans().get(index).map(|s| s.output_start_ms).unwrap_or(timeline.duration_ms());
    // anchored where the next clip starts, or where the last one ends
    let anchor = scene.slices.get(index).map(|s| s.source_start_ms).or_else(|| scene.slices.last().map(|s| s.source_end_ms)).unwrap_or(0.0);
    let card = Slice::card(anchor, duration_ms);
    let len = card.output_duration_ms();
    scene.slices.insert(index, card);
    shift_output_items(scene, at, len);
    at
}

/// Remove the title card at slice `index` and close its gap. False when it is not a card.
pub fn remove_card(scene: &mut Scene, index: usize) -> bool {
    if !scene.slices.get(index).is_some_and(Slice::is_card) {
        return false;
    }
    let span = Timeline::from_scene(scene).spans()[index];
    scene.slices.remove(index);
    shift_output_items(scene, span.output_start_ms, -span.output_duration_ms());
    true
}

/// Make the title card at slice `index` last `ms`, moving what follows with its end.
pub fn set_card_duration(scene: &mut Scene, index: usize, ms: f64) {
    let Some(span) = Timeline::from_scene(scene).spans().get(index).copied().filter(|s| s.card) else { return };
    let ms = ms.max(Card::MIN_MS);
    if let Some(card) = scene.slices[index].card.as_mut() {
        card.duration_ms = ms;
    }
    // from the shorter end: growing pushes what follows, shrinking pulls it (and what sat in
    // the cut-off stretch) back
    let old = span.output_duration_ms();
    shift_output_items(scene, span.output_start_ms + ms.min(old), ms - old);
    // a title made for the card keeps covering it
    let mut tracks = scene.tracks();
    for c in tracks.iter_mut().flat_map(|t| t.clips.iter_mut()).filter(|c| c.link.is_none() && c.text.is_some()) {
        if (c.output_start_ms - span.output_start_ms).abs() < 0.5 && (c.output_end_ms() - span.output_end_ms).abs() < 0.5 {
            c.set_output_duration(ms);
        }
    }
    scene.set_tracks(tracks);
}

/// A title card before slice `index` with `text` centred on it, words blurring in, and a
/// crossfade into the clip after it. Returns the title's clip id.
pub fn add_title_card(scene: &mut Scene, index: usize, text: &str, duration_ms: f64) -> String {
    let at = insert_card(scene, index, duration_ms);
    let index = index.min(scene.slices.len() - 1);
    if let Some(card) = scene.slices[index].card.as_mut() {
        card.background = Some(Card::DEFAULT_FILL.into());
    }
    let len = scene.slices[index].output_duration_ms();
    if let Some(next) = scene.slices.get_mut(index + 1).filter(|s| !s.is_card() && s.transition.is_none()) {
        next.transition = Some(Transition { kind: "crossfade".into(), duration_ms: 600.0 });
    }
    let mut title = Clip::title(at, len, text);
    if let Some(t) = title.text.as_mut() {
        *t = TextStyle { align: "center".into(), position01: sf_project::schema::common::Point::new(0.5, 0.5), size_ratio: 0.08, ..t.clone() };
    }
    let id = title.id.clone();
    let mut tracks = scene.tracks();
    tr::add_clip(&mut tracks, title);
    scene.set_tracks(tracks);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::tracks::{ClipAnim, Link};

    fn tl(slices: &[Slice]) -> Timeline {
        Timeline::from_slices(slices)
    }

    #[test]
    fn linked_clips_follow_cuts_and_hide_inside_them() {
        // keep 0–2 s and 4–6 s: source 5 s plays at output 3 s
        let slices = [Slice::new(0.0, 2000.0), Slice::new(4000.0, 6000.0)];
        let mut c = Clip::title(9999.0, 1000.0, "x");
        c.link = Some(Link { source_ms: 5000.0 });
        assert_eq!(clip_start(&c, &tl(&slices)), Some(3000.0));
        c.link = Some(Link { source_ms: 3000.0 });
        assert_eq!(clip_start(&c, &tl(&slices)), None, "its moment is cut");
        c.link = None;
        assert_eq!(clip_start(&c, &tl(&slices)), Some(9999.0));

        let mut tracks = vec![Track::text()];
        let mut linked = Clip::title(0.0, 1000.0, "y");
        linked.link = Some(Link { source_ms: 4500.0 });
        tracks[0].clips.push(linked);
        resolve_links(&mut tracks, &tl(&slices));
        assert_eq!(tracks[0].clips[0].output_start_ms, 2500.0);
    }

    #[test]
    fn entrances_and_exits() {
        let mut c = Clip::title(0.0, 2000.0, "x");
        c.anim_in = Some(ClipAnim { kind: "rise".into(), duration_ms: 400.0 });
        c.anim_out = Some(ClipAnim { kind: "blur".into(), duration_ms: 400.0 });
        assert!(anim_state(&c, 1000.0, 999.0).is_none());
        assert!(anim_state(&c, 1000.0, 3000.0).is_none());
        let a = anim_state(&c, 1000.0, 1000.0).unwrap();
        assert_eq!(a.opacity, 0.0);
        assert!(a.dy01 > 0.03, "starts lower");
        assert_eq!(anim_state(&c, 1000.0, 2000.0).unwrap(), AnimState::SETTLED);
        let out = anim_state(&c, 1000.0, 2900.0).unwrap();
        assert!(out.blur01 > 0.0 && out.opacity < 1.0);

        // slides enter from one side and leave toward the other
        c.anim_in = Some(ClipAnim::new("slide-left"));
        c.anim_out = Some(ClipAnim::new("slide-left"));
        assert!(anim_state(&c, 0.0, 10.0).unwrap().dx01 > 0.2);
        assert!(anim_state(&c, 0.0, 1990.0).unwrap().dx01 < -0.2);
    }

    #[test]
    fn words_settle_one_after_another() {
        let mut c = Clip::title(0.0, 3000.0, "a b c");
        c.anim_out = None;
        let s = anim_state(&c, 0.0, 100.0).unwrap();
        let w = s.words01.unwrap();
        assert!(word_progress(w, 0, 3) > word_progress(w, 1, 3) && word_progress(w, 1, 3) >= word_progress(w, 2, 3));
        assert_eq!(word_progress(1.0, 2, 3), 1.0);
        assert_eq!(word_progress(0.0, 0, 1), 0.0);
        assert_eq!(s.opacity, 1.0, "the block itself does not fade");
    }

    #[test]
    fn title_cards_play_for_their_length_with_no_recording() {
        let mut scene = Scene::recording("s", 0);
        scene.slices = vec![Slice::new(1000.0, 5000.0)];
        let id = add_title_card(&mut scene, 0, "Hello *world*", 3000.0);
        let t = Timeline::from_scene(&scene);
        assert_eq!(t.duration_ms(), 7000.0);
        let card = t.spans()[0];
        assert!(card.card && card.output_duration_ms() == 3000.0);
        // the card holds the first frame of the clip after it; then the clip plays as before
        assert_eq!(t.output_to_source(1500.0), Some(1000.0));
        assert_eq!(t.output_to_source(3500.0), Some(1500.0));
        assert_eq!(t.source_to_output(1500.0), Some(3500.0));
        assert_eq!(t.view_duration_ms(), 7000.0, "the card is as wide as it lasts");
        assert_eq!(scene.slices[1].transition.as_ref().map(|t| t.kind.as_str()), Some("crossfade"));
        let title = scene.tracks()[0].clips[0].clone();
        assert_eq!((title.id.clone(), title.output_start_ms, title.output_end_ms(), title.link), (id, 0.0, 3000.0, None));
        // no recorded audio under the card
        let spans = crate::audio::audio_spans(&scene, &[sf_project::AudioTrack::Microphone], &sf_project::v4::AudioConfig::default());
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].output_start_ms, spans[0].source_start_ms), (3000.0, 1000.0));
    }

    #[test]
    fn cards_move_what_sits_at_a_fixed_time() {
        let mut scene = Scene::recording("s", 0);
        scene.slices = vec![Slice::new(0.0, 4000.0)];
        let mut audio = sf_project::AudioClip::new(sf_project::AudioTrack::Microphone, 0.0, 0.0, 4000.0, 1.0);
        audio.id = "a".into();
        scene.set_audio_clips(Some(vec![audio]));
        let mut pinned = Clip::title(1000.0, 1000.0, "pinned");
        pinned.link = Some(Link { source_ms: 1000.0 });
        let mut tracks = vec![];
        tr::add_clip(&mut tracks, pinned);
        tr::add_clip(&mut tracks, Clip::title(2000.0, 1000.0, "fixed"));
        scene.set_tracks(tracks);

        insert_card(&mut scene, 0, 2000.0);
        assert_eq!(scene.audio_clips().unwrap()[0].output_start_ms, 2000.0, "the voice stays with the picture");
        let starts = |s: &Scene| s.tracks().iter().flat_map(|t| t.clips.clone()).map(|c| (c.text.unwrap().content, c.output_start_ms)).collect::<Vec<_>>();
        assert!(starts(&scene).contains(&("fixed".into(), 4000.0)));
        assert!(starts(&scene).contains(&("pinned".into(), 1000.0)), "pinned clips follow their link instead");
        set_card_duration(&mut scene, 0, 1000.0);
        assert_eq!(scene.audio_clips().unwrap()[0].output_start_ms, 1000.0);
        assert!(remove_card(&mut scene, 0));
        assert!(!remove_card(&mut scene, 0), "a recording clip is not a card");
        assert_eq!(scene.audio_clips().unwrap()[0].output_start_ms, 0.0);
        assert!(starts(&scene).contains(&("fixed".into(), 2000.0)));
    }

    #[test]
    fn transitions_play_after_the_cut_and_hold_before_it() {
        let mut slices = vec![Slice::new(0.0, 1000.0), Slice::new(5000.0, 7000.0)];
        slices[1].transition = Some(Transition { kind: "crossfade".into(), duration_ms: 400.0 });
        let t = tl(&slices);
        assert!(transition_at(&slices, &t, 999.0).is_none());
        let mid = transition_at(&slices, &t, 1200.0).unwrap();
        assert_eq!((mid.slice, mid.kind.as_str()), (1, "crossfade"));
        assert!((mid.progress - 0.5).abs() < 1e-6);
        assert!(transition_at(&slices, &t, 1400.0).is_none());
        assert_eq!(hold_before(&slices, &t, 900.0), Some(1));
        assert_eq!(hold_before(&slices, &t, 700.0), None);
        assert_eq!(hold_before(&slices, &t, 1100.0), None);

        // the first clip fades in from black and never holds anything
        slices[0].transition = Some(Transition::new("fade-black"));
        assert_eq!(transition_at(&slices, &tl(&slices), 100.0).unwrap().slice, 0);
    }
}
