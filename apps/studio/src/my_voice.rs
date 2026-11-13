//! "My voice": the user's own voice, read on this computer by Chatterbox Turbo
//! ([`sf_tts::clone`]) from a short recording of them; nothing is uploaded. The user records
//! (or picks voice-overs they recorded), names it and confirms it is their own voice; the
//! voice is then a trimmed reference recording and its name, kept in the app's data folder.
//! Narration, the preview and the agent read in it as the voice `my_voice`.

use std::collections::VecDeque;
use std::io::Read;
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

/// A voice the user cloned in their own ElevenLabs account, with the recordings they made it
/// from: `(sample id, file name)`.
#[derive(Debug, Clone)]
pub struct ElevenLabsVoice {
    pub id: String,
    pub name: String,
    pub samples: Vec<(String, String)>,
    /// ElevenLabs' short sample of the voice, played from the list.
    pub preview_url: Option<String>,
}

/// Where the user's ElevenLabs API key is kept on this computer (readable by them only).
fn key_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.data_dir().join("elevenlabs.key"))
}

/// The ElevenLabs API key saved here, if any.
pub fn saved_key() -> Option<String> {
    key_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

fn save_key(key: &str) -> std::io::Result<()> {
    let path = key_path().ok_or_else(|| std::io::Error::other("no folder for the key"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, key.trim())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn forget_key() {
    if let Some(p) = key_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// Most of a voice's recordings imported (the voice is read from its first 15 s anyway).
const IMPORT_SAMPLES: usize = 4;
const ELEVENLABS: &str = "https://api.elevenlabs.io/v1";

enum Imported {
    /// The account's voices, for the key that listed them.
    Voices(String, Result<Vec<ElevenLabsVoice>, String>),
    /// A voice's recordings, and its name.
    Samples(String, Result<Vec<Recorded>, String>),
    /// A voice's preview, decoded: its id and samples.
    Preview(String, Result<Vec<f32>, String>),
}

/// The voices cloned in the ElevenLabs account of `key` (not ElevenLabs' own voices), with
/// the recordings each was made from.
fn elevenlabs_voices(key: &str) -> Result<Vec<ElevenLabsVoice>, String> {
    let reply: serde_json::Value = ureq::get(&format!("{ELEVENLABS}/voices"))
        .set("xi-api-key", key)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(elevenlabs_error)?
        .into_json()
        .map_err(|e| format!("ElevenLabs sent something unexpected: {e}"))?;
    set_account_voices(account_voices_in(&reply));
    let voices = cloned_voices(&reply);
    if voices.is_empty() {
        return Err("This ElevenLabs account has no cloned voices with recordings to import.".into());
    }
    Ok(voices)
}

/// Voice ids of the user's own ElevenLabs voices, read online with their key: `el_user:<id>`.
pub const ACCOUNT: &str = "el_user:";

/// `id` is a voice of the user's own ElevenLabs account.
pub fn is_account_voice(id: &str) -> bool {
    id.strip_prefix(ACCOUNT).is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The user's own voices in their ElevenLabs account (cloned, professional or designed ones;
/// not ElevenLabs' own, which ScreenForge offers already): `(el_user:<id>, name)`.
static ACCOUNT_VOICES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

pub fn account_voices() -> Vec<(String, String)> {
    ACCOUNT_VOICES.lock().map(|v| v.clone()).unwrap_or_default()
}

fn set_account_voices(v: Vec<(String, String)>) {
    if let Ok(mut a) = ACCOUNT_VOICES.lock() {
        *a = v;
    }
}

/// The name of account voice `id`, when it is known.
pub fn account_voice_name(id: &str) -> Option<String> {
    account_voices().into_iter().find(|(v, _)| v == id).map(|(_, n)| n)
}

fn account_voices_in(reply: &serde_json::Value) -> Vec<(String, String)> {
    reply["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| matches!(v["category"].as_str(), Some("cloned" | "professional" | "generated")))
        .filter_map(|v| Some((format!("{ACCOUNT}{}", v["voice_id"].as_str().filter(|i| i.chars().all(|c| c.is_ascii_alphanumeric()))?), v["name"].as_str().unwrap_or("Voice").to_string())))
        .collect()
}

/// List the account's voices in the background when a key is saved (at start), so narration
/// and the agent know them.
pub fn load_account_voices() {
    let Some(key) = saved_key() else { return };
    std::thread::spawn(move || {
        let _ = elevenlabs_voices(&key);
    });
}

/// ElevenLabs' model for the user's own voices (the one ScreenForge's voices are read with).
const ACCOUNT_MODEL: &str = "eleven_v4";

/// Read `text` with account voice `voice` through ElevenLabs, with the user's own key (their
/// ElevenLabs credits, nothing through ScreenForge).
pub fn speak_account_voice(voice: &str, text: &str, speed: f64) -> Result<sf_tts::Speech, String> {
    if !is_account_voice(voice) {
        return Err(format!("unknown voice {voice}"));
    }
    let key = saved_key().ok_or("Connect your ElevenLabs under Narration → My voices → Add a voice → From ElevenLabs to read with your ElevenLabs voices.")?;
    let id = &voice[ACCOUNT.len()..];
    let body = serde_json::json!({ "text": text, "model_id": ACCOUNT_MODEL, "voice_settings": { "speed": speed.clamp(0.7, 1.2) } });
    let reply: serde_json::Value = ureq::post(&format!("{ELEVENLABS}/text-to-speech/{id}/with-timestamps?output_format=pcm_24000"))
        .set("xi-api-key", &key)
        .timeout(std::time::Duration::from_secs(180))
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(401 | 403, r) => {
                let detail = r.into_string().unwrap_or_default();
                if detail.contains("voice_access_denied") || detail.contains("verification") {
                    "ElevenLabs will not read with this voice yet: it needs verifying in your ElevenLabs (open it under Voices there).".to_string()
                } else {
                    "ElevenLabs did not accept your key for reading: edit it in ElevenLabs → Developers → API Keys and set Text to Speech to Access.".to_string()
                }
            }
            ureq::Error::Status(402, _) => "Your ElevenLabs account is out of credits for this reading.".to_string(),
            other => elevenlabs_error(other),
        })?
        .into_json()
        .map_err(|e| format!("ElevenLabs sent something unexpected: {e}"))?;
    use base64::Engine;
    let pcm = base64::engine::general_purpose::STANDARD.decode(reply["audio_base64"].as_str().ok_or("ElevenLabs returned no audio")?).map_err(|e| e.to_string())?;
    sf_tts::cloud::speech_from_pcm(&pcm, &sf_tts::cloud::Alignment::from_json(&reply["alignment"])).map_err(|e| format!("{e:#}"))
}

/// The cloned voices with recordings in ElevenLabs' list of voices.
fn cloned_voices(reply: &serde_json::Value) -> Vec<ElevenLabsVoice> {
    reply["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| matches!(v["category"].as_str(), Some("cloned" | "professional")))
        .filter_map(|v| {
            let samples: Vec<(String, String)> = v["samples"].as_array().into_iter().flatten().filter_map(|s| Some((s["sample_id"].as_str()?.to_string(), s["file_name"].as_str().unwrap_or("sample.mp3").to_string()))).collect();
            (!samples.is_empty()).then(|| ElevenLabsVoice {
                id: v["voice_id"].as_str().unwrap_or_default().to_string(),
                name: v["name"].as_str().unwrap_or("Voice").to_string(),
                samples,
                preview_url: v["preview_url"].as_str().filter(|u| u.starts_with("https://")).map(String::from),
            })
        })
        .collect()
}

/// ElevenLabs' sample of a voice, decoded at [`SAMPLE_RATE`] (kept in the cache after the first
/// play).
fn voice_preview(id: &str, url: &str) -> Result<Vec<f32>, String> {
    let dir = recordings_dir().ok_or("no folder for previews")?.join("previews");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    let path = dir.join(format!("{safe}.mp3"));
    if !path.is_file() {
        let resp = ureq::get(url).timeout(std::time::Duration::from_secs(60)).call().map_err(elevenlabs_error)?;
        let mut bytes = vec![];
        resp.into_reader().take(20_000_000).read_to_end(&mut bytes).map_err(|e| format!("Could not download the preview: {e}"))?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    sf_media::audio::decode(&path, SAMPLE_RATE, 1).map(|c| c.samples.to_vec()).map_err(|e| format!("Could not play the preview: {e}"))
}

fn elevenlabs_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401 | 403, _) => "ElevenLabs did not accept that API key. In ElevenLabs → Developers → API Keys, edit the key and set Voices to Read (or turn off Restrict Key).".into(),
        ureq::Error::Status(code, _) => format!("ElevenLabs answered {code}. Try again in a moment."),
        ureq::Error::Transport(_) => "Could not reach ElevenLabs. Check your connection.".into(),
    }
}

/// Download the recordings `voice` was made from into the form's recordings folder.
fn import_samples(key: &str, voice: &ElevenLabsVoice) -> Result<Vec<Recorded>, String> {
    let dir = recordings_dir().ok_or("no folder for recordings")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for (i, (sample, file)) in voice.samples.iter().take(IMPORT_SAMPLES).enumerate() {
        let resp = ureq::get(&format!("{ELEVENLABS}/voices/{}/samples/{sample}/audio", voice.id)).set("xi-api-key", key).timeout(std::time::Duration::from_secs(120)).call().map_err(elevenlabs_error)?;
        let mut bytes = vec![];
        resp.into_reader().take(50_000_000).read_to_end(&mut bytes).map_err(|e| format!("Could not download a recording: {e}"))?;
        let ext = Path::new(file).extension().and_then(|e| e.to_str()).filter(|e| e.len() <= 4).unwrap_or("mp3");
        let path = dir.join(format!("elevenlabs-{}-{}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S"), i + 1));
        std::fs::write(&path, bytes).map_err(|e| format!("Could not save a recording: {e}"))?;
        let ms = sf_media::audio::decode(&path, 16_000, 1).map(|c| c.samples.len() as f64 / 16.0).map_err(|e| format!("Could not read a recording from ElevenLabs: {e}"))?;
        let n = voice.samples.len().min(IMPORT_SAMPLES);
        let label = if n > 1 { format!("{} {} · ElevenLabs", voice.name, i + 1) } else { format!("{} · ElevenLabs", voice.name) };
        out.push(Recorded { path, label: Some(label), ms, silent: false, clipped: false });
    }
    Ok(out)
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
    pub error: Option<String>,
    /// The voice the form makes again (`None`: a new voice).
    pub editing: Option<String>,
    /// The panel's form: the recordings picked (the project's voice-overs, or recordings made
    /// here), the consent box and the name.
    pub picked: Vec<PathBuf>,
    pub consent: bool,
    /// What to call the voice (empty: "My voice").
    pub name: String,
    /// The form's tab: 0 Record, 1 From ElevenLabs.
    pub tab: usize,
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
    /// The user's ElevenLabs: the API key being typed (saved once ElevenLabs accepts it), the
    /// account's cloned voices once listed, the voice whose preview plays.
    pub el_key: String,
    pub el_voices: Option<Vec<ElevenLabsVoice>>,
    pub el_error: Option<String>,
    importing: Option<Receiver<Imported>>,
    /// The voice whose recordings are being imported, or whose preview is loading.
    pub el_busy: Option<String>,
    el_player: Option<(String, crate::narration::Player)>,
    /// The saved key was listed once this run (not again on every frame after an error).
    el_listed: bool,
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

    /// List the cloned voices of the saved ElevenLabs account, once a run (the panel calls
    /// this whenever it shows the list).
    pub fn ensure_elevenlabs(&mut self) {
        if self.el_listed || self.importing.is_some() {
            return;
        }
        self.el_listed = true;
        if let Some(key) = saved_key() {
            self.start_listing(key);
        }
    }

    /// Check the typed key with ElevenLabs by listing its voices; it is saved when that works.
    pub fn connect_elevenlabs(&mut self) {
        let key = self.el_key.trim().to_string();
        if !key.is_empty() && self.importing.is_none() {
            self.start_listing(key);
        }
    }

    /// List the voices again.
    pub fn refresh_elevenlabs(&mut self) {
        self.el_listed = false;
        self.ensure_elevenlabs();
    }

    fn start_listing(&mut self, key: String) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = elevenlabs_voices(&key);
            let _ = tx.send(Imported::Voices(key, r));
        });
        self.importing = Some(rx);
        self.el_error = None;
    }

    /// Bring in the recordings `voice` was made from: the form opens with them ticked and the
    /// voice's name.
    pub fn import_elevenlabs(&mut self, voice: ElevenLabsVoice) {
        let Some(key) = saved_key() else { return };
        if self.importing.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.el_busy = Some(voice.id.clone());
        std::thread::spawn(move || {
            let r = import_samples(&key, &voice);
            let _ = tx.send(Imported::Samples(voice.name.clone(), r));
        });
        self.importing = Some(rx);
        self.el_error = None;
    }

    /// Play ElevenLabs' sample of `voice`, or stop it when it plays.
    pub fn toggle_elevenlabs_preview(&mut self, voice: &ElevenLabsVoice) {
        if self.el_player.take().is_some_and(|(id, _)| id == voice.id) {
            return;
        }
        let Some(url) = voice.preview_url.clone() else {
            self.el_error = Some(format!("ElevenLabs has no preview of {}.", voice.name));
            return;
        };
        if self.importing.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let id = voice.id.clone();
        self.el_busy = Some(id.clone());
        std::thread::spawn(move || {
            let r = voice_preview(&id, &url);
            let _ = tx.send(Imported::Preview(id, r));
        });
        self.importing = Some(rx);
        self.el_error = None;
    }

    /// The voice whose preview plays.
    pub fn elevenlabs_playing(&self) -> Option<&str> {
        self.el_player.as_ref().filter(|(_, p)| p.is_playing()).map(|(id, _)| id.as_str())
    }

    pub fn is_importing(&self) -> bool {
        self.importing.is_some()
    }

    /// Forget the saved key and the voices listed with it.
    pub fn forget_elevenlabs(&mut self) {
        forget_key();
        set_account_voices(vec![]);
        self.el_key.clear();
        self.el_voices = None;
        self.el_error = None;
        self.el_player = None;
    }

    /// The form closed: stop listening (a recording in progress is thrown away).
    pub fn close(&mut self) {
        self.open = false;
        self.editing = None;
        self.tab = 0;
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
        let (files, name, id) = (self.picked.clone(), clean_name(&self.name), self.editing.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = make(&files, name, id, &tx);
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
        if self.el_player.as_ref().is_some_and(|(_, p)| !p.is_playing()) {
            self.el_player = None;
        }
        if let Some(Ok(r)) = self.importing.as_ref().map(|rx| rx.try_recv()) {
            self.importing = None;
            self.el_busy = None;
            match r {
                Imported::Voices(key, Ok(v)) => {
                    // a key ElevenLabs accepted is kept for next time
                    if saved_key().as_deref() != Some(key.as_str()) {
                        if let Err(e) = save_key(&key) {
                            self.el_error = Some(format!("Could not save the key: {e}"));
                        }
                    }
                    self.el_key.clear();
                    self.el_voices = Some(v);
                }
                Imported::Voices(_, Err(e)) | Imported::Samples(_, Err(e)) | Imported::Preview(_, Err(e)) => self.el_error = Some(e),
                Imported::Samples(name, Ok(recs)) => {
                    self.picked.extend(recs.iter().map(|r| r.path.clone()));
                    self.recorded.extend(recs);
                    if self.name.trim().is_empty() {
                        self.name = name;
                    }
                    // back to the Record tab, where they are ticked
                    self.open = true;
                    self.tab = 0;
                }
                Imported::Preview(id, Ok(samples)) => match crate::narration::Player::play(samples) {
                    Ok(p) => self.el_player = Some((id, p)),
                    Err(e) => self.el_error = Some(format!("Could not play the preview: {e}")),
                },
            }
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
            Ok(_) => {
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
fn make(files: &[PathBuf], name: String, id: Option<String>, tx: &std::sync::mpsc::Sender<Making>) -> Result<String, String> {
    let reference = joined_voice(files)?;
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
fn joined_voice(files: &[PathBuf]) -> Result<Vec<f32>, String> {
    let max = (MAX_REFERENCE_S * SAMPLE_RATE as f64) as usize;
    let mut out: Vec<f32> = vec![];
    let mut voiced = 0usize;
    for f in files {
        if out.len() >= max {
            break;
        }
        let clip = sf_media::audio::decode(f, SAMPLE_RATE, 1).map_err(|e| format!("Could not read {}: {e}", f.display()))?;
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
    fn only_cloned_voices_with_recordings_are_offered() {
        let reply = serde_json::json!({ "voices": [
            { "voice_id": "a", "name": "Me", "category": "cloned", "samples": [{ "sample_id": "s1", "file_name": "take.m4a" }] },
            { "voice_id": "b", "name": "Eric", "category": "premade", "samples": [{ "sample_id": "s2", "file_name": "x.mp3" }] },
            { "voice_id": "c", "name": "Empty", "category": "cloned", "samples": null },
            { "voice_id": "d", "name": "Pro", "category": "professional", "samples": [{ "sample_id": "s3" }] }
        ]});
        let v = cloned_voices(&reply);
        assert_eq!(v.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Me", "Pro"]);
        assert_eq!(v[0].samples, [("s1".to_string(), "take.m4a".to_string())]);
        assert_eq!(v[1].samples[0].1, "sample.mp3");
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
    fn the_accounts_own_voices_are_offered_for_narration() {
        let reply = serde_json::json!({ "voices": [
            { "voice_id": "abc123", "name": "Ricky", "category": "cloned" },
            { "voice_id": "def456", "name": "Eric", "category": "premade" },
            { "voice_id": "ghi789", "name": "Designed", "category": "generated" },
            { "voice_id": "../x", "name": "Bad", "category": "cloned" }
        ]});
        let v = account_voices_in(&reply);
        assert_eq!(v, [("el_user:abc123".to_string(), "Ricky".to_string()), ("el_user:ghi789".to_string(), "Designed".to_string())]);
        assert!(is_account_voice("el_user:abc123") && !is_account_voice("el_user:") && !is_account_voice("el_user:a/b") && !is_account_voice("el_eric"));
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
