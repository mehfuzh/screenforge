//! On-device text to speech for AI voice-overs, with Kokoro-82M (Apache-2.0) run by ONNX
//! Runtime.
//!
//! Text is read sentence by sentence: each sentence is turned into phonemes with misaki's
//! dictionaries (see [`g2p`]), synthesised at 24 kHz with a voice's style vector, and its
//! words are spread over the speech by how long each one's phonemes are, for captions. The
//! model, the dictionaries and each voice are downloaded on first use into
//! `<models dir>/kokoro-v1.0` (see `sf_transcribe::models_dir`), from our `models-v1` release
//! first and where they are published if that fails, and kept only if their
//! SHA-256 matches.

pub mod g2p;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
pub use sf_transcribe::Progress;

pub const SAMPLE_RATE: u32 = 24_000;
/// The voice used when none is chosen.
pub const DEFAULT_VOICE: &str = "am_fenrir";

/// Pause between sentences, and after a paragraph (a blank line) (s).
const SENTENCE_GAP: f64 = 0.22;
const PARAGRAPH_GAP: f64 = 0.45;
/// Kokoro reads at most 510 phonemes at once; sentences longer than this are split.
const MAX_PHONEMES: usize = 400;
/// Peak level of a finished take.
const PEAK: f32 = 0.89;

/// Our mirror (the `models-v1` release, next to the Whisper models), tried first; each file
/// is there as `kokoro-<file>` with `/` as `-`.
const MIRROR: &str = "https://github.com/mehfuzh/screenforge/releases/download/models-v1";
const MODEL_REPO: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/1939ad2a8e416c0acfeecc08a694d14ef25f2231";
const DICT_REPO: &str = "https://raw.githubusercontent.com/hexgrad/misaki/fba1236595f2d2bf21d414ba6e57d25256afada3/misaki/data";

/// A downloadable file: where it goes under the Kokoro folder, where it comes from (besides
/// the mirror), its hash and size.
struct Asset {
    file: String,
    url: String,
    sha256: &'static str,
    size: u64,
}

impl Asset {
    /// Its name in the mirror: `kokoro-model.onnx`, `kokoro-voices-am_fenrir.bin`.
    fn mirror_name(&self) -> String {
        format!("kokoro-{}", self.file.replace('/', "-"))
    }

    /// The mirror first, then where it is published.
    fn urls(&self) -> Vec<String> {
        vec![format!("{MIRROR}/{}", self.mirror_name()), self.url.clone()]
    }
}

/// Bytes of a voice file (510 style vectors of 256 floats).
const VOICE_BYTES: u64 = 510 * 256 * 4;

fn model_assets() -> [Asset; 3] {
    [
        Asset { file: "model.onnx".into(), url: format!("{MODEL_REPO}/onnx/model.onnx"), sha256: "8fbea51ea711f2af382e88c833d9e288c6dc82ce5e98421ea61c058ce21a34cb", size: 325_532_232 },
        Asset { file: "us_gold.json".into(), url: format!("{DICT_REPO}/us_gold.json"), sha256: "dc414872a49a28ae6c141463d502fd945f3b2fde040484fdc47d00cc4612686f", size: 3_000_469 },
        Asset { file: "us_silver.json".into(), url: format!("{DICT_REPO}/us_silver.json"), sha256: "de8f67be911bb6c659187b4a65fd966b6a30e56350e0f790d763210b053ac475", size: 3_099_517 },
    ]
}

/// Everything [`ensure`] downloads: the model, the dictionaries and every voice.
fn all_assets() -> Vec<Asset> {
    let mut v: Vec<Asset> = model_assets().into();
    v.extend(VOICES.iter().map(Voice::asset));
    v
}

/// Size of the whole download in bytes (for "about 330 MB").
pub fn download_size() -> u64 {
    all_assets().iter().map(|a| a.size).sum()
}

/// A voice the editor offers (American English, which the dictionaries are for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    pub id: &'static str,
    pub name: &'static str,
    pub female: bool,
    sha256: &'static str,
}

impl Voice {
    /// "Fenrir (male)".
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, if self.female { "female" } else { "male" })
    }

    fn asset(&self) -> Asset {
        Asset { file: format!("voices/{}.bin", self.id), url: format!("{MODEL_REPO}/voices/{}.bin", self.id), sha256: self.sha256, size: VOICE_BYTES }
    }
}

pub const VOICES: [Voice; 6] = [
    Voice { id: "am_fenrir", name: "Fenrir", female: false, sha256: "c27989f741f7ee34d273a39d8a595cc0837d35f5ced9a29b7cc162614616df43" },
    Voice { id: "am_michael", name: "Michael", female: false, sha256: "1d1f21dd8da39c30705cd4c75d039d265e9bc4a2a93ed09bc9e1b1225eb95ba1" },
    Voice { id: "am_puck", name: "Puck", female: false, sha256: "fcf73c989033e9233e0b98713eca600c8c74dcc1614b37009d5450ff4a2274a0" },
    Voice { id: "af_heart", name: "Heart", female: true, sha256: "d583ccff3cdca2f7fae535cb998ac07e9fcb90f09737b9a41fa2734ec44a8f0b" },
    Voice { id: "af_bella", name: "Bella", female: true, sha256: "f69d836209b78eb8c66e75e3cda491e26ea838a3674257e9d4e5703cbaf55c8b" },
    Voice { id: "af_nicole", name: "Nicole", female: true, sha256: "cd2191ab31b914ed7b318416b0e4440fdf392ddad9106a060819aa600a64f59a" },
];

pub fn voice(id: &str) -> Option<&'static Voice> {
    VOICES.iter().find(|v| v.id == id)
}

/// Where the model, dictionaries and voices are kept.
pub fn kokoro_dir() -> Option<PathBuf> {
    sf_transcribe::models_dir().map(|d| d.join("kokoro-v1.0"))
}

fn voice_path(dir: &Path, voice: &Voice) -> PathBuf {
    dir.join("voices").join(format!("{}.bin", voice.id))
}

/// The model, the dictionaries and every voice are on disk (nothing to download).
pub fn is_downloaded() -> bool {
    kokoro_dir().is_some_and(|d| all_assets().iter().all(|a| d.join(&a.file).is_file()))
}

/// Download whatever of the model, the dictionaries and the voices is missing, reporting
/// progress over the whole download. Returns the Kokoro folder.
pub fn ensure(cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<PathBuf> {
    // the panel and the agent can both ask: one downloads, the other then finds the files
    static DOWNLOADING: Mutex<()> = Mutex::new(());
    let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = kokoro_dir().context("no folder for speech models")?;
    std::fs::create_dir_all(dir.join("voices")).with_context(|| format!("creating {}", dir.display()))?;
    let assets = all_assets();
    let total: u64 = assets.iter().map(|a| a.size).sum();
    let mut before: u64 = assets.iter().filter(|a| dir.join(&a.file).is_file()).map(|a| a.size).sum();
    for asset in assets {
        let path = dir.join(&asset.file);
        if path.is_file() {
            continue;
        }
        sf_transcribe::download_verified(&asset.urls(), asset.sha256, &path, cancel, |p| {
            if let Progress::Downloading { done, .. } = p {
                progress(Progress::Downloading { done: before + done.min(asset.size), total: Some(total) });
            }
        })?;
        before += asset.size;
    }
    Ok(dir)
}

/// Delete the downloaded model and voices (they are downloaded again when next needed).
pub fn remove_download() -> Result<()> {
    let Some(dir) = kokoro_dir().filter(|d| d.exists()) else { return Ok(()) };
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}

/// A word of a [`Speech`], in ms from its start.
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenWord {
    pub start_ms: f64,
    pub end_ms: f64,
    pub text: String,
}

/// Synthesised speech: mono samples at [`SAMPLE_RATE`] and where each word is.
#[derive(Debug, Clone, Default)]
pub struct Speech {
    pub samples: Vec<f32>,
    pub words: Vec<SpokenWord>,
}

impl Speech {
    pub fn duration_ms(&self) -> f64 {
        self.samples.len() as f64 * 1000.0 / SAMPLE_RATE as f64
    }

    /// Write as a 16-bit PCM mono WAV.
    pub fn write_wav(&self, path: &Path) -> Result<()> {
        let data_len = (self.samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        let mut f = std::fs::File::create(path).with_context(|| format!("could not write {}", path.display()))?;
        f.write_all(&out)?;
        Ok(())
    }
}

/// The loaded model and dictionaries.
pub struct Kokoro {
    session: Mutex<ort::session::Session>,
    dict: g2p::Dict,
    vocab: std::collections::HashMap<char, i64>,
    dir: PathBuf,
}

/// Kokoro's phoneme ids (from the model's `config.json`).
const VOCAB: &str = include_str!("vocab.json");

impl Kokoro {
    /// Load the model and dictionaries from `dir` (see [`ensure`]).
    pub fn load(dir: &Path) -> Result<Kokoro> {
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).with_context(|| format!("reading {f}"));
        let dict = g2p::Dict::from_json(&read("us_gold.json")?, &read("us_silver.json")?)?;
        let vocab: serde_json::Map<String, serde_json::Value> = serde_json::from_str(VOCAB)?;
        let vocab = vocab.into_iter().filter_map(|(k, v)| Some((k.chars().next()?, v.as_i64()?))).collect();
        let env = ort::init().with_name("screenforge-tts").build().map_err(|e| anyhow::anyhow!("starting ONNX Runtime: {e}"))?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        let session = ort::session::Session::builder(&env)
            .and_then(|b| b.with_intra_threads(threads).map_err(Into::into))
            .and_then(|mut b| b.commit_from_file(dir.join("model.onnx")))
            .map_err(|e| anyhow::anyhow!("loading the voice model: {e}"))?;
        Ok(Kokoro { session: Mutex::new(session), dict, vocab, dir: dir.to_path_buf() })
    }

    /// The model loaded once per run (it takes a second and some 300 MB).
    pub fn shared(dir: &Path) -> Result<Arc<Kokoro>> {
        static SHARED: Mutex<Option<Arc<Kokoro>>> = Mutex::new(None);
        let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = s.as_ref().filter(|k| k.dir == dir) {
            return Ok(k.clone());
        }
        let k = Arc::new(Kokoro::load(dir)?);
        *s = Some(k.clone());
        Ok(k)
    }

    /// Read `text` aloud with `voice` at `speed` (1 = normal). Blank lines are longer pauses.
    pub fn speak(&self, text: &str, voice: &Voice, speed: f32) -> Result<Speech> {
        let styles = read_styles(&voice_path(&self.dir, voice))?;
        let mut speech = Speech::default();
        let gap = |s: f64| vec![0f32; (s * SAMPLE_RATE as f64) as usize];
        for (pi, paragraph) in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).enumerate() {
            if pi > 0 {
                speech.samples.extend(gap(PARAGRAPH_GAP - SENTENCE_GAP));
            }
            for sentence in sentences(paragraph) {
                for phrase in split_long(self.dict.phrase(&sentence)) {
                    if phrase.is_empty() {
                        continue;
                    }
                    let audio = self.synthesize(&phrase.phonemes, &styles, speed)?;
                    let (audio, voiced) = trim(audio);
                    let at = speech.duration_ms();
                    speech.words.extend(time_words(&phrase, voiced).into_iter().map(|w| SpokenWord { start_ms: at + w.start_ms, end_ms: at + w.end_ms, text: w.text }));
                    speech.samples.extend(audio);
                    speech.samples.extend(gap(SENTENCE_GAP));
                }
            }
        }
        if speech.words.is_empty() {
            bail!("there is nothing to say in the text");
        }
        // the last pause is not part of the take
        speech.samples.truncate(speech.samples.len().saturating_sub(gap(SENTENCE_GAP).len()));
        let peak = speech.samples.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
        speech.samples.iter_mut().for_each(|v| *v *= PEAK / peak);
        Ok(speech)
    }

    fn synthesize(&self, phonemes: &str, styles: &[f32], speed: f32) -> Result<Vec<f32>> {
        let mut ids: Vec<i64> = vec![0];
        ids.extend(phonemes.chars().filter_map(|c| self.vocab.get(&c).copied()));
        ids.push(0);
        // a style vector per phoneme count
        let rows = styles.len() / 256;
        let row = (ids.len() - 2).saturating_sub(1).min(rows.saturating_sub(1));
        let style = styles[row * 256..(row + 1) * 256].to_vec();
        let n = ids.len();
        let inputs = ort::inputs![
            "input_ids" => ort::value::Tensor::from_array(([1usize, n], ids))?,
            "style" => ort::value::Tensor::from_array(([1usize, 256], style))?,
            "speed" => ort::value::Tensor::from_array(([1usize], vec![speed]))?,
        ];
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let out = session.run(inputs)?;
        let (_, wave) = out["waveform"].try_extract_tensor::<f32>()?;
        Ok(wave.to_vec())
    }
}

/// A voice file: 510 style vectors of 256 floats (little-endian f32).
fn read_styles(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() % (256 * 4) != 0 || bytes.is_empty() {
        bail!("{} is not a voice file", path.display());
    }
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Sentences of a paragraph, each with its closing punctuation.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(if c == '\n' { ' ' } else { c });
        // "1.5" and "e.g." are not ends; a stop followed by a space or the end is
        if matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|n| n.is_whitespace() || *n == '"' || *n == '”') {
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// A phrase too long for the model, cut between words (at a comma when there is one).
fn split_long(p: g2p::Phrase) -> Vec<g2p::Phrase> {
    if p.len() <= MAX_PHONEMES || p.words.len() < 2 {
        return vec![p];
    }
    let chars: Vec<char> = p.phonemes.chars().collect();
    let half = p.len() / 2;
    // the word end nearest the middle, preferring one before a comma
    let cut = p.words[..p.words.len() - 1]
        .iter()
        .enumerate()
        .min_by_key(|(_, (c, r))| (r.end as i64 - half as i64).unsigned_abs() / if c.ends_with(',') { 4 } else { 1 })
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = p.words[cut + 1].1.start;
    let left_words: Vec<_> = p.words[..=cut].to_vec();
    let right_words: Vec<_> = p.words[cut + 1..].iter().map(|(c, r)| (c.clone(), r.start - end..r.end - end)).collect();
    let left = g2p::Phrase { phonemes: chars[..end].iter().collect::<String>().trim_end().to_string(), words: left_words };
    let right = g2p::Phrase { phonemes: chars[end..].iter().collect(), words: right_words };
    let mut out = split_long(left);
    out.extend(split_long(right));
    out
}

/// Cut the silence around a sentence (keeping a little), returning the audio and the voiced
/// part of it in ms.
fn trim(audio: Vec<f32>) -> (Vec<f32>, (f64, f64)) {
    const QUIET: f32 = 1e-3;
    let keep = |ms: f64| (ms * SAMPLE_RATE as f64 / 1000.0) as usize;
    let first = audio.iter().position(|v| v.abs() > QUIET).unwrap_or(0);
    let last = audio.iter().rposition(|v| v.abs() > QUIET).unwrap_or(audio.len().saturating_sub(1));
    let start = first.saturating_sub(keep(30.0));
    let end = (last + keep(60.0)).min(audio.len());
    let ms = |i: usize| (i - start) as f64 * 1000.0 / SAMPLE_RATE as f64;
    let voiced = (ms(first), ms(last.max(first)));
    (audio[start..end.max(start)].to_vec(), voiced)
}

/// Spread a phrase's words over its voiced span by the length of their phonemes (the gaps
/// between words, and commas, take time too).
fn time_words(p: &g2p::Phrase, (from, to): (f64, f64)) -> Vec<SpokenWord> {
    let total = p.len().max(1) as f64;
    let at = |i: usize| from + (to - from) * i as f64 / total;
    p.words.iter().map(|(text, r)| SpokenWord { start_ms: at(r.start), end_ms: at(r.end), text: text.clone() }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mirror_comes_first_with_flat_names() {
        let names: Vec<String> = all_assets().iter().map(Asset::mirror_name).collect();
        assert!(names.contains(&"kokoro-model.onnx".to_string()) && names.contains(&"kokoro-voices-af_heart.bin".to_string()), "{names:?}");
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
        let urls = model_assets()[0].urls();
        assert!(urls[0].starts_with(MIRROR) && urls[1].contains("huggingface.co"), "{urls:?}");
    }

    #[test]
    fn sentences_end_at_stops_not_inside_numbers() {
        assert_eq!(sentences("It runs at 1.5x. Really? Yes!\nDone"), ["It runs at 1.5x.", "Really?", "Yes!", "Done"]);
    }

    #[test]
    fn words_are_spread_over_the_voiced_part() {
        let p = g2p::Phrase { phonemes: "ab cd".into(), words: vec![("Ab".into(), 0..2), ("cd.".into(), 3..5)] };
        let w = time_words(&p, (100.0, 600.0));
        assert_eq!(w[0], SpokenWord { start_ms: 100.0, end_ms: 300.0, text: "Ab".into() });
        assert_eq!(w[1], SpokenWord { start_ms: 400.0, end_ms: 600.0, text: "cd.".into() });
    }

    #[test]
    fn long_phrases_are_split_between_words() {
        let word = "ab".repeat(50);
        let mut phonemes = String::new();
        let mut words = vec![];
        for i in 0..10 {
            if i > 0 {
                phonemes.push(' ');
            }
            let s = phonemes.chars().count();
            phonemes.push_str(&word);
            words.push((format!("w{i}"), s..s + word.len()));
        }
        let parts = split_long(g2p::Phrase { phonemes, words });
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|p| p.len() <= MAX_PHONEMES));
        assert_eq!(parts.iter().map(|p| p.words.len()).sum::<usize>(), 10);
        for p in &parts {
            let chars: Vec<char> = p.phonemes.chars().collect();
            assert!(p.words.iter().all(|(_, r)| chars[r.clone()].iter().collect::<String>() == word), "{p:?}");
        }
    }

    #[test]
    fn trim_keeps_a_little_around_the_voice() {
        let mut a = vec![0f32; 24_000];
        a[12_000..13_000].iter_mut().for_each(|v| *v = 0.5);
        let (out, (from, to)) = trim(a);
        assert_eq!(out.len(), 720 + 1000 + 1440 - 1);
        assert!((from - 30.0).abs() < 0.1 && (to - (30.0 + 999.0 / 24.0)).abs() < 0.1, "{from} {to}");
    }

    /// Runs the real model when it is downloaded (`cargo test -p sf-tts -- --ignored`).
    #[test]
    #[ignore]
    fn speaks_with_the_downloaded_model() {
        let v = voice(&std::env::var("SF_TTS_VOICE").unwrap_or_else(|_| DEFAULT_VOICE.into())).unwrap();
        let dir = ensure(&AtomicBool::new(false), |_| {}).unwrap();
        let k = Kokoro::load(&dir).unwrap();
        let s = k.speak("Welcome to ScreenForge. It edits videos 2× faster!", v, 1.0).unwrap();
        assert!(s.duration_ms() > 1500.0 && s.duration_ms() < 8000.0, "{}", s.duration_ms());
        let words: Vec<&str> = s.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(words, ["Welcome", "to", "ScreenForge.", "It", "edits", "videos", "2×", "faster!"]);
        assert!(s.words.windows(2).all(|w| w[0].end_ms <= w[1].start_ms + 1.0));
        assert!(s.words.last().unwrap().end_ms <= s.duration_ms());
        if let Ok(out) = std::env::var("SF_TTS_SAMPLE") {
            s.write_wav(Path::new(&out)).unwrap();
        }
    }
}
