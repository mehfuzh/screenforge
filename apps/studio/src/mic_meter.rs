//! Live microphone level for the picker: opens the selected input device with cpal and keeps
//! a smoothed peak level (0..1) the UI can draw while you talk.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct MicMeter {
    pub device_name: String,
    level: Arc<AtomicU32>,
    _stream: cpal::Stream,
}

impl MicMeter {
    /// Open the input device whose id or name matches `name` (falls back to the default input).
    pub fn start(name: &str) -> Option<MicMeter> {
        let host = cpal::default_host();
        // Linux: a `pw:` id becomes the default PCM pointed at that PipeWire source; the
        // guard clears the target again when this function returns
        let (open_as, _pipewire_target) = sf_recording::discover::alsa_selector(name);
        let device = host
            .input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.id().map(|i| i.id() == open_as).unwrap_or(false) || d.description().map(|n| n.name() == open_as).unwrap_or(false)))
            .or_else(|| host.default_input_device())?;
        let config = device.default_input_config().ok()?;
        let channels = config.channels() as usize;
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let lvl = level.clone();
        let stream = device
            .build_input_stream(
                config.into(),
                move |data: &[f32], _| {
                    let mut peak = 0f32;
                    for frame in data.chunks(channels.max(1)) {
                        peak = peak.max(frame[0].abs());
                    }
                    // fast attack, slow release
                    let prev = f32::from_bits(lvl.load(Ordering::Relaxed));
                    let next = if peak > prev { peak } else { prev * 0.85 + peak * 0.15 };
                    lvl.store(next.to_bits(), Ordering::Relaxed);
                },
                |e| tracing::warn!("mic meter stream error: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(MicMeter { device_name: name.to_string(), level, _stream: stream })
    }

    /// Smoothed level mapped to 0..1 on a dB scale (-50 dB .. 0 dB), so speech fills the icon.
    pub fn level(&self) -> f32 {
        let a = f32::from_bits(self.level.load(Ordering::Relaxed)).max(1e-6);
        ((20.0 * a.log10() + 50.0) / 50.0).clamp(0.0, 1.0)
    }
}
