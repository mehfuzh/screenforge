//! On-device speech to text for captions, with whisper.cpp.
//!
//! The microphone track of each recording session is decoded to 16 kHz mono, run through a
//! Whisper model with per-word timestamps, and saved as
//! `transcripts/microphone-session-<n>.json` (see `sf_project::transcript`). Models are the
//! ggml files published with whisper.cpp; they are downloaded on first use into
//! `<data dir>/ScreenForge/models` (or `$SF_WHISPER_MODELS`).

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
const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
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

    pub fn file_name(self) -> String {
        format!("ggml-{}.bin", self.id())
    }

    pub fn path(self) -> Option<PathBuf> {
        models_dir().map(|d| d.join(self.file_name()))
    }

    pub fn is_downloaded(self) -> bool {
        self.path().is_some_and(|p| p.is_file())
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
pub fn ensure_model(model: Model, cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let path = model.path().context("no folder for speech models")?;
    if path.is_file() {
        return Ok(path);
    }
    let dir = path.parent().context("models folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let url = format!("{MODEL_BASE_URL}/{}", model.file_name());
    tracing::info!("downloading {url}");
    let resp = ureq::get(&url).call().with_context(|| format!("downloading {url}"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok());
    let part = path.with_extension("bin.part");
    let mut out = std::fs::File::create(&part)?;
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = std::fs::remove_file(&part);
            bail!("cancelled");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if done - last_report >= 1 << 20 {
            last_report = done;
            progress(Progress::Downloading { done, total });
        }
    }
    out.flush()?;
    drop(out);
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&part);
        bail!("model download was cut short ({done} bytes)");
    }
    std::fs::rename(&part, &path)?;
    progress(Progress::Downloading { done, total: Some(done) });
    Ok(path)
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

/// Transcribe every microphone session of a project and write its transcript files.
/// Downloads the model first if needed. Returns each file written with its word count.
pub fn transcribe_project(project: &Path, model: Model, opts: &Options, cancel: Arc<AtomicBool>, progress: impl Fn(Progress) + Send + Sync + 'static) -> Result<Vec<(PathBuf, usize)>> {
    let pkg = ProjectPackage::open(project)?;
    let rec = pkg.recording.as_ref().context("the project has no recording")?;
    let dir = pkg.recording_dir();
    let mic = rec.microphone().context("no microphone was recorded")?;
    let files: Vec<PathBuf> = mic.sessions.iter().filter_map(|s| s.media_path(&dir)).collect();
    if files.is_empty() {
        bail!("the microphone recording is missing");
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
    for (i, audio) in files.iter().enumerate() {
        progress(Progress::Transcribing { session: i, sessions, percent: 0 });
        let p = progress.clone();
        let items = transcribe_file(&ctx, audio, opts, cancel.clone(), move |percent| p(Progress::Transcribing { session: i, sessions, percent }))?;
        cancelled(&cancel)?;
        let n = items.len();
        written.push((TranscriptFile::microphone(i as u32, generator.clone(), items).save(&pkg.path)?, n));
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

    #[test]
    fn model_ids_round_trip() {
        for m in Model::ALL {
            assert_eq!(Model::from_id(m.id()), Some(m));
            assert!(m.file_name().starts_with("ggml-"));
        }
    }
}
