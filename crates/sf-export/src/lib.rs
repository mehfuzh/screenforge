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
/// or the detached ScreenForge audio clips) for every microphone / system-audio session.
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
    let spans = sf_engine::audio::audio_spans(scene, &tracks, cfg.audio.volume, cfg.audio.mute_microphone, cfg.audio.mute_system_audio);
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
                    AudioTrack::Microphone => sf_media::microphone_filter(&cfg.audio.microphone_audio_preset).map(str::to_string),
                    AudioTrack::SystemAudio => None,
                },
            });
        }
    }
    if !cfg.audio.mute_background_audio && cfg.audio.background_audio_volume > 0.0 {
        if let Some(path) = session.background_audio_path() {
            inputs.push(AudioMixInput { path, volume: cfg.audio.background_audio_volume, delay_ms: 0.0, start_ms: None, end_ms: None, tempo: 1.0, loop_input: true, filter: None });
        }
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
