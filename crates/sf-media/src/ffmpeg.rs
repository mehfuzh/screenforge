//! Locating and spawning the ffmpeg / ffprobe binaries.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use crate::{Error, Result};

fn find(bin: &str, env: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    // Next to our own executable (bundled), then PATH, then Homebrew.
    if let Ok(exe) = std::env::current_exe() {
        for rel in ["", "../Resources/bin/", "bin/"] {
            let p = exe.parent().unwrap().join(rel).join(bin);
            if p.is_file() {
                return Some(p);
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

pub fn ffmpeg_path() -> Result<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find("ffmpeg", "SF_FFMPEG")).as_ref().ok_or(Error::FfmpegNotFound)
}

pub fn ffprobe_path() -> Result<&'static PathBuf> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| find("ffprobe", "SF_FFPROBE")).as_ref().ok_or(Error::FfmpegNotFound)
}

pub fn ffmpeg_cmd() -> Result<Command> {
    let mut c = Command::new(ffmpeg_path()?);
    c.arg("-hide_banner").arg("-nostdin").arg("-loglevel").arg("error");
    Ok(c)
}

/// Does this ffmpeg have the given encoder?
pub fn has_encoder(name: &str) -> bool {
    static CACHE: OnceLock<String> = OnceLock::new();
    let list = CACHE.get_or_init(|| {
        ffmpeg_cmd()
            .and_then(|mut c| c.arg("-encoders").output().map_err(Into::into))
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    });
    list.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}
