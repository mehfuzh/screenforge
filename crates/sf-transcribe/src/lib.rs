//! On-device speech to text for captions, with whisper.cpp.
//!
//! The microphone track of each recording session is decoded to 16 kHz mono, run through a
//! Whisper model with per-word timestamps, and saved as
//! `transcripts/microphone-session-<n>.json`; each voice-over file likewise as
//! `transcripts/voice-over-<file stem>.json` (see `sf_project::transcript`). Models are the
//! ggml files published with whisper.cpp, mirrored on the `models-v1` GitHub release; they are
//! downloaded on first use into `<data dir>/ScreenForge/models` (or `$SF_WHISPER_MODELS`),
//! from our release first and Hugging Face if that fails, and kept only if their SHA-256
//! matches.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use sf_project::schema::common::Extra;
use sf_project::transcript::{Generator, TranscriptFile, TranscriptItem};
use sf_project::ProjectPackage;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Environment variable overriding the models folder.
pub const MODELS_ENV: &str = "SF_WHISPER_MODELS";
/// Where models are downloaded from, in order: our own release, then whisper.cpp's upload.
const MODEL_SOURCES: [&str; 2] = ["https://github.com/mehfuzh/screenforge/releases/download/models-v1", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"];
const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// 142 MB, multilingual.
    Base,
    /// 466 MB, multilingual, more accurate.
    Small,
}

impl Model {
    pub const ALL: [Model; 2] = [Model::Base, Model::Small];

    /// Name stored in the transcript's generator (`"base"` / `"small"`, like Screen Studio).
    pub fn id(self) -> &'static str {
        match self {
            Model::Base => "base",
            Model::Small => "small",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Model::Base => "Base (142 MB)",
            Model::Small => "Small (466 MB)",
        }
    }

    pub fn from_id(id: &str) -> Option<Model> {
        Model::ALL.into_iter().find(|m| m.id() == id)
    }

    /// SHA-256 of the ggml file (the same on our release and on Hugging Face).
    pub fn sha256(self) -> &'static str {
        match self {
            Model::Base => "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            Model::Small => "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        }
    }

    /// File size in bytes.
    pub fn size(self) -> u64 {
        match self {
            Model::Base => 147_951_465,
            Model::Small => 487_601_967,
        }
    }

    /// Download URLs, preferred first.
    pub fn urls(self) -> Vec<String> {
        MODEL_SOURCES.iter().map(|base| format!("{base}/{}", self.file_name())).collect()
    }

    pub fn file_name(self) -> String {
        format!("ggml-{}.bin", self.id())
    }

    pub fn path(self) -> Option<PathBuf> {
        models_dir().map(|d| d.join(self.file_name()))
    }

    /// The model file is there with the right size (it was verified when downloaded).
    pub fn is_downloaded(self) -> bool {
        self.path().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.len() == self.size())
    }
}

pub fn models_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(MODELS_ENV) {
        return Some(PathBuf::from(p));
    }
    directories::ProjectDirs::from("", "", "ScreenForge").map(|d| d.data_dir().join("models"))
}

/// What a running job reports.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Downloading the model: bytes so far, total when known.
    Downloading { done: u64, total: Option<u64> },
    /// Transcribing session `session` of `sessions`, `percent` done.
    Transcribing { session: usize, sessions: usize, percent: u8 },
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(())
}

/// Make sure the model file exists, downloading it if needed. Returns its path.
pub fn ensure_model(model: Model, cancel: &AtomicBool, progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let path = model.path().context("no folder for speech models")?;
    if model.is_downloaded() {
        return Ok(path);
    }
    let dir = path.parent().context("models folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    download_verified(&model.urls(), model.sha256(), &path, cancel, progress)?;
    Ok(path)
}

/// Download `dest` from the first of `urls` that delivers a file with SHA-256 `sha256`. The
/// file is written next to `dest` as `.part` and renamed only once verified.
pub fn download_verified(urls: &[String], sha256: &str, dest: &Path, cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<()> {
    let part = dest.with_extension("part");
    let mut failures = vec![];
    for url in urls {
        tracing::info!("downloading {url}");
        match download_one(url, &part, cancel, &mut progress) {
            Ok(hash) if hash.eq_ignore_ascii_case(sha256) => {
                std::fs::rename(&part, dest)?;
                return Ok(());
            }
            Ok(hash) => failures.push(format!("{url}: checksum {hash} does not match")),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                cancelled(cancel)?;
                failures.push(format!("{url}: {e:#}"));
            }
        }
        let _ = std::fs::remove_file(&part);
        tracing::warn!("{}", failures.last().map(String::as_str).unwrap_or_default());
    }
    bail!("could not download the speech model:\n{}", failures.join("\n"))
}

/// Stream `url` into `part`, returning the hex SHA-256 of what arrived.
fn download_one(url: &str, part: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(Progress)) -> Result<String> {
    use sha2::{Digest, Sha256};
    let resp = ureq::get(url).call()?;
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok());
    let mut out = std::fs::File::create(part)?;
    let mut reader = resp.into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut last_report) = (0u64, 0u64);
    loop {
        cancelled(cancel)?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if done - last_report >= 1 << 20 {
            last_report = done;
            progress(Progress::Downloading { done, total });
        }
    }
    out.flush()?;
    if total.is_some_and(|t| t != done) {
        bail!("cut short after {done} bytes");
    }
    progress(Progress::Downloading { done, total: Some(done) });
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// ISO-639-1 code, or `None` to detect the language.
    pub language: Option<String>,
    /// Words the model should expect (names, jargon).
    pub prompt: Option<String>,
}

/// `[music]`, `(laughs)` and the like.
fn sound_label(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')')) || (t.starts_with('*') && t.ends_with('*') && t.len() > 1)
}

/// Transcribe mono 16 kHz samples into word items (ms into the samples).
pub fn transcribe_samples(ctx: &WhisperContext, samples: &[f32], opts: &Options, cancel: Arc<AtomicBool>, progress: impl FnMut(u8) + 'static) -> Result<Vec<TranscriptItem>> {
    let mut state = ctx.create_state().context("creating the speech model state")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(opts.language.as_deref().unwrap_or("auto")));
    if let Some(p) = opts.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
        params.set_initial_prompt(p);
    }
    params.set_n_threads(std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4) as i32);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // one segment per word, with token-level timing
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    let mut progress = progress;
    params.set_progress_callback_safe(move |p: i32| progress(p.clamp(0, 100) as u8));
    // whisper-rs 0.16's `set_abort_callback_safe` calls the boxed closure through the wrong
    // type (the callback reads garbage and aborts the encoder), so pass the flag directly.
    unsafe extern "C" fn abort_if_set(flag: *mut std::ffi::c_void) -> bool {
        // SAFETY: `flag` points at the `AtomicBool` in `cancel`, alive until `full` returns.
        unsafe { (*(flag as *const AtomicBool)).load(Ordering::Relaxed) }
    }
    // SAFETY: the callback only reads the flag, and `cancel` outlives `state.full` below.
    unsafe {
        params.set_abort_callback(Some(abort_if_set));
        params.set_abort_callback_user_data(Arc::as_ptr(&cancel) as *mut std::ffi::c_void);
    }
    state.full(params, samples).context("running the speech model")?;

    let mut items = vec![];
    for seg in state.as_iter() {
        let text = seg.to_str_lossy().map(|s| s.trim().to_string()).unwrap_or_default();
        if text.is_empty() || text == "[BLANK_AUDIO]" {
            continue;
        }
        let start = seg.start_timestamp() as f64 * 10.0;
        let end = (seg.end_timestamp() as f64 * 10.0).max(start);
        let mut item = TranscriptItem::new(items.len() as u32, start, end, text.clone());
        if sound_label(&text) {
            item.kind = Some("sound-label".into());
        }
        items.push(item);
    }
    Ok(items)
}

pub fn load_model(path: &Path) -> Result<WhisperContext> {
    let p = path.to_str().context("model path is not UTF-8")?;
    WhisperContext::new_with_params(p, WhisperContextParameters::default()).with_context(|| format!("loading {}", path.display()))
}

/// Decode an audio (or video) file to 16 kHz mono and transcribe it.
pub fn transcribe_file(ctx: &WhisperContext, audio: &Path, opts: &Options, cancel: Arc<AtomicBool>, progress: impl FnMut(u8) + 'static) -> Result<Vec<TranscriptItem>> {
    let clip = sf_media::decode_audio(audio, SAMPLE_RATE, 1).with_context(|| format!("decoding {}", audio.display()))?;
    transcribe_samples(ctx, &clip.samples, opts, cancel, progress)
}

/// What one transcript is made from.
enum Part {
    Microphone(u32, PathBuf),
    /// A voice-over file (by name in `voiceovers/`) and its path.
    VoiceOver(String, PathBuf),
}

/// Transcribe every microphone session and every voice-over file of a project and write
/// their transcript files. Downloads the model first if needed. Returns each file written
/// with its word count.
pub fn transcribe_project(project: &Path, model: Model, opts: &Options, cancel: Arc<AtomicBool>, progress: impl Fn(Progress) + Send + Sync + 'static) -> Result<Vec<(PathBuf, usize)>> {
    let pkg = ProjectPackage::open(project)?;
    let mut files: Vec<Part> = vec![];
    let mic = pkg.recording.as_ref().and_then(|r| r.microphone());
    if let Some(mic) = mic {
        let dir = pkg.recording_dir();
        files.extend(mic.sessions.iter().enumerate().filter_map(|(i, s)| Some(Part::Microphone(i as u32, s.media_path(&dir)?))));
    }
    // takes split from one recording share its file: transcribed once; an AI voice's takes
    // were written with their words
    let vo_dir = pkg.path.join(sf_project::v4::VoiceOver::DIR);
    let mut names: Vec<&str> = pkg.project.scenes.iter().flat_map(|s| &s.voice_overs).filter(|v| v.spoken_voice().is_none()).map(|v| v.file_name.as_str()).filter(|n| !n.is_empty()).collect();
    names.sort_unstable();
    names.dedup();
    files.extend(names.into_iter().map(|n| (n, vo_dir.join(n))).filter(|(_, p)| p.is_file()).map(|(n, p)| Part::VoiceOver(n.to_string(), p)));
    if files.is_empty() {
        if mic.is_some() {
            bail!("the microphone recording is missing");
        }
        // only AI narration: its takes were written with their words, nothing is left to do
        if pkg.project.scenes.iter().flat_map(|s| &s.voice_overs).any(|v| v.spoken_voice().is_some()) {
            return Ok(vec![]);
        }
        bail!("nothing to transcribe: no microphone was recorded and there are no voice-overs");
    }
    let progress = Arc::new(progress);
    let model_path = {
        let p = progress.clone();
        ensure_model(model, &cancel, move |x| p(x))?
    };
    cancelled(&cancel)?;
    let ctx = load_model(&model_path)?;
    let generator = Generator { kind: "whisper".into(), model: Some(model.id().into()), language: opts.language.clone(), prompt: opts.prompt.clone(), extra: Extra::new() };
    let mut written = vec![];
    let sessions = files.len();
    for (i, part) in files.iter().enumerate() {
        progress(Progress::Transcribing { session: i, sessions, percent: 0 });
        let p = progress.clone();
        let audio = match part {
            Part::Microphone(_, a) | Part::VoiceOver(_, a) => a,
        };
        let items = transcribe_file(&ctx, audio, opts, cancel.clone(), move |percent| p(Progress::Transcribing { session: i, sessions, percent }))?;
        cancelled(&cancel)?;
        let n = items.len();
        let file = match part {
            Part::Microphone(index, _) => TranscriptFile::microphone(*index, generator.clone(), items),
            Part::VoiceOver(name, _) => TranscriptFile::voice_over(name, generator.clone(), items),
        };
        written.push((file.save(&pkg.path)?, n));
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_sound_labels() {
        assert!(sound_label("[MUSIC]"));
        assert!(sound_label(" (laughs) "));
        assert!(!sound_label("hello"));
        assert!(!sound_label("[unbalanced"));
    }

    /// Runs when the base model is already downloaded (e.g. after the e2e speech test).
    #[test]
    fn cancel_stops_transcription() {
        let Some(path) = Model::Base.path().filter(|p| p.is_file()) else {
            eprintln!("base model not downloaded; skipping");
            return;
        };
        let ctx = load_model(&path).unwrap();
        let samples = vec![0.0f32; SAMPLE_RATE as usize * 5];
        let err = transcribe_samples(&ctx, &samples, &Options::default(), Arc::new(AtomicBool::new(true)), |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("speech model"), "{err:#}");
        // and silence transcribes to nothing when not cancelled
        let items = transcribe_samples(&ctx, &samples, &Options { language: Some("en".into()), prompt: None }, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(items.iter().all(|i| i.is_sound_label()), "{items:?}");
    }

    /// Serve `/good` (the payload), `/bad` (other bytes) and 404 for anything else, on a
    /// local port; returns the base URL.
    fn serve(payload: &'static [u8]) -> String {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut line);
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let body: &[u8] = match path.as_str() {
                    "/good" => payload,
                    "/bad" => b"not the model",
                    _ => b"",
                };
                let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
                let mut s = &stream;
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(body);
            }
        });
        base
    }

    fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_bad_or_missing_source_falls_back_to_the_next() {
        const PAYLOAD: &[u8] = b"ggml model bytes";
        let base = serve(PAYLOAD);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ggml-test.bin");
        let never = AtomicBool::new(false);
        let urls = vec![format!("{base}/missing"), format!("{base}/bad"), format!("{base}/good")];
        download_verified(&urls, &sha256_hex(PAYLOAD), &dest, &never, |_| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), PAYLOAD);
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn nothing_is_kept_when_every_source_fails() {
        let base = serve(b"x");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ggml-test.bin");
        let never = AtomicBool::new(false);
        let err = download_verified(&[format!("{base}/bad"), format!("{base}/missing")], &sha256_hex(b"the real model"), &dest, &never, |_| {}).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("checksum") && msg.contains("404"), "{msg}");
        assert!(!dest.exists() && !dest.with_extension("part").exists());
    }

    #[test]
    fn cancel_stops_without_trying_the_fallback() {
        const PAYLOAD: &[u8] = b"ggml";
        let base = serve(PAYLOAD);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ggml-test.bin");
        let cancel = AtomicBool::new(true);
        let err = download_verified(&[format!("{base}/good"), format!("{base}/good")], &sha256_hex(PAYLOAD), &dest, &cancel, |_| {}).unwrap_err();
        assert_eq!(err.to_string(), "cancelled");
        assert!(!dest.exists());
    }

    #[test]
    fn our_release_comes_first() {
        for m in Model::ALL {
            let urls = m.urls();
            assert!(urls[0].starts_with("https://github.com/mehfuzh/screenforge/releases/download/models-v1/"), "{urls:?}");
            assert!(urls[1].contains("huggingface.co"));
            assert_eq!(m.sha256().len(), 64);
        }
    }

    #[test]
    fn model_ids_round_trip() {
        for m in Model::ALL {
            assert_eq!(Model::from_id(m.id()), Some(m));
            assert!(m.file_name().starts_with("ggml-"));
        }
    }
}
