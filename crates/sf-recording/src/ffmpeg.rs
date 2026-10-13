//! Locating the `ffmpeg` binary (same lookup order as `sf-media`): `SF_FFMPEG`, next to
//! the executable, `PATH`, then Homebrew prefixes.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;

use crate::log::RecLog;
use crate::{Error, Result};

fn find(bin: &str, env: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["", "../Resources/bin/", "bin/"] {
                let p = dir.join(rel).join(bin);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    if let Ok(p) = which::which(bin) {
        return Some(p);
    }
    for prefix in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let p = PathBuf::from(prefix).join(bin);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Path of the ffmpeg binary.
pub fn ffmpeg_path() -> Result<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find("ffmpeg", "SF_FFMPEG")).as_ref().ok_or(Error::FfmpegNotFound)
}

/// Path of the ffprobe binary.
pub fn ffprobe_path() -> Result<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find("ffprobe", "SF_FFPROBE")).as_ref().ok_or(Error::FfmpegNotFound)
}

/// `ffmpeg -hide_banner -nostdin -loglevel warning …`
pub fn ffmpeg_cmd() -> Result<Command> {
    let mut c = Command::new(ffmpeg_path()?);
    c.arg("-hide_banner").arg("-nostdin").arg("-loglevel").arg("warning");
    Ok(c)
}

/// A running ffmpeg fed through stdin. stderr is forwarded to the recording log.
pub struct FfmpegProc {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr_thread: Option<JoinHandle<()>>,
    label: String,
}

impl std::fmt::Debug for FfmpegProc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FfmpegProc({})", self.label)
    }
}

impl FfmpegProc {
    /// Spawn `ffmpeg <args>` in `cwd` with a piped stdin.
    pub fn spawn(args: &[String], cwd: &Path, log: RecLog, label: &str) -> Result<Self> {
        Self::spawn_with(args, cwd, log, label, false).map(|(p, _)| p)
    }

    /// Like [`spawn`](Self::spawn) but with stdout piped back (for captures that write frames to `pipe:1`).
    pub fn spawn_reading(args: &[String], cwd: &Path, log: RecLog, label: &str) -> Result<(Self, ChildStdout)> {
        let (p, out) = Self::spawn_with(args, cwd, log, label, true)?;
        let out = out.ok_or_else(|| Error::Capture(format!("{label}: no ffmpeg stdout")))?;
        Ok((p, out))
    }

    fn spawn_with(args: &[String], cwd: &Path, log: RecLog, label: &str, read_stdout: bool) -> Result<(Self, Option<ChildStdout>)> {
        let mut cmd = ffmpeg_cmd()?;
        let stdout = if read_stdout { Stdio::piped() } else { Stdio::null() };
        cmd.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(stdout).stderr(Stdio::piped());
        log.debug(label, format!("Spawning ffmpeg {}", args.join(" ")));
        let mut child = cmd.spawn().map_err(|source| Error::Io { path: "ffmpeg".into(), source })?;
        let stdout = child.stdout.take();
        let stdin = child.stdin.take();
        let stderr = child.stderr.take();
        let label_s = label.to_string();
        let stderr_thread = stderr.map(|err| {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(std::result::Result::ok) {
                    log.warning(&label_s, format!("ffmpeg: {line}"));
                }
            })
        });
        Ok((FfmpegProc { child, stdin, stderr_thread, label: label.into() }, stdout))
    }

    /// Write raw input.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Error::Capture(format!("{}: ffmpeg stdin closed", self.label)));
        };
        stdin.write_all(bytes).map_err(|source| Error::Io { path: "ffmpeg stdin".into(), source })
    }

    /// Close stdin, let ffmpeg flush its muxer and wait for it to exit.
    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|source| Error::Io { path: "ffmpeg".into(), source })?;
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        if status.success() {
            Ok(())
        } else {
            Err(Error::Capture(format!("{}: ffmpeg exited with {status}", self.label)))
        }
    }

    /// Ask ffmpeg to stop (SIGINT, like pressing Ctrl-C) so it finalizes the output, then wait.
    /// For captures that do not read stdin (e.g. X11 grabbing).
    #[cfg(unix)]
    pub fn interrupt(mut self) -> Result<()> {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGINT) };
        let status = self.child.wait().map_err(|source| Error::Io { path: "ffmpeg".into(), source })?;
        drop(self.stdin.take());
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        // ffmpeg exits with 255 after SIGINT even when the output is complete
        if status.success() || status.code() == Some(255) {
            Ok(())
        } else {
            Err(Error::Capture(format!("{}: ffmpeg exited with {status}", self.label)))
        }
    }

    /// Kill the process (used by cancel).
    pub fn kill(mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
    }
}

/// Does this ffmpeg have the given encoder (e.g. `h264_videotoolbox`)?
pub fn has_encoder(name: &str) -> bool {
    static CACHE: OnceLock<String> = OnceLock::new();
    let list = CACHE.get_or_init(|| {
        ffmpeg_cmd()
            .and_then(|mut c| c.arg("-encoders").output().map_err(|e| Error::Io { path: "ffmpeg".into(), source: e }))
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    });
    list.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}
