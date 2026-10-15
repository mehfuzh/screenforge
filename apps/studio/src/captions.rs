//! Background transcription for the Captions panel: downloads the Whisper model if needed,
//! transcribes the microphone and reports progress to the UI.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use sf_transcribe::{Model, Options, Progress};

enum Msg {
    Progress(Progress),
    Done(Result<usize, String>),
}

pub struct TranscribeJob {
    pub model: Model,
    /// Language code; empty = detect.
    pub language: String,
    running: Option<(Receiver<Msg>, Arc<AtomicBool>)>,
    pub progress: Option<Progress>,
    /// Outcome of the last run: words transcribed, or the error.
    pub result: Option<Result<usize, String>>,
}

impl Default for TranscribeJob {
    fn default() -> Self {
        TranscribeJob { model: Model::Base, language: String::new(), running: None, progress: None, result: None }
    }
}

impl TranscribeJob {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn start(&mut self, project: PathBuf) {
        if self.is_running() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let opts = Options { language: Some(self.language.trim().to_lowercase()).filter(|l| !l.is_empty()), prompt: None };
        let model = self.model;
        let c = cancel.clone();
        std::thread::spawn(move || {
            let ptx = std::sync::Mutex::new(tx.clone());
            let r = sf_transcribe::transcribe_project(&project, model, &opts, c, move |p| {
                let _ = ptx.lock().map(|t| t.send(Msg::Progress(p)));
            });
            let _ = tx.send(Msg::Done(r.map(|files| files.iter().map(|f| f.1).sum()).map_err(|e| format!("{e:#}"))));
        });
        self.running = Some((rx, cancel));
        self.progress = None;
        self.result = None;
    }

    pub fn cancel(&mut self) {
        if let Some((_, c)) = &self.running {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Drain messages; true when a run just finished successfully (reload the captions).
    pub fn poll(&mut self) -> bool {
        let mut finished = false;
        if let Some((rx, _)) = &self.running {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(p) => self.progress = Some(p),
                    Msg::Done(r) => {
                        finished = r.is_ok();
                        self.result = Some(r);
                        self.progress = None;
                    }
                }
            }
        }
        if self.result.is_some() {
            self.running = None;
        }
        finished
    }

    /// One-line status for the panel while running.
    pub fn status(&self) -> String {
        match &self.progress {
            Some(Progress::Downloading { done, total: Some(t) }) => format!("Downloading the speech model… {:.0}%", *done as f64 * 100.0 / (*t).max(1) as f64),
            Some(Progress::Downloading { done, total: None }) => format!("Downloading the speech model… {:.0} MB", *done as f64 / 1e6),
            Some(Progress::Transcribing { session, sessions, percent }) if *sessions > 1 => format!("Transcribing part {} of {}… {percent}%", session + 1, sessions),
            Some(Progress::Transcribing { percent, .. }) => format!("Transcribing… {percent}%"),
            None => "Preparing…".into(),
        }
    }
}
