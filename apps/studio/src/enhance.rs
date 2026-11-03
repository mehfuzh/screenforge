//! Enhancing a recorded voice-over with ElevenLabs, through the backend's `voice-enhance`
//! function (which holds the key and charges credits), off the UI thread. Either way the user's
//! own timing, delivery and accent stay:
//! - **Clean up** (Voice Isolator): background noise, echo and hum taken out, still their voice.
//! - **Re-voice** (Voice Changer): the same performance in one of the ElevenLabs voices.
//!
//! The take's whole recording is enhanced (takes split from one recording share the file, so
//! they all switch), in pieces of about 40 s cut where it is quiet. Each piece comes back as MP3,
//! is decoded with ffmpeg and fitted to exactly the length sent, so the new file lines up sample
//! for sample with the recording: trims, splits and captions stay where they were. The
//! recording is kept beside it, to go back to.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use base64::Engine;

/// Rate the pieces are sent at and the new file is written at (the Voice Changer answers at
/// 44.1 kHz).
const RATE: u32 = 44_100;
/// About how long a piece is.
const PIECE_MS: f64 = 40_000.0;
/// How far before the 40 s mark a cut may go looking for a quiet spot.
const SEARCH_MS: f64 = 12_000.0;
/// The shortest piece (a shorter tail joins the piece before it).
const MIN_PIECE_MS: f64 = 8_000.0;
/// Fade at each join, at a quiet spot anyway, so pieces never click against each other.
const JOIN_FADE_MS: f64 = 5.0;

/// Credits a minute of audio costs (ElevenLabs bills 1,000 characters a minute: US$0.08 at
/// 1 credit = US$0.01); the backend charges what it is set to, this is for the panel.
pub const CREDITS_PER_MINUTE: f64 = 8.0;

/// What to do with the voice-over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Clean,
    /// Re-voice in this ElevenLabs voice (`el_eric` …).
    Revoice(String),
}

impl Mode {
    /// How the take is marked (see `VoiceOver::enhanced`).
    fn label(&self) -> &str {
        match self {
            Mode::Clean => "clean",
            Mode::Revoice(v) => v,
        }
    }
}

/// A finished enhancement: takes playing `from` now play `file_name`.
#[derive(Debug, Clone)]
pub struct Enhanced {
    pub from: String,
    pub file_name: String,
    pub how: String,
}

enum Msg {
    Piece(usize, usize),
    Done(Result<Enhanced, String>),
}

#[derive(Default)]
pub struct EnhanceJob {
    running: Option<(Receiver<Msg>, Arc<AtomicBool>)>,
    /// The take it was started from (its status shows by that take).
    pub take: Option<String>,
    mode: Option<Mode>,
    /// Pieces done of all of them.
    progress: Option<(usize, usize)>,
    /// The last run's error, and the take it was for.
    pub error: Option<(String, String)>,
}

impl EnhanceJob {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn cancel(&mut self) {
        if let Some((_, c)) = &self.running {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Enhance the recording of take `take` (`recorded`, in `project`'s voice-over folder)
    /// that the take now plays as `playing`.
    pub fn start(&mut self, project: PathBuf, take: &str, recorded: String, playing: String, mode: Mode) {
        if self.running.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let m = mode.clone();
        std::thread::spawn(move || {
            let ptx = tx.clone();
            let r = enhance(&project, &recorded, &m, &c, &|done, all| {
                let _ = ptx.send(Msg::Piece(done, all));
            });
            let _ = tx.send(Msg::Done(r.map(|file_name| Enhanced { from: playing, file_name, how: m.label().to_string() })));
        });
        self.running = Some((rx, cancel));
        self.take = Some(take.to_string());
        self.mode = Some(mode);
        self.progress = None;
        self.error = None;
    }

    /// Drain messages: the enhancement when it just finished.
    pub fn poll(&mut self) -> Option<Enhanced> {
        let mut out = None;
        let mut done = false;
        if let Some((rx, _)) = &self.running {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Piece(d, a) => self.progress = Some((d, a)),
                    Msg::Done(r) => {
                        done = true;
                        match r {
                            Ok(e) => out = Some(e),
                            Err(e) if e == "cancelled" => {}
                            Err(e) => self.error = Some((self.take.clone().unwrap_or_default(), e)),
                        }
                    }
                }
            }
        }
        if done {
            self.running = None;
            self.take = None;
            self.progress = None;
        }
        out
    }

    /// "ElevenLabs is cleaning up the voice-over… 2 of 5" while it runs.
    pub fn status(&self) -> Option<String> {
        self.running.as_ref()?;
        let what = match &self.mode {
            Some(Mode::Revoice(v)) => format!("ElevenLabs is re-voicing it as {}…", sf_tts::cloud::cloud_voice(v).map_or("another voice", |c| c.name)),
            _ => "ElevenLabs is cleaning up the voice-over…".to_string(),
        };
        Some(match self.progress {
            Some((d, a)) if a > 1 => format!("{what} {} of {a}", (d + 1).min(a)),
            _ => what,
        })
    }
}

/// Enhance `recorded` into a new file beside it; returns the new file's name.
fn enhance(project: &Path, recorded: &str, mode: &Mode, cancel: &AtomicBool, progress: &dyn Fn(usize, usize)) -> Result<String, String> {
    let dir = project.join(sf_project::v4::VoiceOver::DIR);
    let clip = sf_media::audio::decode(&dir.join(recorded), RATE, 1).map_err(|e| format!("Could not read the voice-over: {e}"))?;
    let samples = &clip.samples[..];
    if samples.is_empty() {
        return Err("The voice-over has no audio.".into());
    }
    let ranges = pieces(samples, RATE);
    let mut out = Vec::with_capacity(samples.len());
    for (i, r) in ranges.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        progress(i, ranges.len());
        let mut piece = enhance_piece(&samples[r.clone()], mode)?;
        fit(&mut piece, r.len());
        if i > 0 {
            fade(&mut piece, true);
        }
        if i + 1 < ranges.len() {
            fade(&mut piece, false);
        }
        out.extend(piece);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let stem = Path::new(recorded).file_stem().and_then(|s| s.to_str()).unwrap_or("voiceover");
    let name = format!("{stem}-{}-{}.wav", mode.label().trim_start_matches("el_"), chrono::Local::now().format("%Y%m%d-%H%M%S"));
    crate::voice_over::write_wav(&dir.join(&name), RATE, &out).map_err(|e| format!("{e:#}"))?;
    // the words are where they were: the recording's captions are the new file's
    if let Ok(Some(mut t)) = sf_project::transcript::TranscriptFile::load_voice_over(project, recorded) {
        t.location.file_name = Some(name.clone());
        if let Err(e) = t.save(project) {
            tracing::warn!("could not copy the voice-over's captions: {e}");
        }
    }
    Ok(name)
}

/// Send one piece to the backend; its answer decoded at [`RATE`].
fn enhance_piece(samples: &[f32], mode: &Mode) -> Result<Vec<f32>, String> {
    let device = sf_license::device_id().ok_or("ElevenLabs needs this computer's id, which could not be read")?;
    let audio = base64::engine::general_purpose::STANDARD.encode(crate::voice_over::wav_bytes(RATE, samples));
    let mut body = serde_json::json!({ "device": device, "mode": if *mode == Mode::Clean { "clean" } else { "revoice" }, "audio": audio });
    if let Mode::Revoice(v) = mode {
        body["voice"] = v.as_str().into();
    }
    let reply = sf_license::call_backend_within("voice-enhance", body, 240)?;
    crate::narration::note_credits(&reply);
    let mp3 = base64::engine::general_purpose::STANDARD.decode(reply["audio"].as_str().ok_or("ElevenLabs returned no audio")?).map_err(|e| e.to_string())?;
    let tmp = std::env::temp_dir().join(format!("screenforge-enhance-{}-{}.mp3", std::process::id(), chrono::Local::now().format("%H%M%S%3f")));
    std::fs::write(&tmp, mp3).map_err(|e| e.to_string())?;
    let decoded = sf_media::audio::decode(&tmp, RATE, 1);
    let _ = std::fs::remove_file(&tmp);
    Ok(decoded.map_err(|e| format!("Could not read ElevenLabs' audio: {e}"))?.samples.to_vec())
}

/// Where to cut `samples` into pieces of about [`PIECE_MS`]: at the quietest 50 ms in the
/// [`SEARCH_MS`] before each mark (so a cut falls between words), none shorter than
/// [`MIN_PIECE_MS`].
fn pieces(samples: &[f32], rate: u32) -> Vec<Range<usize>> {
    let at = |ms: f64| (ms * rate as f64 / 1000.0) as usize;
    let (target, search, min, window) = (at(PIECE_MS), at(SEARCH_MS), at(MIN_PIECE_MS), at(50.0).max(1));
    let mut out = vec![];
    let mut start = 0;
    while samples.len() - start > target + min {
        let (lo, hi) = (start + target - search, (start + target).min(samples.len() - min));
        let energy = |w: usize| samples[w..(w + window).min(samples.len())].iter().map(|s| s * s).sum::<f32>();
        let quietest = (lo..hi.saturating_sub(window).max(lo + 1)).step_by(window / 2 + 1).min_by(|a, b| energy(*a).total_cmp(&energy(*b))).unwrap_or(lo);
        let cut = quietest + window / 2;
        out.push(start..cut);
        start = cut;
    }
    out.push(start..samples.len());
    out
}

/// Make `piece` exactly `len` samples: ElevenLabs' answer can be a few ms longer or shorter
/// (MP3 framing), which would otherwise pile up and drift the captions.
fn fit(piece: &mut Vec<f32>, len: usize) {
    piece.resize(len, 0.0);
}

/// Fade the start (`in_`) or the end of `piece` over [`JOIN_FADE_MS`].
fn fade(piece: &mut [f32], in_: bool) {
    let n = ((JOIN_FADE_MS * RATE as f64 / 1000.0) as usize).min(piece.len());
    let len = piece.len();
    for i in 0..n {
        let g = i as f32 / n as f32;
        if in_ {
            piece[i] *= g;
        } else {
            piece[len - 1 - i] *= g;
        }
    }
}

/// Credits enhancing `ms` of audio costs, for the button.
pub fn credits_for(ms: f64) -> f64 {
    ms / 60_000.0 * CREDITS_PER_MINUTE
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u32 = 1_000;

    /// Speech everywhere except a silent 100 ms at each of `quiet` (ms).
    fn speech(ms: usize, quiet: &[usize]) -> Vec<f32> {
        (0..ms).map(|i| if quiet.iter().any(|q| (*q..q + 100).contains(&i)) { 0.0 } else { 0.5 }).collect()
    }

    #[test]
    fn a_short_take_is_one_piece() {
        let s = speech(45_000, &[]);
        assert_eq!(pieces(&s, R), vec![0..45_000]);
    }

    #[test]
    fn a_long_take_is_cut_in_its_pauses_and_covers_every_sample() {
        let s = speech(130_000, &[33_000, 70_000, 105_000]);
        let p = pieces(&s, R);
        assert_eq!(p.first().unwrap().start, 0);
        assert_eq!(p.last().unwrap().end, s.len());
        for w in p.windows(2) {
            assert_eq!(w[0].end, w[1].start, "no gaps or overlaps: {p:?}");
        }
        for r in &p[..p.len() - 1] {
            assert_eq!(s[r.end], 0.0, "cut in a pause: {p:?}");
        }
        assert!(p.iter().all(|r| r.len() >= 8_000 && r.len() <= 60_000), "{p:?}");
    }

    #[test]
    fn a_short_tail_joins_the_piece_before_it() {
        let p = pieces(&speech(46_000, &[]), R);
        assert_eq!(p.len(), 1, "6 s left over is not a piece of its own: {p:?}");
    }

    #[test]
    fn an_answer_is_fitted_to_the_length_sent() {
        let mut longer = vec![0.1; 1_030];
        fit(&mut longer, 1_000);
        assert_eq!(longer.len(), 1_000);
        let mut shorter = vec![0.1; 990];
        fit(&mut shorter, 1_000);
        assert_eq!((shorter.len(), shorter[999]), (1_000, 0.0));
    }
}
