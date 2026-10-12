//! `polyrecorder.log`: a plain text log written next to the media files.
//!
//! Line format mirrors polyrecorder's:
//! `2026-10-07T11:53:28.148-0700 notice   [Component] message key=value`.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

/// File name of the log inside the recording directory.
pub const LOG_FILENAME: &str = "polyrecorder.log";

/// Shared handle to the recording log. Cheap to clone; writes are serialized.
#[derive(Clone)]
pub struct RecLog {
    inner: Arc<Mutex<Option<BufWriter<File>>>>,
}

impl std::fmt::Debug for RecLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecLog")
    }
}

impl RecLog {
    /// Open (append) `dir/polyrecorder.log`.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(dir.join(LOG_FILENAME))?;
        Ok(RecLog { inner: Arc::new(Mutex::new(Some(BufWriter::new(file)))) })
    }

    /// A log that discards everything (used before the output directory exists and in tests).
    pub fn null() -> Self {
        RecLog { inner: Arc::new(Mutex::new(None)) }
    }

    fn write(&self, level: &str, component: &str, msg: &str) {
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%z");
        let mut guard = self.inner.lock();
        if let Some(w) = guard.as_mut() {
            let _ = writeln!(w, "{ts} {level:<8} [{component}] {msg}");
            let _ = w.flush();
        }
    }

    /// Lifecycle messages.
    pub fn notice(&self, component: &str, msg: impl AsRef<str>) {
        tracing::info!(target: "sf_recording", "[{component}] {}", msg.as_ref());
        self.write("notice", component, msg.as_ref());
    }

    /// Verbose details.
    pub fn debug(&self, component: &str, msg: impl AsRef<str>) {
        tracing::debug!(target: "sf_recording", "[{component}] {}", msg.as_ref());
        self.write("debug", component, msg.as_ref());
    }

    /// Something went wrong but recording continues.
    pub fn warning(&self, component: &str, msg: impl AsRef<str>) {
        tracing::warn!(target: "sf_recording", "[{component}] {}", msg.as_ref());
        self.write("warning", component, msg.as_ref());
    }

    /// Fatal for a recorder.
    pub fn error(&self, component: &str, msg: impl AsRef<str>) {
        tracing::error!(target: "sf_recording", "[{component}] {}", msg.as_ref());
        self.write("error", component, msg.as_ref());
    }
}
