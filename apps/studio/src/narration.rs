//! AI voice narration for the Narration panel and the agent: downloading the voice model,
//! previewing a voice, and reading text into voice-over takes (with their words for captions),
//! all off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sf_project::transcript::{Generator, TranscriptFile, TranscriptItem};
use sf_tts::Progress;

/// What the preview says.
const PREVIEW: &str = "Hi! This is how I sound reading your video's narration.";

/// A take read into the project's voice-over folder.
#[derive(Debug, Clone)]
pub struct Made {
    pub file_name: String,
    pub duration_ms: f64,
    pub text: String,
    pub voice: String,
    /// The take this replaces (same place, new reading), or `None` for a new take at
    /// `source_ms`.
    pub replaces: Option<String>,
    pub source_ms: f64,
}

/// One take to read.
#[derive(Debug, Clone)]
pub struct TakeRequest {
    pub text: String,
    pub voice: String,
    pub speed: f64,
    pub replaces: Option<String>,
    pub source_ms: f64,
}

enum Msg {
    Progress(Progress),
    Done(Result<Finished, String>),
}

enum Finished {
    Downloaded,
    Preview(sf_tts::Speech),
    Takes(Vec<Made>),
}

/// What a run is doing, for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Download,
    Preview,
    Takes(usize),
}

#[derive(Default)]
pub struct NarrationJob {
    running: Option<(Receiver<Msg>, Arc<AtomicBool>, Work)>,
    pub progress: Option<Progress>,
    /// The last run's error.
    pub error: Option<String>,
    /// The text box of the panel.
    pub draft: String,
    /// The selected narrated take's script while it is edited: (take id, text).
    pub take_draft: Option<(String, String)>,
    /// The take being read again when a run reads just one (its status shows by that take).
    for_take: Option<String>,
    player: Option<Player>,
}

impl NarrationJob {
    pub fn working(&self) -> Option<Work> {
        self.running.as_ref().map(|r| r.2)
    }

    /// The model is on disk (a few file checks; the agent may have downloaded it).
    pub fn is_downloaded(&self) -> bool {
        sf_tts::is_downloaded()
    }

    /// The model is being downloaded (on its own, or before a preview or a reading).
    pub fn is_downloading(&self) -> bool {
        match (self.working(), &self.progress) {
            (Some(Work::Download), _) => true,
            (Some(_), Some(Progress::Downloading { done, total: Some(t) })) => done < t,
            _ => false,
        }
    }

    /// The one take a run is reading again.
    pub fn rereading(&self) -> Option<&str> {
        self.working().and(self.for_take.as_deref())
    }

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(Player::is_playing)
    }

    pub fn stop_preview(&mut self) {
        self.player = None;
    }

    pub fn cancel(&mut self) {
        if let Some((_, c, _)) = &self.running {
            c.store(true, Ordering::Relaxed);
        }
    }

    fn start(&mut self, work: Work, f: impl FnOnce(&AtomicBool, &dyn Fn(Progress)) -> Result<Finished, String> + Send + 'static) {
        if self.running.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            let ptx = std::sync::Mutex::new(tx.clone());
            let progress = move |p: Progress| {
                let _ = ptx.lock().map(|t| t.send(Msg::Progress(p)));
            };
            let r = f(&c, &progress);
            let _ = tx.send(Msg::Done(r));
        });
        self.running = Some((rx, cancel, work));
        self.progress = None;
        self.error = None;
    }

    /// Download the model, the dictionaries and every voice.
    pub fn download(&mut self) {
        self.start(Work::Download, |cancel, progress| sf_tts::ensure(cancel, progress).map(|_| Finished::Downloaded).map_err(|e| format!("{e:#}")));
    }

    /// Delete the downloaded model.
    pub fn remove(&mut self) {
        if self.running.is_some() {
            return;
        }
        self.player = None;
        self.error = sf_tts::remove_download().err().map(|e| format!("{e:#}"));
    }

    /// Say a line with `voice` (downloading the model first if needed).
    pub fn preview(&mut self, voice: &str, speed: f64) {
        self.player = None;
        let voice = voice.to_string();
        self.start(Work::Preview, move |cancel, progress| {
            let v = sf_tts::voice(&voice).ok_or("unknown voice")?;
            let dir = sf_tts::ensure(cancel, progress).map_err(|e| format!("{e:#}"))?;
            sf_tts::Kokoro::shared(&dir).and_then(|k| k.speak(PREVIEW, v, speed as f32)).map(Finished::Preview).map_err(|e| format!("{e:#}"))
        });
    }

    /// Read each of `takes` into `project`'s voice-over folder.
    pub fn read(&mut self, project: PathBuf, takes: Vec<TakeRequest>) {
        if takes.is_empty() || self.running.is_some() {
            return;
        }
        self.for_take = if let [t] = takes.as_slice() { t.replaces.clone() } else { None };
        self.start(Work::Takes(takes.len()), move |cancel, progress| {
            sf_tts::ensure(cancel, progress).map_err(|e| format!("{e:#}"))?;
            let mut made = vec![];
            for t in takes {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let spoken = speak_into(&project, &t.text, &t.voice, t.speed)?;
                made.push(Made { file_name: spoken.file_name, duration_ms: spoken.duration_ms, text: t.text, voice: t.voice, replaces: t.replaces, source_ms: t.source_ms });
            }
            Ok(Finished::Takes(made))
        });
    }

    /// Drain messages: the takes read when a run of them just finished.
    pub fn poll(&mut self) -> Option<Vec<Made>> {
        let mut out = None;
        let mut done = false;
        if let Some((rx, _, _)) = &self.running {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(p) => self.progress = Some(p),
                    Msg::Done(r) => {
                        done = true;
                        match r {
                            Ok(Finished::Downloaded) => {}
                            Ok(Finished::Preview(speech)) => match Player::play(speech.samples) {
                                Ok(p) => self.player = Some(p),
                                Err(e) => self.error = Some(format!("Could not play the preview: {e}")),
                            },
                            Ok(Finished::Takes(made)) => out = Some(made),
                            Err(e) if e == "cancelled" => {}
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
            }
        }
        if done {
            self.running = None;
            self.progress = None;
            self.for_take = None;
        }
        if self.player.as_ref().is_some_and(|p| !p.is_playing()) {
            self.player = None;
        }
        out
    }

    /// "Downloading the voice model… 42%" while a run downloads.
    pub fn status(&self) -> Option<String> {
        let work = self.working()?;
        Some(match (&self.progress, work) {
            (Some(Progress::Downloading { done, total: Some(t) }), _) if done < t => format!("Downloading the voice model… {:.0}%", *done as f64 * 100.0 / (*t).max(1) as f64),
            (_, Work::Download) => "Downloading the voice model…".into(),
            (_, Work::Preview) => "Preparing the preview…".into(),
            (_, Work::Takes(1)) => "Reading the narration…".into(),
            (_, Work::Takes(n)) => format!("Reading {n} takes…"),
        })
    }
}

/// Put `made` takes on the timeline: a new take where it was asked for, a re-read one in
/// place of the take it replaces (keeping its place, lane, volume and boost). Returns the ids
/// of the takes placed.
pub fn place(project: &mut sf_project::Project, made: Vec<Made>) -> Vec<String> {
    use sf_project::v4::VoiceOver;
    let list = &mut project.scenes[0].voice_overs;
    let mut ids = vec![];
    for m in made {
        let vo = VoiceOver::spoken(m.source_ms, m.duration_ms, m.file_name, &m.text, &m.voice);
        match m.replaces.and_then(|id| list.iter_mut().find(|v| v.id == id)) {
            Some(old) => {
                old.file_name = vo.file_name;
                old.voiceover_duration_ms = vo.voiceover_duration_ms;
                old.file_offset_ms = 0.0;
                old.source = vo.source;
                ids.push(old.id.clone());
            }
            None => {
                ids.push(vo.id.clone());
                list.push(vo);
            }
        }
    }
    ids
}

/// Read `text` with `voice` into `project`'s voice-over folder, with its words saved as the
/// take's transcript (they are the captions; the take is never transcribed).
pub fn speak_into(project: &Path, text: &str, voice: &str, speed: f64) -> Result<sf_agent::host::Spoken, String> {
    let v = sf_tts::voice(voice).ok_or_else(|| format!("unknown voice {voice}"))?;
    // the first narration downloads the model
    let models = sf_tts::ensure(&AtomicBool::new(false), |_| {}).map_err(|e| format!("{e:#}"))?;
    let speech = sf_tts::Kokoro::shared(&models).and_then(|k| k.speak(text, v, speed as f32)).map_err(|e| format!("{e:#}"))?;
    let dir = project.join(sf_project::v4::VoiceOver::DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("narration-{}.wav", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
    speech.write_wav(&dir.join(&name)).map_err(|e| format!("{e:#}"))?;
    let items = speech.words.iter().enumerate().map(|(i, w)| TranscriptItem::new(i as u32, w.start_ms, w.end_ms, w.text.clone())).collect();
    let generator = Generator { kind: "kokoro".into(), model: Some(voice.into()), language: Some("en".into()), prompt: None, extra: Default::default() };
    TranscriptFile::voice_over(&name, generator, items).save(project).map_err(|e| e.to_string())?;
    Ok(sf_agent::host::Spoken { file_name: name, duration_ms: speech.duration_ms() })
}

/// Plays a preview on the default output once.
struct Player {
    _stream: cpal::Stream,
    finished: Arc<AtomicBool>,
}

impl Player {
    fn play(samples: Vec<f32>) -> anyhow::Result<Player> {
        use anyhow::Context;
        let device = cpal::default_host().default_output_device().context("no speaker or headphones")?;
        let config = device.default_output_config()?;
        let channels = config.channels().max(1) as usize;
        let rate = config.sample_rate() as f64;
        // linear resampling from 24 kHz is plenty for a preview
        let step = sf_tts::SAMPLE_RATE as f64 / rate;
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let mut pos = 0f64;
        let mut next = move || {
            let i = pos as usize;
            let v = match (samples.get(i), samples.get(i + 1)) {
                (Some(a), Some(b)) => a + (b - a) * (pos - i as f64) as f32,
                (Some(a), None) => *a,
                _ => {
                    done.store(true, Ordering::Relaxed);
                    0.0
                }
            };
            pos += step;
            v
        };
        let err = |e| tracing::warn!("preview output error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(config.into(), move |data: &mut [f32], _| data.chunks_mut(channels).for_each(|f| f.fill(next())), err, None)?,
            cpal::SampleFormat::I16 => device.build_output_stream(config.into(), move |data: &mut [i16], _| data.chunks_mut(channels).for_each(|f| f.fill((next() * 32767.0) as i16)), err, None)?,
            f => anyhow::bail!("unsupported output format {f:?}"),
        };
        stream.play()?;
        Ok(Player { _stream: stream, finished })
    }

    fn is_playing(&self) -> bool {
        !self.finished.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::v4::{Slice, VoiceOver};

    fn made(file: &str, replaces: Option<&str>) -> Made {
        Made { file_name: file.into(), duration_ms: 900.0, text: "Hello there".into(), voice: "am_michael".into(), replaces: replaces.map(str::to_string), source_ms: 420.0 }
    }

    #[test]
    fn a_new_take_is_added_beside_the_others_and_a_reread_one_keeps_its_place() {
        let mut p = sf_project::Project::new("T");
        p.scenes[0].slices = vec![Slice::new(0.0, 10_000.0)];
        let mut rec = VoiceOver::microphone(1_400.0, 4_000.0, "voiceover.wav");
        rec.volume = 0.5;
        p.scenes[0].voice_overs.push(rec);
        let ids = place(&mut p, vec![made("n1.wav", None)]);
        let vos = &p.scenes[0].voice_overs;
        assert_eq!(vos.len(), 2, "the recorded take stays");
        let new = vos.iter().find(|v| v.id == ids[0]).unwrap();
        assert_eq!((new.source_start_ms, new.voiceover_duration_ms, new.spoken_voice()), (420.0, 900.0, Some("am_michael")));

        // read again in another voice: same take, same place, new file
        let id = ids[0].clone();
        p.scenes[0].voice_overs.iter_mut().find(|v| v.id == id).unwrap().boost_db = 6.0;
        let mut again = made("n2.wav", Some(&id));
        again.voice = "af_heart".into();
        again.source_ms = 0.0;
        assert_eq!(place(&mut p, vec![again]), vec![id.clone()]);
        let t = p.scenes[0].voice_overs.iter().find(|v| v.id == id).unwrap();
        assert_eq!((t.file_name.as_str(), t.source_start_ms, t.boost_db, t.spoken_voice()), ("n2.wav", 420.0, 6.0, Some("af_heart")));
        assert_eq!(p.scenes[0].voice_overs.len(), 2);
    }
}
