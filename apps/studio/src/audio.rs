//! Preview audio playback: the microphone / system-audio tracks are decoded to a 48 kHz stereo
//! mix in memory and played through cpal, following the output timeline (cuts and speed).
//!
//! The audio callback owns the master clock while playing: the editor reads
//! [`AudioPlayer::position_ms`] every frame so video follows audio.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use sf_engine::audio::AudioSpan;
use sf_media::AudioClip;
use sf_project::audio_clips::AudioTrack;

const RATE: u32 = 48_000;

#[derive(Clone)]
struct Track {
    kind: AudioTrack,
    clip: Arc<AudioClip>,
}

struct Background {
    clip: Arc<AudioClip>,
    volume: f32,
    muted: bool,
}

struct Shared {
    tracks: Vec<Track>,
    background: Option<Background>,
    spans: Vec<AudioSpan>,
    playing: bool,
    output_ms: f64,
    duration_ms: f64,
    master_volume: f32,
    /// Background music level from the project (applied when a decode finishes too).
    bg_volume: f32,
    bg_muted: bool,
}

pub struct AudioPlayer {
    /// Bumped when a `load` starts and again when its decode lands, so cached waveforms know
    /// to refresh (filter or track changes) and a superseded decode is dropped.
    pub generation: Arc<std::sync::atomic::AtomicU64>,
    /// Decodes still running (the editor keeps repainting so the waveform shows up).
    pending: Arc<std::sync::atomic::AtomicUsize>,
    _stream: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    pub available: bool,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared { tracks: vec![], background: None, spans: vec![], playing: false, output_ms: 0.0, duration_ms: 0.0, master_volume: 1.0, bg_volume: 0.05, bg_muted: false }));
        let stream = Self::open_stream(shared.clone());
        if let Err(e) = &stream {
            tracing::warn!("audio output unavailable: {e}");
        }
        AudioPlayer { generation: Arc::new(std::sync::atomic::AtomicU64::new(0)), pending: Default::default(), available: stream.is_ok(), _stream: stream.ok(), shared }
    }

    fn open_stream(shared: Arc<Mutex<Shared>>) -> anyhow::Result<cpal::Stream> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device"))?;
        let config = cpal::StreamConfig { channels: 2, sample_rate: RATE, buffer_size: cpal::BufferSize::Default };
        let stream = device.build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let mut s = shared.lock();
                let step_ms = 1000.0 / RATE as f64;
                for frame in data.chunks_mut(2) {
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    if s.playing {
                        if s.output_ms >= s.duration_ms {
                            s.playing = false;
                        } else {
                            let t_out = s.output_ms;
                            let master = s.master_volume;
                            for span in s.spans.iter().filter(|sp| !sp.muted && sp.contains_output(t_out)) {
                                let src_ms = span.output_to_source(t_out);
                                let vol = span.volume as f32 * master;
                                for t in s.tracks.iter().filter(|t| t.kind == span.track) {
                                    let i = t.clip.frame_at_ms(src_ms);
                                    let ch = t.clip.channels as usize;
                                    if i < t.clip.frames() {
                                        l += t.clip.samples[i * ch] * vol;
                                        r += t.clip.samples[i * ch + (ch - 1).min(1)] * vol;
                                    }
                                }
                            }
                            if let Some(bg) = &s.background {
                                if !bg.muted && bg.clip.frames() > 0 {
                                    let i = bg.clip.frame_at_ms(t_out) % bg.clip.frames();
                                    let ch = bg.clip.channels as usize;
                                    l += bg.clip.samples[i * ch] * bg.volume * master;
                                    r += bg.clip.samples[i * ch + (ch - 1).min(1)] * bg.volume * master;
                                }
                            }
                            s.output_ms += step_ms;
                        }
                    }
                    frame[0] = l.clamp(-1.0, 1.0);
                    if frame.len() > 1 {
                        frame[1] = r.clamp(-1.0, 1.0);
                    }
                }
            },
            |e| tracing::warn!("audio stream error: {e}"),
            None,
        )?;
        stream.play()?;
        Ok(stream)
    }

    /// Decode the session's audio tracks on a background thread (long recordings with a voice
    /// filter take seconds); the previous tracks keep playing until the new ones land.
    pub fn load(&self, session: &sf_session::Session) {
        use std::sync::atomic::Ordering;
        let my_gen = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let sources: Vec<(std::path::PathBuf, AudioTrack)> = session
            .source
            .microphone
            .iter()
            .map(|p| (p.clone(), AudioTrack::Microphone))
            .chain(session.source.system_audio.iter().map(|p| (p.clone(), AudioTrack::SystemAudio)))
            .collect();
        let mic_filter = sf_media::microphone_filter(&session.pkg.project.config.audio.microphone_audio_preset);
        let background_path = session.background_audio_path();
        let (shared, generation, pending) = (self.shared.clone(), self.generation.clone(), self.pending.clone());
        self.update_timeline(session);
        pending.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            struct Done(Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let _done = Done(pending);
            let mut tracks = vec![];
            for (path, kind) in sources {
                let filter = if kind == AudioTrack::Microphone { mic_filter } else { None };
                match sf_media::decode_audio_filtered(&path, RATE, 2, filter) {
                    Ok(clip) => tracks.push(Track { kind, clip: Arc::new(clip) }),
                    Err(e) => tracing::warn!("audio decode failed for {}: {e}", path.display()),
                }
            }
            let background = background_path.and_then(|p| match sf_media::decode_audio(&p, RATE, 2) {
                Ok(clip) => Some(Arc::new(clip)),
                Err(e) => {
                    tracing::warn!("background audio decode failed for {}: {e}", p.display());
                    None
                }
            });
            let mut s = shared.lock();
            // a newer load started meanwhile: its result wins
            if generation.load(Ordering::Relaxed) != my_gen {
                return;
            }
            s.tracks = tracks;
            let (volume, muted) = (s.bg_volume, s.bg_muted);
            s.background = background.map(|clip| Background { clip, volume, muted });
            generation.fetch_add(1, Ordering::Relaxed);
        });
    }

    /// A `load` is still decoding.
    pub fn is_loading(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    /// Tracks present in the recording (for building the audio timeline).
    pub fn available_tracks(session: &sf_session::Session) -> Vec<AudioTrack> {
        let mut t = vec![];
        if !session.source.microphone.is_empty() {
            t.push(AudioTrack::Microphone);
        }
        if !session.source.system_audio.is_empty() {
            t.push(AudioTrack::SystemAudio);
        }
        t
    }

    /// Refresh cut/speed/volume info after an edit (cheap; no decoding).
    pub fn update_timeline(&self, session: &sf_session::Session) {
        let cfg = &session.pkg.project.config;
        let scene = &session.pkg.project.scenes[0];
        let spans = sf_engine::audio::audio_spans(scene, &Self::available_tracks(session), cfg.audio.volume, cfg.audio.mute_microphone, cfg.audio.mute_system_audio);
        let mut s = self.shared.lock();
        s.spans = spans;
        s.duration_ms = session.duration_ms();
        let (volume, muted) = (cfg.audio.background_audio_volume as f32, cfg.audio.mute_background_audio);
        s.bg_volume = volume;
        s.bg_muted = muted;
        if let Some(bg) = &mut s.background {
            bg.volume = volume;
            bg.muted = muted;
        }
    }

    /// Waveform peaks of a track over the whole recording (source time), `buckets` wide.
    pub fn peaks(&self, kind: &AudioTrack, buckets: usize) -> Option<Vec<f32>> {
        let s = self.shared.lock();
        let t = s.tracks.iter().find(|t| &t.kind == kind)?;
        Some(t.clip.peaks(buckets))
    }

    pub fn track_duration_ms(&self, kind: &AudioTrack) -> Option<f64> {
        let s = self.shared.lock();
        s.tracks.iter().find(|t| &t.kind == kind).map(|t| t.clip.duration_ms())
    }

    pub fn play_from(&self, output_ms: f64) {
        let mut s = self.shared.lock();
        s.output_ms = output_ms;
        s.playing = true;
    }

    pub fn pause(&self) {
        self.shared.lock().playing = false;
    }

    pub fn seek(&self, output_ms: f64) {
        self.shared.lock().output_ms = output_ms;
    }

    pub fn is_playing(&self) -> bool {
        self.shared.lock().playing
    }

    pub fn position_ms(&self) -> f64 {
        self.shared.lock().output_ms
    }

    pub fn has_audio(&self) -> bool {
        !self.shared.lock().tracks.is_empty()
    }

    #[allow(dead_code)]
    pub fn set_volume(&self, v: f32) {
        self.shared.lock().master_volume = v;
    }
}
