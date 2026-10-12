//! Audio decoding to interleaved f32 PCM through ffmpeg (for preview playback and waveforms).

use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use crate::{ffmpeg::ffmpeg_cmd, Error, Result};

/// Fully decoded audio clip.
#[derive(Debug, Clone)]
pub struct AudioClip {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples.
    pub samples: Arc<[f32]>,
}

impl AudioClip {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn duration_ms(&self) -> f64 {
        self.frames() as f64 * 1000.0 / self.sample_rate as f64
    }

    pub fn frame_at_ms(&self, ms: f64) -> usize {
        ((ms / 1000.0) * self.sample_rate as f64).max(0.0) as usize
    }

    /// Peak amplitude per bucket (for waveform drawing), `buckets` across the whole clip.
    pub fn peaks(&self, buckets: usize) -> Vec<f32> {
        let frames = self.frames().max(1);
        let ch = self.channels.max(1) as usize;
        let per = (frames as f64 / buckets.max(1) as f64).max(1.0);
        (0..buckets)
            .map(|b| {
                let s = (b as f64 * per) as usize;
                let e = (((b + 1) as f64 * per) as usize).min(frames);
                self.samples[s * ch..e.max(s) * ch].iter().fold(0.0f32, |m, v| m.max(v.abs()))
            })
            .collect()
    }
}

/// Decode `path` to f32 PCM at `sample_rate` with `channels` channels (1 or 2).
/// ffmpeg audio filter for a microphone preset (`config.audio.microphoneAudioPreset`).
/// `voice` (Standard) is untouched; the others clean up and level speech.
pub fn microphone_filter(preset: &str) -> Option<&'static str> {
    match preset {
        // rumble cut + FFT noise reduction
        "noise-reduction" => Some("highpass=f=80,afftdn=nr=18:nf=-30:tn=1"),
        // noise reduction, presence, compression and make-up gain, limited: louder, even voice
        "podcast" => Some("highpass=f=80,afftdn=nr=18:nf=-30:tn=1,equalizer=f=3500:t=q:w=1.2:g=3,acompressor=threshold=-20dB:ratio=3.5:attack=5:release=150:makeup=6,alimiter=limit=0.89"),
        _ => None,
    }
}

pub fn decode(path: &Path, sample_rate: u32, channels: u16) -> Result<AudioClip> {
    decode_filtered(path, sample_rate, channels, None)
}

/// `decode` with an optional ffmpeg `-af` filter chain applied before resampling.
pub fn decode_filtered(path: &Path, sample_rate: u32, channels: u16, filter: Option<&str>) -> Result<AudioClip> {
    let mut cmd = ffmpeg_cmd()?;
    if path.extension().and_then(|e| e.to_str()) == Some("m3u8") {
        cmd.args(["-allowed_extensions", "ALL"]);
    }
    cmd.arg("-i").arg(path);
    cmd.args(["-vn", "-sn", "-dn"]);
    if let Some(f) = filter {
        cmd.arg("-af").arg(f);
    }
    cmd.args(["-f", "f32le", "-acodec", "pcm_f32le"]);
    cmd.arg("-ar").arg(sample_rate.to_string());
    cmd.arg("-ac").arg(channels.to_string());
    cmd.arg("-");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut bytes = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut bytes)?;
    let status = child.wait()?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        return Err(Error::FfmpegFailed { status: status.to_string(), stderr: err });
    }
    let samples: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    Ok(AudioClip { sample_rate, channels, samples: samples.into() })
}
