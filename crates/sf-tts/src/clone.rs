//! "My voice" on this computer: Chatterbox Turbo (Resemble AI, MIT), run by ONNX Runtime, reads
//! text in the voice of a short recording of the user, with nothing uploaded.
//!
//! The model's four parts (4-bit, about 0.7 GB) are downloaded on first use into
//! `<models dir>/chatterbox-turbo-q4` from a pinned Hugging Face revision and kept only if
//! their SHA-256 matches. Reading follows the model card: the recording is encoded once into
//! the speaker's conditioning; each sentence's text tokens ([`crate::bpe`]) then lead a greedy
//! loop of speech tokens through the language model (with its key/value cache), and the
//! decoder turns those tokens into 24 kHz audio. Words are spread over each sentence's voiced
//! part by their length, for captions.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::{sentences, trim, Progress, Speech, SpokenWord, PEAK, SAMPLE_RATE};

/// The voice id of the user's own voices made on this computer: `my_voice` alone means "their
/// voice" (the one chosen, or their first), `my_voice:<id>` one of them.
pub const MY_VOICE: &str = "my_voice";

/// `id` is one of the user's own voices ([`MY_VOICE`]).
pub fn is_my_voice(id: &str) -> bool {
    id.strip_prefix(MY_VOICE).is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

const REPO: &str = "https://huggingface.co/ResembleAI/chatterbox-turbo-ONNX/resolve/d21799bd0354adb85e348b8a0442a8405110a2cf";

/// Least and most of the recording the voice is made from (s): under 5 s the model loses
/// track (long silences); 15 s is plenty.
pub const MIN_REFERENCE_S: f64 = 5.0;
pub const MAX_REFERENCE_S: f64 = 15.0;

const START_SPEECH: i64 = 6561;
const STOP_SPEECH: i64 = 6562;
const SILENCE: i64 = 4299;
const KV_HEADS: usize = 16;
const HEAD_DIM: usize = 64;
const REPETITION_PENALTY: f32 = 1.2;
/// Longest piece of text read at once (characters); longer sentences are cut between words.
const MAX_CHARS: usize = 300;
/// Speech tokens a character of text takes, about (for progress only).
const TOKENS_PER_CHAR: f32 = 1.5;
const SENTENCE_GAP: f64 = 0.22;
const PARAGRAPH_GAP: f64 = 0.45;

struct Asset {
    file: &'static str,
    sha256: &'static str,
    size: u64,
}

const ASSETS: [Asset; 9] = [
    Asset { file: "tokenizer.json", sha256: "3f04e34bea22f9144d1a19151154095bc9ce0430bf421304f5797e716288a906", size: 3_562_272 },
    Asset { file: "onnx/speech_encoder_q4.onnx", sha256: "37956c20b67bed85a0da4bc83509d67b5969a1b257d1c546516a5236a17ad71e", size: 1_200_346 },
    Asset { file: "onnx/speech_encoder_q4.onnx_data", sha256: "58956db217c6443e49c91bdd54d7cf76b4a243f225c748b7bf746459fc27bc7d", size: 229_560_112 },
    Asset { file: "onnx/embed_tokens_q4.onnx", sha256: "fd6ba1d22902e8f539d3dd6d7c1c44b98ebb4c84ebbb5e47fcb826ddcf667561", size: 2_844 },
    Asset { file: "onnx/embed_tokens_q4.onnx_data", sha256: "f54a51e234b509b64c3a03bb79e1149fba7e2eba6c2d9c222f18883379e1f5d8", size: 37_286_384 },
    Asset { file: "onnx/language_model_q4.onnx", sha256: "b39d03d3f8b943b9e60c6fce3fb41191dbc1df4589f913291db1e214eef669b1", size: 274_572 },
    Asset { file: "onnx/language_model_q4.onnx_data", sha256: "2c029dc0acf48752473d8c74c72b5ceaaad76b9886fe106eaf2022142d5b5d5e", size: 204_456_572 },
    Asset { file: "onnx/conditional_decoder_q4.onnx", sha256: "dccb7a6cea3472dc7f7d070eeb70ade18e6327fb4ec61a3d62cf211bfed90ea2", size: 2_179_022 },
    Asset { file: "onnx/conditional_decoder_q4.onnx_data", sha256: "b5c5317e0b79a1a19dd3d5e2b2091ea06b15716716ab801a54eaeb906c6971ec", size: 246_397_384 },
];

/// Size of the whole download in bytes (for "about 0.7 GB").
pub fn download_size() -> u64 {
    ASSETS.iter().map(|a| a.size).sum()
}

/// Where the model is kept.
pub fn model_dir() -> Option<PathBuf> {
    sf_transcribe::models_dir().map(|d| d.join("chatterbox-turbo-q4"))
}

pub fn is_downloaded() -> bool {
    model_dir().is_some_and(|d| ASSETS.iter().all(|a| d.join(a.file).is_file()))
}

/// Download whatever of the model is missing, reporting progress over the whole download.
pub fn ensure(cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<PathBuf> {
    static DOWNLOADING: Mutex<()> = Mutex::new(());
    let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = model_dir().context("no folder for speech models")?;
    std::fs::create_dir_all(dir.join("onnx")).with_context(|| format!("creating {}", dir.display()))?;
    let total = download_size();
    let mut before: u64 = ASSETS.iter().filter(|a| dir.join(a.file).is_file()).map(|a| a.size).sum();
    for a in &ASSETS {
        let path = dir.join(a.file);
        if path.is_file() {
            continue;
        }
        sf_transcribe::download_verified(&[format!("{REPO}/{}", a.file)], a.sha256, &path, cancel, |p| {
            if let Progress::Downloading { done, .. } = p {
                progress(Progress::Downloading { done: before + done.min(a.size), total: Some(total) });
            }
        })?;
        before += a.size;
    }
    Ok(dir)
}

/// Delete the downloaded model (downloaded again when next needed).
pub fn remove_download() -> Result<()> {
    let Some(dir) = model_dir().filter(|d| d.exists()) else { return Ok(()) };
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
}

/// The speaker's conditioning, from the recording.
struct Speaker {
    /// [1, n, 1024]
    features: (usize, Vec<f32>),
    prompt_tokens: Vec<i64>,
    embedding: Vec<f32>,
    /// [1, frames, 80]
    speaker_features: (usize, Vec<f32>),
}

pub struct Chatterbox {
    encoder: Mutex<Session>,
    embed: Mutex<Session>,
    lm: Mutex<Session>,
    decoder: Mutex<Session>,
    bpe: crate::bpe::Bpe,
    dir: PathBuf,
}

impl Chatterbox {
    pub fn load(dir: &Path) -> Result<Chatterbox> {
        let env = ort::init().with_name("screenforge-my-voice").build().map_err(|e| anyhow::anyhow!("starting ONNX Runtime: {e}"))?;
        // every core: reading is bound by the language model's many small steps
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let open = |name: &str| -> Result<Mutex<Session>> {
            let s = Session::builder(&env)
                .and_then(|b| b.with_intra_threads(threads).map_err(Into::into))
                .and_then(|mut b| b.commit_from_file(dir.join("onnx").join(format!("{name}_q4.onnx"))))
                .map_err(|e| anyhow::anyhow!("loading the voice model ({name}): {e}"))?;
            Ok(Mutex::new(s))
        };
        let bpe = crate::bpe::Bpe::from_json(&std::fs::read_to_string(dir.join("tokenizer.json")).context("reading tokenizer.json")?)?;
        Ok(Chatterbox { encoder: open("speech_encoder")?, embed: open("embed_tokens")?, lm: open("language_model")?, decoder: open("conditional_decoder")?, bpe, dir: dir.to_path_buf() })
    }

    /// Loaded once per run (a few seconds and some 700 MB).
    pub fn shared(dir: &Path) -> Result<Arc<Chatterbox>> {
        static SHARED: Mutex<Option<Arc<Chatterbox>>> = Mutex::new(None);
        let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = s.as_ref().filter(|c| c.dir == dir) {
            return Ok(c.clone());
        }
        let c = Arc::new(Chatterbox::load(dir)?);
        *s = Some(c.clone());
        Ok(c)
    }

    /// Read `text` in the voice of `reference` (mono at [`SAMPLE_RATE`]); blank lines are
    /// longer pauses. `cancel` stops between sentences.
    pub fn speak(&self, text: &str, reference: &[f32], cancel: &AtomicBool) -> Result<Speech> {
        self.speak_with_progress(text, reference, cancel, |_| {})
    }

    /// [`Chatterbox::speak`], telling `progress` how far it is (0–1, estimated from the speech
    /// read so far against the text's length).
    pub fn speak_with_progress(&self, text: &str, reference: &[f32], cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<Speech> {
        let max = (MAX_REFERENCE_S * SAMPLE_RATE as f64) as usize;
        if (reference.len() as f64) < MIN_REFERENCE_S * SAMPLE_RATE as f64 {
            bail!("the recording of your voice is too short (at least {MIN_REFERENCE_S:.0} s)");
        }
        let speaker = self.speaker(&reference[..reference.len().min(max)])?;
        let total: usize = text.split("\n\n").flat_map(|p| sentences(p.trim())).map(|s| s.chars().count()).sum::<usize>().max(1);
        let mut done = 0usize;
        progress(0.0);
        let mut speech = Speech::default();
        let gap = |s: f64| vec![0f32; (s * SAMPLE_RATE as f64) as usize];
        for (pi, paragraph) in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).enumerate() {
            if pi > 0 {
                speech.samples.extend(gap(PARAGRAPH_GAP - SENTENCE_GAP));
            }
            for piece in sentences(paragraph).iter().flat_map(|s| split_long(s)) {
                if cancel.load(Ordering::Relaxed) {
                    bail!("cancelled");
                }
                let chars = piece.chars().count();
                let audio = self.generate(&speaker, &piece, &mut |tokens| {
                    let part = (tokens as f32 / (chars as f32 * TOKENS_PER_CHAR)).min(0.98);
                    progress((done as f32 + part * chars as f32) / total as f32);
                })?;
                done += chars;
                progress(done as f32 / total as f32);
                let (audio, voiced) = trim(audio);
                let at = speech.duration_ms();
                speech.words.extend(time_words(&piece, voiced).into_iter().map(|w| SpokenWord { start_ms: at + w.start_ms, end_ms: at + w.end_ms, text: w.text }));
                speech.samples.extend(audio);
                speech.samples.extend(gap(SENTENCE_GAP));
            }
        }
        if speech.words.is_empty() {
            bail!("there is nothing to say in the text");
        }
        speech.samples.truncate(speech.samples.len().saturating_sub(gap(SENTENCE_GAP).len()));
        let peak = speech.samples.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
        speech.samples.iter_mut().for_each(|v| *v *= PEAK / peak);
        Ok(speech)
    }

    fn speaker(&self, reference: &[f32]) -> Result<Speaker> {
        let mut enc = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        let out = enc.run(ort::inputs!["audio_values" => Tensor::from_array(([1usize, reference.len()], reference.to_vec()))?])?;
        let (shape, features) = out["audio_features"].try_extract_tensor::<f32>()?;
        let n = shape[1] as usize;
        let features = features.to_vec();
        let (_, prompt) = out["audio_tokens"].try_extract_tensor::<i64>()?;
        let (_, embedding) = out["speaker_embeddings"].try_extract_tensor::<f32>()?;
        let (fshape, speaker_features) = out["speaker_features"].try_extract_tensor::<f32>()?;
        Ok(Speaker { features: (n, features), prompt_tokens: prompt.to_vec(), embedding: embedding.to_vec(), speaker_features: (fshape[1] as usize, speaker_features.to_vec()) })
    }

    fn embed(&self, ids: &[i64]) -> Result<Vec<f32>> {
        let mut s = self.embed.lock().unwrap_or_else(|e| e.into_inner());
        let out = s.run(ort::inputs!["input_ids" => Tensor::from_array(([1usize, ids.len()], ids.to_vec()))?])?;
        Ok(out["inputs_embeds"].try_extract_tensor::<f32>()?.1.to_vec())
    }

    /// One piece of text as audio: speech tokens until the model stops, then the decoder.
    /// `tokens` hears how many speech tokens are read so far.
    fn generate(&self, speaker: &Speaker, text: &str, tokens_so_far: &mut dyn FnMut(usize)) -> Result<Vec<f32>> {
        let ids = self.bpe.encode(text);
        let width = 1024;
        // the speaker's conditioning, then the text
        let mut embeds = speaker.features.1.clone();
        embeds.extend(self.embed(&ids)?);
        let mut seq = embeds.len() / width;
        let mut lm = self.lm.lock().unwrap_or_else(|e| e.into_inner());
        let past_names: Vec<String> = lm.inputs().iter().map(|i| i.name().to_string()).filter(|n| n.starts_with("past_key_values")).collect();
        let mut past: Vec<SessionInputValue<'static>> = past_names.iter().map(|_| Tensor::from_array(([1usize, KV_HEADS, 0, HEAD_DIM], Vec::<f32>::new())).map(Into::into)).collect::<ort::Result<_>>()?;
        let mut total = 0usize;
        let mut tokens: Vec<i64> = vec![START_SPEECH];
        // a runaway reading (silence that never stops) is cut off; ~25 tokens a second of speech
        let max_tokens = 120 + text.chars().count() * 8;
        for step in 0..max_tokens {
            let n = seq;
            total += n;
            let positions: Vec<i64> = ((total - n) as i64..total as i64).collect();
            let mut inputs: Vec<(Cow<str>, SessionInputValue)> = vec![
                ("inputs_embeds".into(), Tensor::from_array(([1usize, n, width], std::mem::take(&mut embeds)))?.into()),
                ("attention_mask".into(), Tensor::from_array(([1usize, total], vec![1i64; total]))?.into()),
                ("position_ids".into(), Tensor::from_array(([1usize, n], positions))?.into()),
            ];
            inputs.extend(past_names.iter().map(|k| Cow::Owned(k.clone())).zip(std::mem::take(&mut past)));
            let mut out = lm.run(inputs)?;
            let next = {
                let (shape, logits) = out["logits"].try_extract_tensor::<f32>()?;
                let vocab = shape[2] as usize;
                let mut last = logits[(n - 1) * vocab..n * vocab].to_vec();
                for &t in &tokens {
                    if let Some(l) = last.get_mut(t as usize) {
                        *l = if *l < 0.0 { *l * REPETITION_PENALTY } else { *l / REPETITION_PENALTY };
                    }
                }
                last.iter().enumerate().fold((0usize, f32::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b }).0 as i64
            };
            tokens.push(next);
            tokens_so_far(tokens.len() - 1);
            if next == STOP_SPEECH {
                break;
            }
            past = past_names
                .iter()
                .map(|k| out.remove(k.replace("past_key_values", "present")).map(SessionInputValue::Owned).with_context(|| format!("no {k} from the model")))
                .collect::<Result<_>>()?;
            drop(out);
            embeds = self.embed(&[next])?;
            seq = 1;
            if step + 1 == max_tokens {
                tracing::warn!("my voice: reading stopped after {max_tokens} tokens");
            }
        }
        drop(lm);
        let generated: Vec<i64> = tokens[1..].iter().copied().filter(|&t| t != STOP_SPEECH).collect();
        let mut speech_tokens = speaker.prompt_tokens.clone();
        speech_tokens.extend(generated);
        speech_tokens.extend([SILENCE; 3]);
        let mut dec = self.decoder.lock().unwrap_or_else(|e| e.into_inner());
        let (frames, feats) = &speaker.speaker_features;
        let out = dec.run(ort::inputs![
            "speech_tokens" => Tensor::from_array(([1usize, speech_tokens.len()], speech_tokens))?,
            "speaker_embeddings" => Tensor::from_array(([1usize, 192], speaker.embedding.clone()))?,
            "speaker_features" => Tensor::from_array(([1usize, *frames, 80], feats.clone()))?,
        ])?;
        Ok(out["waveform"].try_extract_tensor::<f32>()?.1.to_vec())
    }
}

/// A sentence longer than [`MAX_CHARS`] cut between words (after a comma when there is one).
fn split_long(s: &str) -> Vec<String> {
    if s.chars().count() <= MAX_CHARS {
        return vec![s.to_string()];
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let half = s.chars().count() / 2;
    let mut at = 0;
    let mut best = (usize::MAX, 1);
    for (i, w) in words.iter().enumerate().take(words.len().saturating_sub(1)) {
        at += w.chars().count() + 1;
        let score = at.abs_diff(half) / if w.ends_with(',') || w.ends_with(';') { 4 } else { 1 };
        if score < best.0 {
            best = (score, i + 1);
        }
    }
    let (left, right) = words.split_at(best.1);
    let mut out = split_long(&left.join(" "));
    out.extend(split_long(&right.join(" ")));
    out
}

/// Spread a piece's words over its voiced span by their length (with a space's worth between).
fn time_words(text: &str, (from, to): (f64, f64)) -> Vec<SpokenWord> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum::<usize>().max(1);
    let at = |i: usize| from + (to - from) * i as f64 / total as f64;
    let mut i = 0;
    words
        .iter()
        .map(|w| {
            let len = w.chars().count();
            let word = SpokenWord { start_ms: at(i), end_ms: at(i + len), text: w.to_string() };
            i += len + 1;
            word
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_sentences_are_cut_between_words() {
        let s = format!("{}, {}", "word ".repeat(40).trim(), "more ".repeat(40).trim());
        let parts = split_long(&s);
        assert!(parts.len() >= 2 && parts.iter().all(|p| p.chars().count() <= MAX_CHARS), "{parts:?}");
        assert_eq!(parts.join(" "), s);
        assert_eq!(split_long("Short one."), ["Short one."]);
    }

    #[test]
    fn words_are_spread_by_length() {
        let w = time_words("ab cd", (0.0, 600.0));
        assert_eq!(w[0], SpokenWord { start_ms: 0.0, end_ms: 200.0, text: "ab".into() });
        assert_eq!(w[1], SpokenWord { start_ms: 300.0, end_ms: 500.0, text: "cd".into() });
    }

    /// Reads with the downloaded model in the voice of `SF_MY_VOICE_REF` (a WAV at 24 kHz):
    /// `SF_MY_VOICE_REF=… SF_TTS_SAMPLE=out.wav cargo test -p sf-tts --release my_voice -- --ignored`.
    #[test]
    #[ignore]
    fn my_voice_speaks_with_the_downloaded_model() {
        let dir = ensure(&AtomicBool::new(false), |_| {}).unwrap();
        let reference = read_wav_24k(Path::new(&std::env::var("SF_MY_VOICE_REF").expect("SF_MY_VOICE_REF")));
        let c = Chatterbox::load(&dir).unwrap();
        let t = std::time::Instant::now();
        let mut seen = vec![];
        let s = c.speak_with_progress("We're provisioning our personal GPU box with a single command. Terraform plans six resources, and we let it apply.", &reference, &AtomicBool::new(false), |f| seen.push(f)).unwrap();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "progress goes back: {seen:?}");
        assert_eq!((seen.first().copied(), seen.last().copied()), (Some(0.0), Some(1.0)));
        let mid = seen.iter().filter(|f| **f > 0.1 && **f < 0.9).count();
        eprintln!("{} progress updates, {mid} between 10 and 90 %", seen.len());
        assert!(mid > 20, "progress jumps: {seen:?}");
        eprintln!("{:.1} s of speech in {:.1} s", s.duration_ms() / 1000.0, t.elapsed().as_secs_f64());
        assert!(s.duration_ms() > 3000.0 && s.duration_ms() < 15000.0, "{}", s.duration_ms());
        assert_eq!(s.words.len(), 19);
        if let Ok(out) = std::env::var("SF_TTS_SAMPLE") {
            s.write_wav(Path::new(&out)).unwrap();
        }
    }

    /// A 16-bit mono WAV at 24 kHz (what the test is given).
    fn read_wav_24k(path: &Path) -> Vec<f32> {
        let b = std::fs::read(path).unwrap();
        let data = b.windows(4).position(|w| w == b"data").unwrap() + 8;
        b[data..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
    }
}
