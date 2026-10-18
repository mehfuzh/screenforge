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
    let file = format!("{bin}{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        for rel in ["", "../Resources/bin/", "bin/",
                // `cargo run` from the repo: the static build scripts/fetch-ffmpeg.sh downloads
                "../ffmpeg-static/windows-x86_64/", "../ffmpeg-static/linux-x86_64/", "../ffmpeg-static/arm64/", "../ffmpeg-static/x86_64/"] {
            let p = exe.parent().unwrap().join(rel).join(&file);
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
    hide_console(&mut c);
    c.arg("-hide_banner").arg("-nostdin").arg("-loglevel").arg("error");
    Ok(c)
}

/// `ffprobe` with the same console handling as [`ffmpeg_cmd`].
pub fn ffprobe_cmd() -> Result<Command> {
    let mut c = Command::new(ffprobe_path()?);
    hide_console(&mut c);
    Ok(c)
}

/// Windows: a GUI app would otherwise pop up a console window for every process it spawns.
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
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

/// Can this machine actually run `encoder` with `pix_fmt`? `-encoders` lists the VideoToolbox
/// encoders on every Mac, but the hardware ProRes encoder only works where the media engine
/// has ProRes (not on base M1/M2 chips or virtual Macs). A tiny test encode, cached.
pub fn encoder_works(encoder: &str, pix_fmt: &str) -> bool {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    let key = format!("{encoder}/{pix_fmt}");
    let cache = CACHE.get_or_init(Default::default);
    if let Some(ok) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        return ok;
    }
    let ok = has_encoder(encoder)
        && ffmpeg_cmd()
            .and_then(|mut c| {
                c.args(["-f", "lavfi", "-i", "color=c=gray:s=64x64:d=0.1", "-c:v", encoder, "-pix_fmt", pix_fmt, "-f", "null", "-"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map_err(Into::into)
            })
            .map(|s| s.success())
            .unwrap_or(false);
    if let Ok(mut c) = cache.lock() {
        c.insert(key, ok);
    }
    ok
}
