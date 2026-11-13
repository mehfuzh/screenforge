//! "My voice": the user's own voice, read on this computer by Chatterbox Turbo
//! ([`sf_tts::clone`]) from a short recording of them; nothing is uploaded. The user records
//! (or picks voice-overs they recorded), names it and confirms it is their own voice; the
//! voice is then a trimmed reference recording and its name, kept in the app's data folder.
//! Narration, the preview and the agent read in it as the voice `my_voice`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use sf_tts::clone::{MAX_REFERENCE_S, MY_VOICE};
use sf_tts::SAMPLE_RATE;

/// Least of the user's voice a voice is made from, the amount that sounds best, and the most a
/// single recording takes (the model listens to the first [`MAX_REFERENCE_S`] of voice).
pub const MIN_MS: f64 = 10_000.0;
pub const BEST_MS: f64 = 20_000.0;
pub const MAX_MS: f64 = 60_000.0;
/// Level below which the start and end of a recording are trimmed off as silence.
const SILENCE: f32 = 0.01;
/// Pause put between recordings joined into the reference.
const JOIN_GAP_S: f64 = 0.3;

/// Something to read aloud for the sample: about half a minute at a normal pace, with the
/// questions, numbers and short and long sentences a voice is best learned from.
pub const READ_ALOUD: &str = "Hi, this is me recording a sample of my own voice. I'm reading at my normal pace, the way I would explain something to a friend. Today I'll walk you through a quick demo: first we open the project, then we trim the start, and export it in about two minutes. Does that sound simple? It is. Numbers like 3, 25 and 1,000 should sound natural too.";

/// What the user agrees to before a voice is made.
pub const CONSENT: &str = "This is my own voice. I will only use it for my own narration, not to imitate anyone else.";

/// One recording made for the voice in the panel (kept outside the project).
#[derive(Debug, Clone)]
pub struct Recorded {
    pub path: PathBuf,
    /// How the form lists it ("Recording 2" when `None`).
    pub label: Option<String>,
    pub ms: f64,
    pub silent: bool,
    pub clipped: bool,
}

/// Where recordings made for the voice are kept until it is made.
fn recordings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("my-voice-recordings"))
}

/// An audio file a recording can be taken from (dropped on the form).
pub fn is_audio(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "aif" | "aiff" | "caf")
}

/// Where the voices are kept: a folder per voice with `reference.wav` and `voice.json`.
fn voices_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.data_dir().join("my-voice-local"))
}

/// The folder of voice `id` (`my_voice:<key>`).
fn voice_dir(id: &str) -> Option<PathBuf> {
    let key = id.strip_prefix(MY_VOICE)?.strip_prefix(':')?;
    // keys are made here: letters and digits only, never a path
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    voices_dir().map(|d| d.join(key))
}

/// A voice made on this computer.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceClone {
    /// `my_voice:<key>`
    pub id: String,
    pub name: String,
    pub seconds: f64,
    pub created_at: String,
}

fn read_voice(id: &str) -> Option<VoiceClone> {
    let dir = voice_dir(id)?;
    if !dir.join("reference.wav").is_file() {
        return None;
    }
    let v: serde_json::Value = std::fs::read_to_string(dir.join("voice.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    Some(VoiceClone {
        id: id.to_string(),
        name: v["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(sf_tts::clone_name()).to_string(),
        seconds: v["seconds"].as_f64().unwrap_or(0.0),
        created_at: v["createdAt"].as_str().unwrap_or_default().to_string(),
    })
}

/// The voices made, oldest first.
pub fn list() -> Vec<VoiceClone> {
    let Some(dir) = voices_dir() else { return vec![] };
    let mut out: Vec<VoiceClone> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| read_voice(&format!("{MY_VOICE}:{}", e.file_name().to_string_lossy()))).collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

/// Voice `id`: `my_voice` alone is the first one.
pub fn get(id: &str) -> Option<VoiceClone> {
    if id == MY_VOICE {
        return list().into_iter().next();
    }
    read_voice(id)
}

/// What voice `id` is called: the name the user gave it, or "My voice".
pub fn name(id: &str) -> String {
    get(id).map_or_else(|| sf_tts::clone_name().to_string(), |c| c.name)
}

/// The longest name a voice can have.
pub const NAME_MAX: usize = 40;

/// Keep a name being typed within [`NAME_MAX`] characters.
pub fn limit_name(name: &mut String) {
    if let Some((i, _)) = name.char_indices().nth(NAME_MAX) {
        name.truncate(i);
    }
}

/// `name` as kept: trimmed, at most [`NAME_MAX`] characters, "My voice" when empty.
fn clean_name(name: &str) -> String {
    let n: String = name.trim().chars().take(NAME_MAX).collect();
    if n.is_empty() {
        sf_tts::clone_name().to_string()
    } else {
        n
    }
}

fn save_meta(c: &VoiceClone) -> Result<(), String> {
    let dir = voice_dir(&c.id).ok_or("no folder for your voice")?;
    let json = serde_json::json!({ "name": c.name, "seconds": c.seconds, "createdAt": c.created_at });
    std::fs::write(dir.join("voice.json"), json.to_string()).map_err(|e| format!("Could not save your voice: {e}"))
}

/// Call voice `id` `name` from now on.
pub fn rename(id: &str, name: &str) -> Result<(), String> {
    let Some(mut c) = read_voice(id) else { return Ok(()) };
    c.name = clean_name(name);
    save_meta(&c)
}

/// The recording voice `id` is read from, mono at [`SAMPLE_RATE`].
pub fn reference(id: &str) -> Result<Vec<f32>, String> {
    let c = get(id).ok_or("You have not made your voice yet. Make it under Narration → My voice.")?;
    let path = voice_dir(&c.id).map(|d| d.join("reference.wav")).ok_or("no folder for your voice")?;
    sf_media::audio::decode(&path, SAMPLE_RATE, 1).map(|c| c.samples.to_vec()).map_err(|e| format!("Could not read your voice: {e}"))
}

/// Delete voice `id` (takes already read keep their audio).
pub fn delete(id: &str) -> Result<(), String> {
    let Some(dir) = voice_dir(id).filter(|d| d.exists()) else { return Ok(()) };
    crate::narration::forget_previews(id);
    std::fs::remove_dir_all(dir).map_err(|e| format!("Could not delete your voice: {e}"))
}

/// A new voice's id.
fn new_id() -> String {
    format!("{MY_VOICE}:{}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// The voice model is downloaded first (once).
    Downloading,
    /// The voice is read once (its preview), so it is ready to use.
    Preparing,
}

enum Making {
    Stage(Work),
    Progress(sf_tts::Progress),
    /// How far preparing the voice is (0–1).
    Reading(f32),
    Done(Result<String, String>),
}

#[derive(Default)]
pub struct MyVoiceJob {
    running: Option<(Receiver<Making>, Work)>,
    /// How far the model's download is: (done, total) bytes.
    pub progress: Option<(u64, u64)>,
    /// How far preparing the voice is (0–1).
    pub reading: Option<f32>,
    /// The voice just made (or made again), for the panel to pick and keep in view.
    pub made: Option<String>,
    pub error: Option<String>,
    /// The voice the form makes again (`None`: a new voice).
    pub editing: Option<String>,
    /// The panel's form: the recordings picked (the project's voice-overs, or recordings made
    /// here), the consent box and the name.
    pub picked: Vec<PathBuf>,
    pub consent: bool,
    /// Leave background noise in the recordings (off: hiss and hum are taken out before the
    /// voice is made from them, so the voice does not read with them).
    pub keep_noise: bool,
    /// What to call the voice (empty: "My voice").
    pub name: String,
    /// The form is open (making the voice, or making it again).
    pub open: bool,
    /// Recordings made here, newest last.
    pub recorded: Vec<Recorded>,
    rec: Option<crate::voice_over::VoiceRecorder>,
    /// Listens to the microphone while the form is open and nothing records, so its bars show
    /// it hears you before you start.
    meter: Option<crate::mic_meter::MicMeter>,
    levels: VecDeque<f32>,
    levels_at: Option<Instant>,
    pub rec_error: Option<String>,
    /// When the panel last drew the form (the meter lets go of the microphone soon after it
    /// stops being shown).
    shown_at: Option<Instant>,
}

/// What the panel shows of the microphone.
pub struct Mic {
    pub levels: Vec<f32>,
    pub recording: bool,
    pub elapsed_ms: f64,
    /// Nothing heard for the first couple of seconds of a recording.
    pub no_sound: bool,
    pub too_loud: bool,
}

impl MyVoiceJob {
    /// A job with the Make my voice form open (`open`) or not.
    pub fn with_form(open: bool) -> Self {
        MyVoiceJob { open, ..Default::default() }
    }

    pub fn working(&self) -> Option<Work> {
        self.running.as_ref().map(|r| r.1)
    }

    /// Keep the meter going (`mic_id`: the voice-over microphone, `None` for the default) and
    /// stop a recording that reached [`MAX_MS`]; what the panel draws.
    pub fn mic(&mut self, mic_id: Option<&str>) -> Mic {
        self.shown_at = Some(Instant::now());
        if self.rec.as_ref().is_some_and(|r| r.elapsed_ms() >= MAX_MS) {
            self.stop_recording();
        }
        let id = mic_id.unwrap_or("default");
        if self.rec.is_none() && self.meter.as_ref().is_none_or(|m| m.device_name != id) {
            self.meter = crate::mic_meter::MicMeter::start(id);
        }
        let level = match (&self.rec, &self.meter) {
            (Some(r), _) => r.level(),
            (None, Some(m)) => m.level(),
            _ => 0.0,
        };
        if self.levels_at.is_none_or(|t| t.elapsed().as_millis() >= 50) {
            self.levels_at = Some(Instant::now());
            self.levels.push_back(level);
            while self.levels.len() > 48 {
                self.levels.pop_front();
            }
        }
        let elapsed_ms = self.rec.as_ref().map_or(0.0, |r| r.elapsed_ms());
        Mic {
            levels: self.levels.iter().copied().collect(),
            recording: self.rec.is_some(),
            elapsed_ms,
            no_sound: elapsed_ms > 2000.0 && self.rec.as_ref().is_some_and(|r| r.is_silent()),
            too_loud: self.rec.as_ref().is_some_and(|r| r.is_clipping()),
        }
    }

    /// Record from microphone `mic` (the system default when `None`).
    pub fn start_recording(&mut self, mic: Option<&sf_recording::discover::AudioDevice>, name: String) {
        if self.rec.is_some() {
            return;
        }
        // the meter lets go of the microphone before the recording opens it
        self.meter = None;
        self.rec_error = None;
        match crate::voice_over::VoiceRecorder::start(0.0, mic, name) {
            Ok(r) => self.rec = Some(r),
            Err(e) => self.rec_error = Some(format!("Could not start the microphone: {e}")),
        }
    }

    /// Stop and keep the recording (ticked for the voice), unless it was a slip of the button.
    pub fn stop_recording(&mut self) {
        let Some(rec) = self.rec.take() else { return };
        let Some(dir) = recordings_dir() else { return };
        match rec.finish(&dir) {
            Ok(Some(t)) => {
                let path = dir.join(&t.file_name);
                self.rec_error = if t.silent {
                    Some("That recording is silent: the microphone sent no voice (muted, or its input volume is too low?).".into())
                } else if t.clipped {
                    Some("That recording is distorted: the microphone was too loud. Lower its input volume or move back, and record again.".into())
                } else {
                    None
                };
                if !t.silent {
                    self.picked.push(path.clone());
                }
                self.recorded.push(Recorded { path, label: None, ms: t.duration_ms, silent: t.silent, clipped: t.clipped });
            }
            Ok(None) => self.rec_error = Some("Too short: hold on and read the text aloud, then press Stop.".into()),
            Err(e) => self.rec_error = Some(format!("Could not save the recording: {e}")),
        }
    }

    /// Throw recording `path` away.
    pub fn discard(&mut self, path: &Path) {
        self.recorded.retain(|r| r.path != path);
        self.picked.retain(|p| p != path);
        let _ = std::fs::remove_file(path);
    }

    /// Recordings dropped on the form: copied into the form's folder (the originals are never
    /// touched) and ticked for the voice.
    pub fn add_files(&mut self, files: &[PathBuf]) {
        let Some(dir) = recordings_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "recording".into());
            let copy = dir.join(format!("dropped-{}-{name}", chrono::Local::now().format("%H%M%S%3f")));
            let ms = std::fs::copy(f, &copy).map_err(|e| e.to_string()).and_then(|_| sf_media::audio::decode(&copy, 16_000, 1).map(|c| c.samples.len() as f64 / 16.0).map_err(|e| e.to_string()));
            match ms {
                Ok(ms) => {
                    self.picked.push(copy.clone());
                    self.recorded.push(Recorded { path: copy, label: Some(name), ms, silent: false, clipped: false });
                    self.rec_error = None;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&copy);
                    self.rec_error = Some(format!("Could not use {name}: {e}"));
                }
            }
        }
    }

    /// The form closed: stop listening (a recording in progress is thrown away).
    pub fn close(&mut self) {
        self.open = false;
        self.editing = None;
        self.rec = None;
        self.meter = None;
        self.levels.clear();
    }

    /// The microphone is open for the form (the editor repaints for the bars).
    pub fn listening(&self) -> bool {
        self.rec.is_some() || self.meter.is_some()
    }

    /// Make the voice from the picked recordings (voice-overs of the project, or made here):
    /// their voice, trimmed of silence and joined, kept as the reference.
    pub fn create(&mut self) {
        if self.running.is_some() {
            return;
        }
        let (files, name, id, denoise) = (self.picked.clone(), clean_name(&self.name), self.editing.clone(), !self.keep_noise);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = make(&files, name, id, denoise, &tx);
            let _ = tx.send(Making::Done(r));
        });
        self.running = Some((rx, Work::Downloading));
        self.progress = None;
        self.reading = None;
        self.error = None;
    }

    /// Drain the result of a finished run; let go of the microphone's meter once the form is
    /// no longer shown (another panel, or the sidebar scrolled on).
    pub fn poll(&mut self) {
        if self.meter.is_some() && self.shown_at.is_none_or(|t| t.elapsed().as_millis() > 500) {
            self.meter = None;
            self.levels.clear();
        }
        let Some((rx, _)) = &self.running else { return };
        let messages: Vec<Making> = rx.try_iter().collect();
        let mut done = None;
        for m in messages {
            match m {
                Making::Stage(w) => {
                    if let Some(r) = self.running.as_mut() {
                        r.1 = w;
                    }
                }
                Making::Progress(sf_tts::Progress::Downloading { done, total: Some(t) }) => self.progress = Some((done, t)),
                Making::Progress(_) => {}
                Making::Reading(f) => self.reading = Some(f),
                Making::Done(r) => done = Some(r),
            }
        }
        let Some(r) = done else { return };
        self.running = None;
        self.progress = None;
        self.reading = None;
        match r {
            Ok(id) => {
                self.made = Some(id);
                self.close();
                self.consent = false;
                self.picked.clear();
                self.name.clear();
                for r in std::mem::take(&mut self.recorded) {
                    let _ = std::fs::remove_file(r.path);
                }
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// Make the voice from `files` (blocking): the model is downloaded first, the voice is read
/// once (its preview, kept) to be sure it works, and only then saved, as voice `id` (made again)
/// or a new one. Returns its id.
fn make(files: &[PathBuf], name: String, id: Option<String>, denoise: bool, tx: &std::sync::mpsc::Sender<Making>) -> Result<String, String> {
    let reference = joined_voice(files, denoise)?;
    let _ = tx.send(Making::Stage(Work::Downloading));
    sf_tts::clone::ensure(&std::sync::atomic::AtomicBool::new(false), |p| {
        let _ = tx.send(Making::Progress(p));
    })
    .map_err(|e| format!("Could not download the voice model: {e:#}"))?;
    let _ = tx.send(Making::Stage(Work::Preparing));
    let id = id.filter(|i| voice_dir(i).is_some()).unwrap_or_else(new_id);
    let voice = VoiceClone { id: id.clone(), name, seconds: (reference.len() as f64 / SAMPLE_RATE as f64 * 10.0).round() / 10.0, created_at: chrono::Utc::now().to_rfc3339() };
    crate::narration::prepare_my_voice(&voice, &reference, |f| {
        let _ = tx.send(Making::Reading(f));
    })?;
    let dir = voice_dir(&id).ok_or("no folder for your voice")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not save your voice: {e}"))?;
    sf_tts::Speech { samples: reference, words: vec![] }.write_wav(&dir.join("reference.wav")).map_err(|e| format!("Could not save your voice: {e:#}"))?;
    save_meta(&voice)?;
    Ok(id)
}

/// The picked recordings' voice, trimmed of silence at both ends, joined with short pauses and
/// cut off at [`MAX_REFERENCE_S`].
fn joined_voice(files: &[PathBuf], denoise: bool) -> Result<Vec<f32>, String> {
    let max = (MAX_REFERENCE_S * SAMPLE_RATE as f64) as usize;
    // the microphone's Noise reduction preset: rumble cut and FFT noise reduction
    let filter = denoise.then(|| sf_media::audio::microphone_filter("noise-reduction", 0.0)).flatten();
    let mut out: Vec<f32> = vec![];
    let mut voiced = 0usize;
    for f in files {
        if out.len() >= max {
            break;
        }
        let clip = sf_media::audio::decode_filtered(f, SAMPLE_RATE, 1, filter.as_deref()).map_err(|e| format!("Could not read {}: {e}", f.display()))?;
        let s = trim_silence(&clip.samples);
        // a second or less is a slip, not a sample of the voice
        if s.len() < SAMPLE_RATE as usize {
            continue;
        }
        if !out.is_empty() {
            out.extend(std::iter::repeat_n(0.0, (JOIN_GAP_S * SAMPLE_RATE as f64) as usize));
        }
        voiced += s.len();
        out.extend(s);
    }
    out.truncate(max);
    if (voiced as f64) < MIN_MS / 1000.0 * SAMPLE_RATE as f64 {
        return Err(format!("These recordings have {:.0} s of voice; at least {:.0} s are needed (about 20 s sounds best). Record more, or pick more takes.", voiced as f64 / SAMPLE_RATE as f64, MIN_MS / 1000.0));
    }
    // a steady level, whatever the microphone's gain
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    out.iter_mut().for_each(|v| *v *= 0.89 / peak);
    Ok(out)
}

/// `samples` without the silence before the first sound and after the last.
fn trim_silence(samples: &[f32]) -> Vec<f32> {
    let start = samples.iter().position(|s| s.abs() > SILENCE).unwrap_or(samples.len());
    let end = samples.iter().rposition(|s| s.abs() > SILENCE).map_or(start, |i| i + 1);
    samples[start..end.max(start)].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_at_both_ends_is_trimmed() {
        assert_eq!(trim_silence(&[0.0, 0.001, 0.5, 0.0, -0.3, 0.002, 0.0]), vec![0.5, 0.0, -0.3]);
        assert!(trim_silence(&[0.0; 10]).is_empty());
    }

    #[test]
    fn only_voice_ids_made_here_have_a_folder() {
        assert!(voice_dir("my_voice:20261112093000123").is_some_and(|d| d.ends_with("20261112093000123")));
        for bad in ["my_voice", "my_voice:", "my_voice:../x", "my_voice:a/b", "am_fenrir", "my_voicex:1"] {
            assert!(voice_dir(bad).is_none(), "{bad}");
        }
        assert!(sf_tts::clone::is_my_voice("my_voice") && sf_tts::clone::is_my_voice("my_voice:1"));
        assert!(!sf_tts::clone::is_my_voice("my_voices") && !sf_tts::clone::is_my_voice("el_eric"));
    }

    #[test]
    fn names_are_trimmed_and_never_empty() {
        assert_eq!(clean_name("  Mehfuz  "), "Mehfuz");
        assert_eq!(clean_name("   "), "My voice");
        assert_eq!(clean_name(&"x".repeat(60)).chars().count(), NAME_MAX);
        let mut typed = "y".repeat(50);
        limit_name(&mut typed);
        assert_eq!(typed.chars().count(), NAME_MAX);
    }
}
