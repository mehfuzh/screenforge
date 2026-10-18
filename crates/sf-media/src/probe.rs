//! `ffprobe -print_format json` wrapper.

use std::path::Path;

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct StreamInfo {
    pub index: u32,
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub sample_rate: Option<String>,
    pub channels: Option<u32>,
    pub duration: Option<String>,
    pub nb_frames: Option<String>,
}

impl StreamInfo {
    pub fn fps(&self) -> Option<f64> {
        let parse = |s: &str| {
            let (n, d) = s.split_once('/')?;
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            (d > 0.0 && n > 0.0).then_some(n / d)
        };
        self.avg_frame_rate.as_deref().and_then(parse).or_else(|| self.r_frame_rate.as_deref().and_then(parse))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormatInfo {
    pub duration: Option<String>,
    pub format_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaInfo {
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
    pub format: Option<FormatInfo>,
}

impl MediaInfo {
    pub fn video(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.codec_type == "video")
    }
    pub fn audio(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.codec_type == "audio")
    }
    pub fn duration_ms(&self) -> Option<f64> {
        let d = self
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .or_else(|| self.streams.iter().find_map(|s| s.duration.as_deref()))?;
        d.parse::<f64>().ok().map(|s| s * 1000.0)
    }
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let out = crate::ffmpeg::ffprobe_cmd()?
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .output()?;
    if !out.status.success() {
        return Err(Error::FfmpegFailed { status: out.status.to_string(), stderr: String::from_utf8_lossy(&out.stderr).into() });
    }
    serde_json::from_slice(&out.stdout).map_err(|e| Error::Probe(e.to_string()))
}
