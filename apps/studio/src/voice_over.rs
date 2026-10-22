//! Voice-over recording: narration captured from a microphone (the system default unless one
//! is chosen) while the video plays (muted) from the playhead, saved as a WAV file in the
//! project's `voiceovers/` folder.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

/// Recordings shorter than this are dropped (a slip of the button).
pub const MIN_MS: f64 = 300.0;

/// RMS level below which a take counts as silent (-50 dBFS; speech sits around -30..-20). A
/// microphone held by a call, or muted, delivers about this little; a single click or bump
/// does not lift the RMS of a take the way it lifts its peak.
pub const SILENT_RMS: f64 = 0.003_16;

/// Samples captured so far and their running sum of squares (for the RMS level).
#[derive(Default)]
struct Capture {
    samples: Vec<f32>,
    sum_sq: f64,
}

impl Capture {
    fn rms(&self) -> f64 {
        (self.sum_sq / self.samples.len().max(1) as f64).sqrt()
    }
}

/// Names of the input devices, for the microphone menu.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices().map(|it| it.filter_map(|d| d.description().ok().map(|n| n.name().to_string())).collect()).unwrap_or_default()
}

/// Name of the system's default input device.
pub fn default_input_name() -> Option<String> {
    cpal::default_host().default_input_device().and_then(|d| d.description().ok()).map(|n| n.name().to_string())
}

/// A saved take.
pub struct Take {
    pub file_name: String,
    pub duration_ms: f64,
    /// RMS level of the whole take (0..1).
    pub rms: f64,
}

impl Take {
    pub fn is_silent(&self) -> bool {
        self.rms < SILENT_RMS
    }
}

pub struct VoiceRecorder {
    /// Source time the voice-over is anchored to (the frame under the playhead at the start).
    pub source_start_ms: f64,
    pub device_name: String,
    started: Instant,
    rate: u32,
    /// Mono samples captured so far.
    capture: Arc<Mutex<Capture>>,
    level: Arc<AtomicU32>,
    _stream: cpal::Stream,
}

impl VoiceRecorder {
    /// Open the input device named `device` (the system default when `None` or not found) and
    /// start capturing.
    pub fn start(source_start_ms: f64, device: Option<&str>) -> Result<VoiceRecorder> {
        let host = cpal::default_host();
        let chosen = device.and_then(|want| host.input_devices().ok()?.find(|d| d.description().map(|n| n.name() == want).unwrap_or(false)));
        let device = chosen.or_else(|| host.default_input_device()).context("no microphone found")?;
        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Microphone".into());
        let config = device.default_input_config().context("the microphone has no usable format")?;
        let channels = config.channels().max(1) as usize;
        let rate = config.sample_rate();
        let capture = Arc::new(Mutex::new(Capture { samples: Vec::with_capacity(rate as usize * 60), sum_sq: 0.0 }));
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let (buf, lvl) = (capture.clone(), level.clone());
        let push = move |frames: &mut dyn Iterator<Item = f32>| {
            let mut peak = 0f32;
            let mut out = buf.lock();
            for v in frames {
                peak = peak.max(v.abs());
                out.sum_sq += (v as f64) * (v as f64);
                out.samples.push(v);
            }
            // fast attack, slow release (as the picker's level meter)
            let prev = f32::from_bits(lvl.load(Ordering::Relaxed));
            let next = if peak > prev { peak } else { prev * 0.85 + peak * 0.15 };
            lvl.store(next.to_bits(), Ordering::Relaxed);
        };
        let err = |e| tracing::warn!("voice-over input error: {e}");
        // downmix to mono: the first channel is the microphone on every common device
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(config.into(), move |data: &[f32], _| push(&mut data.chunks(channels).map(|f| f[0])), err, None)?,
            cpal::SampleFormat::I16 => device.build_input_stream(config.into(), move |data: &[i16], _| push(&mut data.chunks(channels).map(|f| f[0] as f32 / 32768.0)), err, None)?,
            f => anyhow::bail!("unsupported microphone sample format {f:?}"),
        };
        stream.play()?;
        Ok(VoiceRecorder { source_start_ms, device_name, started: Instant::now(), rate, capture, level, _stream: stream })
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// Smoothed input level mapped to 0..1 on a dB scale (-50 dB .. 0 dB).
    pub fn level(&self) -> f32 {
        let a = f32::from_bits(self.level.load(Ordering::Relaxed)).max(1e-6);
        ((20.0 * a.log10() + 50.0) / 50.0).clamp(0.0, 1.0)
    }

    /// What came in so far is below the silence threshold.
    pub fn is_silent(&self) -> bool {
        self.capture.lock().rms() < SILENT_RMS
    }

    /// Stop capturing and write the take to `dir` as a new WAV file, or `None` when the take
    /// was too short to keep.
    pub fn finish(self, dir: &Path) -> Result<Option<Take>> {
        let VoiceRecorder { rate, capture, _stream, .. } = self;
        drop(_stream);
        let capture = std::mem::take(&mut *capture.lock());
        let (rms, samples) = (capture.rms(), capture.samples);
        let duration_ms = samples.len() as f64 * 1000.0 / rate as f64;
        if duration_ms < MIN_MS {
            return Ok(None);
        }
        std::fs::create_dir_all(dir)?;
        let name = format!("voiceover-{}.wav", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
        write_wav(&dir.join(&name), rate, &samples)?;
        Ok(Some(Take { file_name: name, duration_ms, rms }))
    }
}

/// 16-bit PCM mono WAV.
fn write_wav(path: &Path, rate: u32, samples: &[f32]) -> Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, out).with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_click_in_silence_is_still_silent() {
        let mut c = super::Capture::default();
        for i in 0..48_000 {
            let v = if i == 100 { 0.04 } else { 0.0005 };
            c.sum_sq += v * v;
            c.samples.push(v as f32);
        }
        assert!(c.rms() < super::SILENT_RMS);
        let speech = super::Capture { samples: vec![0.05; 10], sum_sq: 10.0 * 0.05 * 0.05 };
        assert!(speech.rms() > super::SILENT_RMS);
    }

    #[test]
    fn wav_header_describes_the_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        super::write_wav(&path, 48_000, &[0.0, 0.5, -1.0]).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(b.len(), 44 + 6);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 6);
        assert_eq!(i16::from_le_bytes(b[46..48].try_into().unwrap()), 16383);
        assert_eq!(i16::from_le_bytes(b[48..50].try_into().unwrap()), -32767);
    }
}
