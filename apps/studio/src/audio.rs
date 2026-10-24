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
    /// Decoded with this boost (dB): clips with different boosts play different decodes.
    boost_db: f64,
    clip: Arc<AudioClip>,
}

/// Cache key of a voice-over file decoded at a boost: takes cut from one recording share a
/// file but can each have their own boost.
pub fn voice_key(file_name: &str, boost_db: f64) -> String {
    if boost_db > 0.0 {
        format!("{file_name}@{boost_db:.1}dB")
    } else {
        file_name.to_string()
    }
}

/// A music clip as the mixer plays it: the clip, its effective fades, its track and level.
struct MusicPlay {
    clip: sf_project::MusicClip,
    fades: (f64, f64),
    track: String,
    level: f32,
}

struct Shared {
    tracks: Vec<Track>,
    spans: Vec<AudioSpan>,
    /// Decoded music tracks by project name (each music clip can have its own).
    music_tracks: std::collections::HashMap<String, Arc<AudioClip>>,
    /// Decoded voice-overs by file name, and where they play.
    voice_clips: std::collections::HashMap<String, Arc<AudioClip>>,
    /// The audio mode `voice_clips` were decoded with (each take adds its own boost).
    voice_filter: Option<String>,
    voice_spans: Vec<VoiceOverSpan>,
    /// Where the background music plays (it loops inside each clip).
    music: Vec<MusicPlay>,
    playing: bool,
    /// End of the audio handed to the device so far (advances a whole buffer per callback).
    output_ms: f64,
    /// When the last callback ran and `output_ms` at its start: `position_ms` interpolates from
    /// these, so the playhead moves every frame instead of in buffer-sized steps.
    callback_at: Option<std::time::Instant>,
    callback_ms: f64,
    duration_ms: f64,
    master_volume: f32,
}

pub struct AudioPlayer {
    /// Bumped when a `load` starts and whenever a decode lands (tracks, music or a voice-over),
    /// so cached waveforms know to refresh.
    pub generation: Arc<std::sync::atomic::AtomicU64>,
    /// Bumped by every `load`: a decode whose number is no longer the latest is dropped. Kept
    /// apart from `generation`, which voice-over decodes bump too: sharing one counter made a
    /// voice-over that finished first discard the recording's and the music's decode.
    load_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Decodes still running (the editor keeps repainting so the waveform shows up).
    pending: Arc<std::sync::atomic::AtomicUsize>,
    _stream: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    pub available: bool,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared { tracks: vec![], spans: vec![], music_tracks: Default::default(), voice_clips: Default::default(), voice_filter: None, voice_spans: vec![], music: vec![], playing: false, output_ms: 0.0, callback_at: None, callback_ms: 0.0, duration_ms: 0.0, master_volume: 1.0 }));
        let stream = Self::open_stream(shared.clone());
        if let Err(e) = &stream {
            tracing::warn!("audio output unavailable: {e}");
        }
        AudioPlayer { generation: Arc::new(std::sync::atomic::AtomicU64::new(0)), load_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)), pending: Default::default(), available: stream.is_ok(), _stream: stream.ok(), shared }
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
                                // the decode at the span's boost; until it lands, any of the track
                                let same = |t: &&Track| t.kind == span.track && (t.boost_db - span.boost_db).abs() < 0.05;
                                let boost = if s.tracks.iter().any(|t| same(&t)) { span.boost_db } else { s.tracks.iter().find(|t| t.kind == span.track).map_or(0.0, |t| t.boost_db) };
                                for t in s.tracks.iter().filter(|t| t.kind == span.track && (t.boost_db - boost).abs() < 0.05) {
                                    let i = t.clip.frame_at_ms(src_ms);
                                    let ch = t.clip.channels as usize;
                                    if i < t.clip.frames() {
                                        l += t.clip.samples[i * ch] * vol;
                                        r += t.clip.samples[i * ch + (ch - 1).min(1)] * vol;
                                    }
                                }
                            }
                            for vo in s.voice_spans.iter().filter(|v| !v.muted && v.contains_output(t_out)) {
                                if let Some(clip) = s.voice_clips.get(&voice_key(&vo.file_name, vo.boost_db)) {
                                    let i = clip.frame_at_ms(vo.file_ms_at(t_out));
                                    let ch = clip.channels as usize;
                                    if i < clip.frames() {
                                        let vol = vo.volume as f32 * master;
                                        l += clip.samples[i * ch] * vol;
                                        r += clip.samples[i * ch + (ch - 1).min(1)] * vol;
                                    }
                                }
                            }
                            for m in s.music.iter().filter(|m| m.level > 0.0 && m.clip.contains_output(t_out)) {
                                let Some(track) = s.music_tracks.get(&m.track).filter(|t| t.frames() > 0) else { continue };
                                let i = track.frame_at_ms(m.clip.file_ms_at(t_out).max(0.0)) % track.frames();
                                let ch = track.channels as usize;
                                let vol = m.level * master * sf_project::music_clips::fade_gain(&m.clip, m.fades, t_out) as f32;
                                l += track.samples[i * ch] * vol;
                                r += track.samples[i * ch + (ch - 1).min(1)] * vol;
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
        let my_load = self.load_seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.generation.fetch_add(1, Ordering::Relaxed);
        let sources: Vec<(std::path::PathBuf, AudioTrack)> = session
            .source
            .microphone
            .iter()
            .map(|p| (p.clone(), AudioTrack::Microphone))
            .chain(session.source.system_audio.iter().map(|p| (p.clone(), AudioTrack::SystemAudio)))
            .collect();
        let audio_cfg = &session.pkg.project.config.audio;
        // each track at no boost (the waveform) and at every boost its clips use
        let spans = sf_engine::audio::audio_spans(&session.pkg.project.scenes[0], &Self::available_tracks(session), audio_cfg);
        let preset = audio_cfg.microphone_audio_preset.clone();
        let decodes: Vec<(std::path::PathBuf, AudioTrack, f64, Option<String>)> = sources
            .into_iter()
            .flat_map(|(path, kind)| {
                let mut boosts = vec![0.0];
                for sp in spans.iter().filter(|sp| sp.track == kind) {
                    if !boosts.iter().any(|b: &f64| (b - sp.boost_db).abs() < 0.05) {
                        boosts.push(sp.boost_db);
                    }
                }
                let preset = if kind == AudioTrack::Microphone { preset.clone() } else { "voice".to_string() };
                boosts.into_iter().map(move |b| (path.clone(), kind.clone(), b, sf_media::microphone_filter(&preset, b)))
            })
            .collect();
        let (shared, generation, load_seq, pending) = (self.shared.clone(), self.generation.clone(), self.load_seq.clone(), self.pending.clone());
        self.update_timeline(session);
        self.load_voice_overs(session);
        self.load_music(session);
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
            for (path, kind, boost_db, filter) in decodes {
                match sf_media::decode_audio_filtered(&path, RATE, 2, filter.as_deref()) {
                    Ok(clip) => tracks.push(Track { kind, boost_db, clip: Arc::new(clip) }),
                    Err(e) => tracing::warn!("audio decode failed for {}: {e}", path.display()),
                }
            }
            let mut s = shared.lock();
            // a newer load started meanwhile: its result wins
            if load_seq.load(Ordering::Relaxed) != my_load {
                return;
            }
            s.tracks = tracks;
            generation.fetch_add(1, Ordering::Relaxed);
        });
    }

    /// Decode the voice-overs not decoded yet (a new take) on a background thread.
    pub fn load_voice_overs(&self, session: &sf_session::Session) {
        use std::sync::atomic::Ordering;
        let preset = session.pkg.project.config.audio.microphone_audio_preset.clone();
        // (cache key, file, filter): a file once per boost its takes use
        let missing: Vec<(String, std::path::PathBuf, Option<String>)> = {
            let mut s = self.shared.lock();
            // the audio mode changed: decode every take again
            if s.voice_filter.as_deref() != Some(preset.as_str()) {
                s.voice_clips.clear();
                s.voice_filter = Some(preset.clone());
            }
            let mut wanted: Vec<(String, String, f64)> = session.pkg.project.scenes[0].voice_overs.iter().filter(|v| !v.file_name.is_empty()).map(|v| (voice_key(&v.file_name, v.boost()), v.file_name.clone(), v.boost())).filter(|(k, _, _)| !s.voice_clips.contains_key(k)).collect();
            wanted.sort_by(|a, b| a.0.cmp(&b.0));
            wanted.dedup_by(|a, b| a.0 == b.0);
            wanted.into_iter().map(|(k, n, boost)| (k, session.voice_over_path(&n), sf_media::microphone_filter(&preset, boost))).filter(|(_, p, _)| p.is_file()).collect()
        };
        if missing.is_empty() {
            return;
        }
        let (shared, generation, pending) = (self.shared.clone(), self.generation.clone(), self.pending.clone());
        pending.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            for (key, path, filter) in missing {
                match sf_media::decode_audio_filtered(&path, RATE, 2, filter.as_deref()) {
                    Ok(clip) => {
                        shared.lock().voice_clips.insert(key, Arc::new(clip));
                    }
                    Err(e) => tracing::warn!("voice-over decode failed for {}: {e}", path.display()),
                }
            }
            generation.fetch_add(1, Ordering::Relaxed);
            pending.fetch_sub(1, Ordering::Relaxed);
        });
    }

    /// Decode the music tracks the music clips use that are not decoded yet (a newly chosen
    /// track) on a background thread.
    pub fn load_music(&self, session: &sf_session::Session) {
        use std::sync::atomic::Ordering;
        let audio = &session.pkg.project.config.audio;
        let missing: Vec<(String, std::path::PathBuf)> = {
            let s = self.shared.lock();
            let mut names: Vec<String> = sf_engine::lanes::music_clips(&session.pkg.project, &session.timeline).iter().filter_map(|c| c.track(audio).map(str::to_string)).filter(|n| !s.music_tracks.contains_key(n)).collect();
            names.sort_unstable();
            names.dedup();
            names.into_iter().filter_map(|n| session.music_path(&n).map(|p| (n, p))).collect()
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
                        shared.lock().music_tracks.insert(name, Arc::new(clip));
                    }
                    Err(e) => tracing::warn!("music decode failed for {}: {e}", path.display()),
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
        let spans = sf_engine::audio::audio_spans(scene, &Self::available_tracks(session), &cfg.audio);
        let mut s = self.shared.lock();
        s.spans = spans;
        s.voice_spans = session.voice_over_spans();
        s.duration_ms = session.duration_ms();
        let music = sf_engine::lanes::music_clips(&session.pkg.project, &session.timeline);
        let fades = sf_project::music_clips::effective_fades(&music);
        s.music = music
            .into_iter()
            .zip(fades)
            .filter_map(|(clip, fades)| {
                let track = clip.track(&cfg.audio)?.to_string();
                let level = clip.level(&cfg.audio) as f32;
                Some(MusicPlay { clip, fades, track, level })
            })
            .collect();
        drop(s);
        // a part may have just been given a track that is not decoded yet, and a take (one
        // the agent narrated) or a boost may be new
        self.load_music(session);
        self.load_voice_overs(session);
    }

    /// Waveform peaks of a track over the whole recording (source time), `buckets` wide.
    pub fn peaks(&self, kind: &AudioTrack, buckets: usize) -> Option<Vec<f32>> {
        let s = self.shared.lock();
        let t = s.tracks.iter().find(|t| &t.kind == kind && t.boost_db == 0.0)?;
        Some(t.clip.peaks(buckets))
    }

    /// Waveform peaks of a voice-over file at a boost (`voice_key`), across the whole take.
    pub fn voice_peaks(&self, key: &str, buckets: usize) -> Option<Vec<f32>> {
        self.shared.lock().voice_clips.get(key).map(|c| c.peaks(buckets))
    }

    /// Waveform peaks of music track `name`, `buckets` across the whole track, and its length
    /// (ms); `None` until it is decoded.
    pub fn music_peaks(&self, name: &str, buckets: usize) -> Option<(Vec<f32>, f64)> {
        let s = self.shared.lock();
        let t = s.music_tracks.get(name)?;
        Some((t.peaks(buckets), t.duration_ms()))
    }

    /// Length of a decoded voice-over file (ms), by `voice_key`.
    pub fn voice_duration_ms(&self, key: &str) -> Option<f64> {
        self.shared.lock().voice_clips.get(key).map(|c| c.duration_ms())
    }

    pub fn track_duration_ms(&self, kind: &AudioTrack) -> Option<f64> {
        let s = self.shared.lock();
        s.tracks.iter().find(|t| &t.kind == kind && t.boost_db == 0.0).map(|t| t.clip.duration_ms())
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
        !s.tracks.is_empty() || !s.voice_clips.is_empty() || !s.music_tracks.is_empty()
    }

    /// Master volume (0 while a voice-over is being recorded, so the mic does not pick it up).
    pub fn set_volume(&self, v: f32) {
        self.shared.lock().master_volume = v;
    }
}
