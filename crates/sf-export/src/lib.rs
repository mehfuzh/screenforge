//! Export loop: output time → `FrameDesc` → GPU → readback → encoder, plus audio mixing.

use std::path::PathBuf;

use anyhow::Result;
use sf_media::{AudioMixInput, VideoCodec, VideoWriter, VideoWriterOptions};
use sf_render::Compositor;
use sf_session::Session;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub output: PathBuf,
    pub fps: f64,
    /// Output height cap: 720, 1080, 1440, 2160 …
    pub max_height: u32,
    pub codec: VideoCodec,
    pub quality: f64,
    pub include_audio: bool,
    /// Optional output-time range (ms).
    pub range_ms: Option<(f64, f64)>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { output: "export.mp4".into(), fps: 60.0, max_height: 1080, codec: VideoCodec::H264, quality: 0.85, include_audio: true, range_ms: None }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub frame: u64,
    pub total_frames: u64,
    pub output_ms: f64,
}

/// Build the ffmpeg audio inputs reproducing the audio timeline (linked to the video slices,
/// or the detached ScreenForge audio clips) for every microphone / system-audio session, plus
/// the voice-overs and the background music.
pub fn audio_inputs(session: &Session) -> Vec<AudioMixInput> {
    use sf_project::audio_clips::AudioTrack;
    let cfg = &session.pkg.project.config;
    let scene = &session.pkg.project.scenes[0];
    let mut tracks = vec![];
    if !session.source.microphone.is_empty() {
        tracks.push(AudioTrack::Microphone);
    }
    if !session.source.system_audio.is_empty() {
        tracks.push(AudioTrack::SystemAudio);
    }
    let spans = sf_engine::audio::audio_spans(scene, &tracks, &cfg.audio);
    let mut inputs = vec![];
    for span in spans {
        if span.muted || span.volume <= 0.0 || span.output_duration_ms() <= 1.0 {
            continue;
        }
        let paths: &[std::path::PathBuf] = match span.track {
            AudioTrack::Microphone => &session.source.microphone,
            AudioTrack::SystemAudio => &session.source.system_audio,
        };
        for path in paths {
            inputs.push(AudioMixInput {
                path: path.clone(),
                volume: span.volume,
                delay_ms: span.output_start_ms,
                start_ms: Some(span.source_start_ms),
                end_ms: Some(span.source_end_ms),
                tempo: span.speed,
                loop_input: false,
                filter: match span.track {
                    AudioTrack::Microphone => sf_media::microphone_filter(&cfg.audio.microphone_audio_preset, span.boost_db),
                    AudioTrack::SystemAudio => sf_media::microphone_filter("voice", span.boost_db),
                },
            });
        }
    }
    for vo in session.voice_over_spans() {
        let path = session.voice_over_path(&vo.file_name);
        if vo.muted || vo.volume <= 0.0 || !path.is_file() {
            continue;
        }
        let dur = vo.output_duration_ms();
        // a voice-over is a microphone take: the microphone's audio mode, then the take's boost
        let filter = sf_media::microphone_filter(&cfg.audio.microphone_audio_preset, vo.boost_db);
        inputs.push(AudioMixInput { path, volume: vo.volume, delay_ms: vo.output_start_ms, start_ms: Some(vo.file_offset_ms), end_ms: Some(vo.file_offset_ms + dur), tempo: 1.0, loop_input: false, filter });
    }
    // one looping input per music clip (each can have its own track and level), started at
    // its place in the track and cut to its length (`atrim` after the loop, so a clip longer
    // than the track keeps going)
    let clips = sf_engine::lanes::music_clips(&session.pkg.project, &session.timeline);
    let fades = sf_project::music_clips::effective_fades(&clips);
    let mut track_ms: std::collections::HashMap<std::path::PathBuf, Option<f64>> = Default::default();
    for (m, (fade_in, fade_out)) in clips.into_iter().zip(fades) {
        let level = m.level(&cfg.audio);
        let dur = m.duration_ms.min(session.duration_ms() - m.output_start_ms);
        let Some(path) = m.track(&cfg.audio).and_then(|n| session.music_path(n)) else { continue };
        if level <= 0.0 || dur <= 1.0 {
            continue;
        }
        let len = *track_ms.entry(path.clone()).or_insert_with(|| sf_media::probe(&path).ok().and_then(|i| i.duration_ms()).filter(|d| *d > 0.0));
        let offset = len.map(|t| m.file_offset_ms.rem_euclid(t)).unwrap_or(0.0);
        // fades in the clip's own time (it starts at 0 after the trim); a fade-out runs to the
        // clip's end even when the video cuts the clip short
        let mut chain = format!("atrim=duration={:.3}", dur / 1000.0);
        if fade_in > 0.0 {
            chain.push_str(&format!(",afade=t=in:st=0:d={:.3}", fade_in / 1000.0));
        }
        if fade_out > 0.0 {
            chain.push_str(&format!(",afade=t=out:st={:.3}:d={:.3}", ((m.duration_ms - fade_out) / 1000.0).max(0.0), fade_out / 1000.0));
        }
        inputs.push(AudioMixInput { path, volume: level, delay_ms: m.output_start_ms, start_ms: (offset > 0.0).then_some(offset), end_ms: None, tempo: 1.0, loop_input: true, filter: Some(chain) });
    }
    inputs
}

pub fn export(session: &mut Session, comp: &mut Compositor, opts: &ExportOptions, progress: impl FnMut(Progress)) -> Result<PathBuf> {
    export_cancellable(session, comp, opts, &std::sync::atomic::AtomicBool::new(false), progress)
}

/// `export` that stops as soon as `cancel` becomes true, deleting the partial file and
/// returning an error whose message is "cancelled".
pub fn export_cancellable(session: &mut Session, comp: &mut Compositor, opts: &ExportOptions, cancel: &std::sync::atomic::AtomicBool, mut progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let out = session.output_size(opts.max_height);
    let (start, end) = opts.range_ms.unwrap_or((0.0, session.duration_ms()));
    let frame_ms = 1000.0 / opts.fps;
    let total_frames = ((end - start) / frame_ms).ceil().max(1.0) as u64;

    let mut audio = if opts.include_audio && opts.codec != VideoCodec::Gif { audio_inputs(session) } else { vec![] };
    if opts.range_ms.is_some() {
        // Shift/clip audio to the exported range.
        audio.retain(|a| a.delay_ms < end);
        for a in &mut audio {
            a.delay_ms -= start;
        }
        audio.retain(|a| a.delay_ms > -1.0);
    }

    let wopts = VideoWriterOptions {
        width: out.width,
        height: out.height,
        fps: opts.fps,
        codec: opts.codec,
        quality: opts.quality,
        audio,
        duration_ms: Some(total_frames as f64 * frame_ms),
        ..Default::default()
    };
    let mut writer = VideoWriter::create(&opts.output, &wopts)?;

    session.decode_max_width = None;
    for i in 0..total_frames {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            writer.abort();
            anyhow::bail!("cancelled");
        }
        let t = start + i as f64 * frame_ms;
        let desc = session.frame_desc_at(t, out)?;
        comp.render(&desc);
        let px = comp.read_pixels()?;
        writer.write_frame(&px)?;
        progress(Progress { frame: i + 1, total_frames, output_ms: t });
    }
    Ok(writer.finish()?)
}
