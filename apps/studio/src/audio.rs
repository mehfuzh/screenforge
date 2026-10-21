//! Preview audio playback: the microphone / system-audio tracks are decoded to a 48 kHz stereo
//! mix in memory and played through cpal, following the output timeline (cuts and speed).
//!
//! Voice-overs are decoded on their own (by file name) and played at their place on the output
//! timeline, so recording a new one does not decode the recording again.
//!
//! The audio callback owns the master clock while playing: the editor reads
//! [`AudioPlayer::position_ms`] every frame so video follows audio.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use sf_engine::audio::{AudioSpan, VoiceOverSpan};
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
    /// Decoded voice-overs by file name, and where they play.
    voice_clips: std::collections::HashMap<String, Arc<AudioClip>>,
    voice_spans: Vec<VoiceOverSpan>,
    playing: bool,
    /// End of the audio handed to the device so far (advances a whole buffer per callback).
    output_ms: f64,
    /// When the last callback ran and `output_ms` at its start: `position_ms` interpolates from
    /// these, so the playhead moves every frame instead of in buffer-sized steps.
    callback_at: Option<std::time::Instant>,
    callback_ms: f64,
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
        let shared = Arc::new(Mutex::new(Shared { tracks: vec![], background: None, spans: vec![], voice_clips: Default::default(), voice_spans: vec![], playing: false, output_ms: 0.0, callback_at: None, callback_ms: 0.0, duration_ms: 0.0, master_volume: 1.0, bg_volume: 0.05, bg_muted: false }));
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
                if s.playing {
                    s.callback_at = Some(std::time::Instant::now());
                    s.callback_ms = s.output_ms;
                }
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
                            for vo in s.voice_spans.iter().filter(|v| !v.muted && v.contains_output(t_out)) {
                                if let Some(clip) = s.voice_clips.get(&vo.file_name) {
                                    let i = clip.frame_at_ms(vo.file_ms_at(t_out));
                                    let ch = clip.channels as usize;
                                    if i < clip.frames() {
                                        let vol = vo.volume as f32 * master;
                                        l += clip.samples[i * ch] * vol;
                                        r += clip.samples[i * ch + (ch - 1).min(1)] * vol;
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
        let audio_cfg = &session.pkg.project.config.audio;
        let mic_filter = sf_media::microphone_filter(&audio_cfg.microphone_audio_preset, audio_cfg.microphone_boost_db());
        let background_path = session.background_audio_path();
        let (shared, generation, pending) = (self.shared.clone(), self.generation.clone(), self.pending.clone());
        self.update_timeline(session);
        self.load_voice_overs(session);
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
                let filter = if kind == AudioTrack::Microphone { mic_filter.as_deref() } else { None };
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

    /// Decode the voice-overs not decoded yet (a new take) on a background thread.
    pub fn load_voice_overs(&self, session: &sf_session::Session) {
        use std::sync::atomic::Ordering;
        let missing: Vec<(String, std::path::PathBuf)> = {
            let s = self.shared.lock();
            let mut names: Vec<String> = session.pkg.project.scenes[0].voice_overs.iter().map(|v| v.file_name.clone()).filter(|n| !n.is_empty() && !s.voice_clips.contains_key(n)).collect();
            names.sort_unstable();
            names.dedup();
            names.into_iter().map(|n| (session.voice_over_path(&n), n)).filter(|(p, _)| p.is_file()).map(|(p, n)| (n, p)).collect()
        };
        if missing.is_empty() {
            return;
        }
        let (shared, generation, pending) = (self.shared.clone(), self.generation.clone(), self.pending.clone());
        pending.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            for (name, path) in missing {
                match sf_media::decode_audio(&path, RATE, 2) {
                    Ok(clip) => {
                        shared.lock().voice_clips.insert(name, Arc::new(clip));
                    }
                    Err(e) => tracing::warn!("voice-over decode failed for {}: {e}", path.display()),
                }
            }
            generation.fetch_add(1, Ordering::Relaxed);
            pending.fetch_sub(1, Ordering::Relaxed);
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
        s.voice_spans = session.voice_over_spans();
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

    /// Waveform peaks of a voice-over file, `buckets` across the whole take.
    pub fn voice_peaks(&self, file_name: &str, buckets: usize) -> Option<Vec<f32>> {
        self.shared.lock().voice_clips.get(file_name).map(|c| c.peaks(buckets))
    }

    /// Length of a decoded voice-over file (ms).
    pub fn voice_duration_ms(&self, file_name: &str) -> Option<f64> {
        self.shared.lock().voice_clips.get(file_name).map(|c| c.duration_ms())
    }

    pub fn track_duration_ms(&self, kind: &AudioTrack) -> Option<f64> {
        let s = self.shared.lock();
        s.tracks.iter().find(|t| &t.kind == kind).map(|t| t.clip.duration_ms())
    }

    pub fn play_from(&self, output_ms: f64) {
        let mut s = self.shared.lock();
        s.output_ms = output_ms;
        s.callback_at = None;
        s.playing = true;
    }

    pub fn pause(&self) {
        self.shared.lock().playing = false;
    }

    pub fn seek(&self, output_ms: f64) {
        let mut s = self.shared.lock();
        s.output_ms = output_ms;
        s.callback_at = None;
    }

    pub fn is_playing(&self) -> bool {
        self.shared.lock().playing
    }

    /// Playback position: the start of the last buffer plus the time since it was requested,
    /// never past the audio handed over so far.
    pub fn position_ms(&self) -> f64 {
        let s = self.shared.lock();
        match s.callback_at {
            Some(at) if s.playing => (s.callback_ms + at.elapsed().as_secs_f64() * 1000.0).min(s.output_ms),
            _ => s.output_ms,
        }
    }

    /// Something to play drives the clock: recorded tracks or voice-overs.
    pub fn has_audio(&self) -> bool {
        let s = self.shared.lock();
        !s.tracks.is_empty() || !s.voice_clips.is_empty()
    }

    /// Master volume (0 while a voice-over is being recorded, so the mic does not pick it up).
    pub fn set_volume(&self, v: f32) {
        self.shared.lock().master_volume = v;
    }
}
