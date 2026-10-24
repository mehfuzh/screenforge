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
use sf_recording::discover::AudioDevice;

/// Recordings shorter than this are dropped (a slip of the button).
pub const MIN_MS: f64 = 300.0;

/// Level a 50 ms stretch must reach to count as voice (-45 dBFS RMS). A room heard through a
/// working microphone sits near -65 dB, an unplugged or muted one lower still, and speech
/// reaches -40..-20 dB even with the input volume turned well down.
pub const VOICE_RMS: f64 = 0.005_62;

/// Stretches of voice a take needs before it has sound (200 ms in all). Counting stretches
/// rather than the level of the whole take keeps pauses from making quiet speech "silent",
/// while a single click or bump fills one stretch at most.
pub const MIN_VOICE_WINDOWS: u32 = 4;

/// A sample this close to full scale was cut off by the microphone's converter: the input is
/// too hot, and no boost or filter can put the top of the wave back.
pub const CLIP_LEVEL: f32 = 0.999;

/// Clipped samples a take needs before it counts as clipped (a lone full-scale click does not).
pub const MIN_CLIPPED: u32 = 16;

/// How long the meter stays red after the input clipped.
const CLIP_HOLD: std::time::Duration = std::time::Duration::from_millis(1500);

/// Samples captured so far, cut into 50 ms stretches to count the ones with voice in them.
#[derive(Default)]
struct Capture {
    samples: Vec<f32>,
    /// Samples per stretch (50 ms at the device rate).
    window: usize,
    win_sum_sq: f64,
    win_len: usize,
    voice_windows: u32,
    clipped: u32,
    /// When the input last clipped, for the meter.
    clipped_at: Option<Instant>,
}

impl Capture {
    fn new(rate: u32) -> Capture {
        Capture { samples: Vec::with_capacity(rate as usize * 60), window: (rate as usize / 20).max(1), ..Default::default() }
    }

    fn push(&mut self, v: f32) {
        self.samples.push(v);
        if v.abs() >= CLIP_LEVEL {
            self.clipped += 1;
            if self.clipped >= MIN_CLIPPED {
                self.clipped_at = Some(Instant::now());
            }
        }
        self.win_sum_sq += (v as f64) * (v as f64);
        self.win_len += 1;
        if self.win_len >= self.window {
            if (self.win_sum_sq / self.win_len as f64).sqrt() >= VOICE_RMS {
                self.voice_windows += 1;
            }
            self.win_sum_sq = 0.0;
            self.win_len = 0;
        }
    }

    fn is_silent(&self) -> bool {
        self.voice_windows < MIN_VOICE_WINDOWS
    }

    fn is_clipped(&self) -> bool {
        self.clipped >= MIN_CLIPPED
    }
}

/// Microphones for the menu: the list the recording toolbar offers (on Linux every PipeWire
/// source by name, plus a `default` entry saying which source the system default is).
pub fn microphones() -> Vec<AudioDevice> {
    sf_recording::discover::microphones()
}

/// Label of the "System default" row. Linux lists its own `default` entry, already named after
/// the source it routes to; elsewhere it is the device flagged as the default.
pub fn default_label(mics: &[AudioDevice]) -> String {
    match mics.iter().find(|m| m.id == "default") {
        Some(m) => m.name.clone(),
        None => match mics.iter().find(|m| m.is_default) {
            Some(m) => format!("System default ({})", m.name),
            None => "System default".into(),
        },
    }
}

/// A saved take.
pub struct Take {
    pub file_name: String,
    pub duration_ms: f64,
    /// Nothing in the take was loud enough to be a voice (see [`MIN_VOICE_WINDOWS`]).
    pub silent: bool,
    /// The microphone was too loud and cut off the peaks (see [`MIN_CLIPPED`]).
    pub clipped: bool,
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
    /// Open microphone `mic` from [`microphones`] (the system default when `None` or no longer
    /// there) and start capturing; `device_name` is what the status line calls it.
    pub fn start(source_start_ms: f64, mic: Option<&AudioDevice>, device_name: String) -> Result<VoiceRecorder> {
        let host = cpal::default_host();
        // Linux: a `pw:` id opens the default PCM pointed at that PipeWire source; the guard
        // clears the target again when this function returns, after the stream is open
        let (open_as, _pipewire_target) = sf_recording::discover::alsa_selector(mic.map_or("default", |m| m.id.as_str()));
        let chosen = mic.and_then(|m| host.input_devices().ok()?.find(|d| d.id().map(|i| i.id() == open_as).unwrap_or(false) || d.description().map(|n| n.name() == m.name).unwrap_or(false)));
        let device = chosen.or_else(|| host.default_input_device()).context("no microphone found")?;
        let config = device.default_input_config().context("the microphone has no usable format")?;
        let channels = config.channels().max(1) as usize;
        let rate = config.sample_rate();
        let capture = Arc::new(Mutex::new(Capture::new(rate)));
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let (buf, lvl) = (capture.clone(), level.clone());
        let push = move |frames: &mut dyn Iterator<Item = f32>| {
            let mut peak = 0f32;
            let mut out = buf.lock();
            for v in frames {
                peak = peak.max(v.abs());
                out.push(v);
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
        self.capture.lock().is_silent()
    }

    /// The input clipped within the last moment (the meter shows it in red).
    pub fn is_clipping(&self) -> bool {
        self.capture.lock().clipped_at.is_some_and(|t| t.elapsed() < CLIP_HOLD)
    }

    /// Stop capturing and write the take to `dir` as a new WAV file, or `None` when the take
    /// was too short to keep.
    pub fn finish(self, dir: &Path) -> Result<Option<Take>> {
        let VoiceRecorder { rate, capture, _stream, .. } = self;
        drop(_stream);
        let capture = std::mem::take(&mut *capture.lock());
        let (silent, clipped) = (capture.is_silent(), capture.is_clipped());
        let samples = capture.samples;
        let duration_ms = samples.len() as f64 * 1000.0 / rate as f64;
        if duration_ms < MIN_MS {
            return Ok(None);
        }
        std::fs::create_dir_all(dir)?;
        let name = format!("voiceover-{}.wav", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
        write_wav(&dir.join(&name), rate, &samples)?;
        Ok(Some(Take { file_name: name, duration_ms, silent, clipped }))
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
    fn take(samples: impl Iterator<Item = f32>) -> super::Capture {
        let mut c = super::Capture::new(48_000);
        samples.for_each(|v| c.push(v));
        c
    }

    #[test]
    fn a_click_in_silence_is_still_silent() {
        // room tone of a working microphone (-66 dB) with one loud click
        let c = take((0..48_000 * 3).map(|i| if i == 100 { 0.5 } else if i % 2 == 0 { 0.0005 } else { -0.0005 }));
        assert!(c.is_silent());
    }

    #[test]
    fn quiet_speech_between_pauses_has_sound() {
        // half a second of speech at -40 dB inside three seconds of room tone: about -48 dB
        // over the whole take, which the old whole-take level called silent
        let c = take((0..48_000 * 3).map(|i| {
            let a = if (48_000..72_000).contains(&i) { 0.014 } else { 0.0005 };
            if i % 2 == 0 { a } else { -a }
        }));
        assert!(!c.is_silent());
    }

    #[test]
    fn speech_at_full_scale_is_clipped_but_a_click_is_not() {
        let click = take((0..48_000).map(|i| if i == 100 { 1.0 } else { 0.01 }));
        assert!(!click.is_clipped());
        // a loud word flattened at full scale for 2 ms
        let hot = take((0..48_000).map(|i| if (1_000..1_096).contains(&i) { 1.0 } else { 0.3 }));
        assert!(hot.is_clipped());
        assert!(hot.clipped_at.is_some());
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
