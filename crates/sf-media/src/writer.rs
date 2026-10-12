//! Encoding raw frames to MP4 (H.264 / HEVC), GIF or WebM through an ffmpeg pipe, with optional
//! audio inputs mixed in the same pass.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};

use crate::{ffmpeg::{ffmpeg_cmd, has_encoder}, Error, PixelFormat, Result};

/// Screen Studio's export "Compression" presets. Bitrate = width × height × fps × bits-per-pixel;
/// when the encoder supports constant quality we use the quantizer instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityPreset {
    Studio,
    SocialMedia,
    WebHigh,
    WebLow,
}

impl QualityPreset {
    pub const ALL: [QualityPreset; 4] = [QualityPreset::Studio, QualityPreset::SocialMedia, QualityPreset::WebHigh, QualityPreset::WebLow];

    pub fn label(self) -> &'static str {
        match self {
            QualityPreset::Studio => "Studio",
            QualityPreset::SocialMedia => "Social Media",
            QualityPreset::WebHigh => "Web",
            QualityPreset::WebLow => "Web (Low)",
        }
    }

    pub fn bits_per_pixel(self) -> f64 {
        match self {
            QualityPreset::Studio => 0.3,
            QualityPreset::SocialMedia => 0.05,
            QualityPreset::WebHigh => 0.0175,
            QualityPreset::WebLow => 0.0075,
        }
    }

    /// H.264 quantizer (lower = better) used with constant-quality encoders.
    pub fn quantizer(self) -> u32 {
        match self {
            QualityPreset::Studio => 14,
            QualityPreset::SocialMedia => 24,
            QualityPreset::WebHigh => 30,
            QualityPreset::WebLow => 42,
        }
    }

    pub fn bitrate_kbps(self, width: u32, height: u32, fps: f64) -> u64 {
        ((width as f64 * height as f64 * fps * self.bits_per_pixel()) / 1000.0).floor().max(500.0) as u64
    }

    /// Nearest preset for a 0..1 quality slider.
    pub fn from_quality(q: f64) -> Self {
        if q >= 0.9 {
            QualityPreset::Studio
        } else if q >= 0.6 {
            QualityPreset::SocialMedia
        } else if q >= 0.35 {
            QualityPreset::WebHigh
        } else {
            QualityPreset::WebLow
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264, hardware (VideoToolbox) when available, libx264 otherwise.
    H264,
    /// HEVC/H.265, hardware when available.
    Hevc,
    /// ProRes 4444 (with alpha) via VideoToolbox / prores_ks.
    ProRes4444,
    Gif,
    /// VP9 in WebM (software).
    Vp9,
}

/// An audio file to mix into the output. `delay_ms` positions it on the output timeline.
#[derive(Debug, Clone)]
pub struct AudioMixInput {
    pub path: PathBuf,
    pub volume: f64,
    pub delay_ms: f64,
    /// Optional trim of the input (ms).
    pub start_ms: Option<f64>,
    pub end_ms: Option<f64>,
    /// Playback speed (2.0 = twice as fast, pitch preserved via `atempo`).
    pub tempo: f64,
    /// Loop the input forever (background music); the output `duration_ms` clips it.
    pub loop_input: bool,
    /// Extra ffmpeg filter chain for this input (e.g. microphone noise reduction).
    pub filter: Option<String>,
}

impl AudioMixInput {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        AudioMixInput { path: path.into(), volume: 1.0, delay_ms: 0.0, start_ms: None, end_ms: None, tempo: 1.0, loop_input: false, filter: None }
    }
}

/// `atempo` accepts 0.5..=100 per instance; chain for slower rates.
fn atempo_chain(mut tempo: f64) -> String {
    let mut parts = vec![];
    while tempo < 0.5 {
        parts.push("atempo=0.5".to_string());
        tempo /= 0.5;
    }
    if (tempo - 1.0).abs() > 1e-6 {
        parts.push(format!("atempo={tempo:.4}"));
    }
    parts.join(",")
}

#[derive(Debug, Clone)]
pub struct VideoWriterOptions {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub format: PixelFormat,
    pub codec: VideoCodec,
    /// 0..=1, mapped to bitrate / CRF.
    pub quality: f64,
    pub audio: Vec<AudioMixInput>,
    pub audio_sample_rate: u32,
    /// Clip the output (video and mixed audio) to this length.
    pub duration_ms: Option<f64>,
}

impl Default for VideoWriterOptions {
    fn default() -> Self {
        VideoWriterOptions {
            width: 1920,
            height: 1080,
            fps: 60.0,
            format: PixelFormat::Rgba8,
            codec: VideoCodec::H264,
            quality: 0.8,
            audio: vec![],
            audio_sample_rate: 48_000,
            duration_ms: None,
        }
    }
}

pub struct VideoWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    frame_bytes: usize,
    pub frames_written: u64,
    pub output: PathBuf,
}

impl VideoWriter {
    pub fn create(output: &Path, opts: &VideoWriterOptions) -> Result<Self> {
        let mut cmd = ffmpeg_cmd()?;
        cmd.arg("-y");
        // 0: raw frames on stdin
        cmd.args(["-f", "rawvideo", "-pix_fmt", opts.format.ffmpeg_name()])
            .arg("-video_size")
            .arg(format!("{}x{}", opts.width, opts.height))
            .arg("-framerate")
            .arg(format!("{}", opts.fps))
            .args(["-i", "pipe:0"]);
        // 1..n: audio inputs
        for a in &opts.audio {
            if a.loop_input {
                cmd.args(["-stream_loop", "-1"]);
            }
            if let Some(s) = a.start_ms {
                cmd.arg("-ss").arg(format!("{:.3}", s / 1000.0));
            }
            if let Some(e) = a.end_ms {
                cmd.arg("-to").arg(format!("{:.3}", e / 1000.0));
            }
            cmd.arg("-i").arg(&a.path);
        }
        if !opts.audio.is_empty() {
            let mut filter = String::new();
            for (i, a) in opts.audio.iter().enumerate() {
                let d = a.delay_ms.max(0.0).round() as i64;
                let tempo = atempo_chain(a.tempo.max(0.01));
                let tempo = if tempo.is_empty() { String::new() } else { format!("{tempo},") };
                let extra = a.filter.as_deref().map(|f| format!("{f},")).unwrap_or_default();
                filter.push_str(&format!(
                    "[{}:a]{extra}aresample={},aformat=channel_layouts=stereo,{tempo}volume={:.4},adelay={d}|{d}[a{i}];",
                    i + 1,
                    opts.audio_sample_rate,
                    a.volume
                ));
            }
            for i in 0..opts.audio.len() {
                filter.push_str(&format!("[a{i}]"));
            }
            filter.push_str(&format!("amix=inputs={}:normalize=0:dropout_transition=0[aout]", opts.audio.len()));
            cmd.arg("-filter_complex").arg(filter);
            cmd.args(["-map", "0:v", "-map", "[aout]"]);
            cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        }
        // video codec
        let preset = QualityPreset::from_quality(opts.quality);
        let bitrate_kbps = |_q: f64| -> u64 { preset.bitrate_kbps(opts.width, opts.height, opts.fps) };
        match opts.codec {
            VideoCodec::H264 => {
                if has_encoder("h264_videotoolbox") {
                    cmd.args(["-c:v", "h264_videotoolbox", "-profile:v", "high", "-allow_sw", "1"]);
                    cmd.arg("-b:v").arg(format!("{}k", bitrate_kbps(opts.quality)));
                    cmd.arg("-maxrate").arg(format!("{}k", bitrate_kbps(opts.quality) * 2));
                } else {
                    cmd.args(["-c:v", "libx264", "-preset", "medium", "-profile:v", "high"]);
                    cmd.arg("-crf").arg(preset.quantizer().to_string());
                }
                cmd.args(["-pix_fmt", "yuv420p", "-movflags", "+faststart", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"]);
            }
            VideoCodec::Hevc => {
                if has_encoder("hevc_videotoolbox") {
                    cmd.args(["-c:v", "hevc_videotoolbox", "-tag:v", "hvc1", "-allow_sw", "1"]);
                    cmd.arg("-b:v").arg(format!("{}k", bitrate_kbps(opts.quality)));
                } else {
                    let crf = (32.0 - 12.0 * opts.quality).round() as u32;
                    cmd.args(["-c:v", "libx265", "-tag:v", "hvc1"]);
                    cmd.arg("-crf").arg(crf.to_string());
                }
                cmd.args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
            }
            VideoCodec::ProRes4444 => {
                if has_encoder("prores_videotoolbox") {
                    cmd.args(["-c:v", "prores_videotoolbox", "-profile:v", "4444", "-pix_fmt", "bgra"]);
                } else {
                    cmd.args(["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le"]);
                }
            }
            VideoCodec::Gif => {
                cmd.arg("-vf").arg("split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle");
                cmd.args(["-loop", "0"]);
            }
            VideoCodec::Vp9 => {
                let crf = (40.0 - 20.0 * opts.quality).round() as u32;
                cmd.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-row-mt", "1", "-pix_fmt", "yuv420p"]);
                cmd.arg("-crf").arg(crf.to_string());
            }
        }
        if let Some(d) = opts.duration_ms {
            cmd.arg("-t").arg(format!("{:.3}", d / 1000.0));
        }
        cmd.arg(output);
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        Ok(VideoWriter {
            child,
            stdin,
            frame_bytes: opts.width as usize * opts.height as usize * opts.format.bytes_per_pixel(),
            frames_written: 0,
            output: output.into(),
        })
    }

    pub fn write_frame(&mut self, pixels: &[u8]) -> Result<()> {
        debug_assert_eq!(pixels.len(), self.frame_bytes, "frame size mismatch");
        let stdin = self.stdin.as_mut().expect("writer already finished");
        stdin.write_all(pixels)?;
        self.frames_written += 1;
        Ok(())
    }

    /// Close the pipe and wait for ffmpeg to finish muxing.
    pub fn finish(mut self) -> Result<PathBuf> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        if !status.success() {
            let mut err = String::new();
            if let Some(mut e) = self.child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            return Err(Error::FfmpegFailed { status: status.to_string(), stderr: err });
        }
        Ok(self.output.clone())
    }
}

impl VideoWriter {
    /// Stop ffmpeg immediately and delete the partial output (export cancelled).
    pub fn abort(mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.output);
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            drop(self.stdin.take());
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VideoReader, VideoReaderOptions};

    #[test]
    fn encode_then_decode_roundtrip() {
        if crate::ffmpeg::ffmpeg_path().is_err() {
            eprintln!("ffmpeg missing; skipping");
            return;
        }
        let out = std::env::temp_dir().join("sf-media-test.mp4");
        let opts = VideoWriterOptions { width: 320, height: 240, fps: 30.0, codec: VideoCodec::H264, ..Default::default() };
        let mut w = VideoWriter::create(&out, &opts).unwrap();
        let mut px = vec![0u8; 320 * 240 * 4];
        for i in 0..15 {
            for p in px.chunks_mut(4) {
                p[0] = (i * 16) as u8;
                p[1] = 128;
                p[2] = 255 - (i * 16) as u8;
                p[3] = 255;
            }
            w.write_frame(&px).unwrap();
        }
        w.finish().unwrap();
        let mut r = VideoReader::open(&out, VideoReaderOptions { fps: Some(30.0), ..Default::default() }).unwrap();
        assert_eq!((r.width, r.height), (320, 240));
        let mut n = 0;
        while let Some(f) = r.next_frame().unwrap() {
            assert_eq!(f.data.len(), 320 * 240 * 4);
            n += 1;
        }
        assert_eq!(n, 15);
        let _ = std::fs::remove_file(out);
    }
}
