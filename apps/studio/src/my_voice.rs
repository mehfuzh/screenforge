//! "My voice": the user's own voice, cloned by ElevenLabs from voice-overs they recorded, through
//! the backend's `my-voice` function (one clone per Pro license; the user confirms it is their
//! voice). Once made it is the voice `my_voice` ([`sf_tts::cloud::MY_VOICE`]): Narration, the
//! agent and Re-voice can speak in it, paid like the other ElevenLabs voices.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::Instant;

use base64::Engine;

/// Rate the recordings are sent at: plenty for a clone, and 3 minutes stay under 8 MB.
const RATE: u32 = 22_050;
/// Least and most of the user's voice sent (ElevenLabs: 1–2 minutes sound best, more adds little).
pub const MIN_MS: f64 = 10_000.0;
pub const BEST_MS: f64 = 60_000.0;
pub const MAX_MS: f64 = 180_000.0;
/// Level below which the start and end of a recording are trimmed off as silence.
const SILENCE: f32 = 0.01;

/// Something to read aloud for the sample: about a minute at a normal pace, with the questions,
/// numbers and short and long sentences a clone learns the most from.
pub const READ_ALOUD: &str = "Hi, this is me recording a sample of my own voice. I'm reading at my normal pace, the way I would explain something to a friend. Today I'll walk you through a quick demo: first we open the project, then we trim the start, add a zoom on the button, and export it in about two minutes. Does that sound simple? It is. Some days I talk a little faster, and some days I take my time... but my voice stays the same. Numbers like 3, 25 and 1,000 should sound natural too. Thanks for listening, and let's get started.";

/// One recording made for the voice in the panel (kept outside the project).
#[derive(Debug, Clone)]
pub struct Recorded {
    pub path: PathBuf,
    pub ms: f64,
    pub silent: bool,
    pub clipped: bool,
}

/// Where recordings made for the voice are kept until it is made.
fn recordings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("my-voice-recordings"))
}

/// What the user agrees to before a clone is made (the backend keeps its own copy).
pub const CONSENT: &str = "This is my own voice. I agree to ElevenLabs making a clone of it for my narration, and I will not use it to imitate anyone else.";

/// The clone the backend has for this computer's license.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceClone {
    pub seconds: f64,
    pub created_at: String,
}

/// `None` until checked; then whether this license has a clone.
static STATE: Mutex<Option<Option<VoiceClone>>> = Mutex::new(None);

/// This license has made its voice (as far as the last check knows).
pub fn has() -> bool {
    STATE.lock().ok().and_then(|s| s.clone()).flatten().is_some()
}

pub fn current() -> Option<VoiceClone> {
    STATE.lock().ok().and_then(|s| s.clone()).flatten()
}

fn set(c: Option<VoiceClone>) {
    if let Ok(mut s) = STATE.lock() {
        *s = Some(c);
    }
}

fn from_reply(v: &serde_json::Value) -> Option<VoiceClone> {
    let c = v.get("voice").filter(|c| c.is_object())?;
    Some(VoiceClone { seconds: c["seconds"].as_f64().unwrap_or(0.0), created_at: c["createdAt"].as_str().unwrap_or_default().to_string() })
}

fn call(body: serde_json::Value, secs: u64) -> Result<serde_json::Value, String> {
    let device = sf_license::device_id().ok_or("My voice needs this computer's id, which could not be read")?;
    let mut body = body;
    body["device"] = device.into();
    sf_license::call_backend_within("my-voice", body, secs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Checking,
    Creating,
    Deleting,
}

#[derive(Default)]
pub struct MyVoiceJob {
    running: Option<(Receiver<Result<Option<VoiceClone>, String>>, Work)>,
    checked: bool,
    pub error: Option<String>,
    /// The panel's form: the recordings picked (the project's voice-overs, or recordings made
    /// here), the consent box, and whether ElevenLabs should take out background noise first.
    pub picked: Vec<PathBuf>,
    pub consent: bool,
    pub clean: bool,
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

    fn start(&mut self, work: Work, f: impl FnOnce() -> Result<Option<VoiceClone>, String> + Send + 'static) {
        if self.running.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.running = Some((rx, work));
        self.error = None;
    }

    /// Ask the backend once whether this license has a voice.
    pub fn ensure_checked(&mut self) {
        if self.checked || self.running.is_some() {
            return;
        }
        self.checked = true;
        self.start(Work::Checking, || call(serde_json::json!({ "action": "status" }), 15).map(|v| from_reply(&v)));
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
                self.recorded.push(Recorded { path, ms: t.duration_ms, silent: t.silent, clipped: t.clipped });
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

    /// The form closed: stop listening (a recording in progress is thrown away).
    pub fn close(&mut self) {
        self.open = false;
        self.rec = None;
        self.meter = None;
        self.levels.clear();
    }

    /// The microphone is open for the form (the editor repaints for the bars).
    pub fn listening(&self) -> bool {
        self.rec.is_some() || self.meter.is_some()
    }

    /// Make the voice from the picked recordings (voice-overs of the project, or made here).
    pub fn create(&mut self) {
        let (files, clean) = (self.picked.clone(), self.clean);
        self.start(Work::Creating, move || {
            let samples = samples(&files)?;
            let audio: Vec<String> = samples.iter().map(|s| base64::engine::general_purpose::STANDARD.encode(crate::voice_over::wav_bytes(RATE, s))).collect();
            let reply = call(serde_json::json!({ "action": "create", "consent": true, "clean": clean, "samples": audio }), 240)?;
            from_reply(&reply).map(Some).ok_or_else(|| "ElevenLabs returned no voice".to_string())
        });
    }

    pub fn delete(&mut self) {
        self.start(Work::Deleting, || call(serde_json::json!({ "action": "delete" }), 30).map(|_| None));
    }

    /// Drain the result of a finished run; let go of the microphone's meter once the form is
    /// no longer shown (another panel, or the sidebar scrolled on).
    pub fn poll(&mut self) {
        if self.meter.is_some() && self.shown_at.is_none_or(|t| t.elapsed().as_millis() > 500) {
            self.meter = None;
            self.levels.clear();
        }
        let Some((rx, work)) = &self.running else { return };
        let Ok(r) = rx.try_recv() else { return };
        let work = *work;
        self.running = None;
        match r {
            Ok(c) => {
                if work != Work::Checking {
                    crate::narration::forget_previews(sf_tts::cloud::MY_VOICE.id);
                    self.close();
                    self.consent = false;
                    self.picked.clear();
                    for r in std::mem::take(&mut self.recorded) {
                        let _ = std::fs::remove_file(r.path);
                    }
                }
                set(c);
            }
            // a failed check leaves My voice unknown (hidden); it is asked again next launch
            Err(_) if work == Work::Checking => {}
            Err(e) => self.error = Some(e),
        }
    }
}

/// The picked recordings, trimmed of silence at both ends and cut off at [`MAX_MS`] in all.
fn samples(files: &[PathBuf]) -> Result<Vec<Vec<f32>>, String> {
    let max = (MAX_MS / 1000.0 * RATE as f64) as usize;
    let mut out: Vec<Vec<f32>> = vec![];
    let mut total = 0;
    for f in files {
        if total >= max {
            break;
        }
        let clip = sf_media::audio::decode(f, RATE, 1).map_err(|e| format!("Could not read {}: {e}", f.display()))?;
        let mut s = trim_silence(&clip.samples);
        s.truncate(max - total);
        // ElevenLabs wants each recording at least a second long
        if s.len() >= RATE as usize {
            total += s.len();
            out.push(s);
        }
    }
    if (total as f64) < MIN_MS / 1000.0 * RATE as f64 {
        return Err(format!("These recordings have {:.0} s of voice; ElevenLabs needs at least {:.0} s (about a minute sounds best). Record more, or pick more takes.", total as f64 / RATE as f64, MIN_MS / 1000.0));
    }
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
    fn a_reply_with_a_voice_is_a_clone() {
        let c = from_reply(&serde_json::json!({ "voice": { "name": "My voice", "seconds": 64.5, "createdAt": "2026-11-02" } })).unwrap();
        assert_eq!(c.seconds, 64.5);
        assert_eq!(from_reply(&serde_json::json!({ "voice": null })), None);
    }
}
