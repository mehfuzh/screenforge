//! Media I/O.
//!
//! Screen Studio bundles an `ffmpeg` binary and drives it over pipes; we do the same for the
//! first backend so recordings (HLS fMP4 playlists, `.m4a` audio, `.mp4`) decode anywhere ffmpeg
//! runs. Hardware encoding uses `h264_videotoolbox`/`hevc_videotoolbox` on macOS and falls back
//! to `libx264`.

pub mod audio;
pub mod ffmpeg;
pub mod frame;
pub mod probe;
pub mod reader;
pub mod writer;

pub use audio::{decode as decode_audio, decode_filtered as decode_audio_filtered, microphone_filter, AudioClip};
pub use frame::{Frame, PixelFormat};
pub use probe::{probe, MediaInfo, StreamInfo};
pub use reader::{VideoReader, VideoReaderOptions};
pub use writer::{AudioMixInput, QualityPreset, VideoCodec, VideoWriter, VideoWriterOptions};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ffmpeg not found. Install it (brew install ffmpeg) or set SF_FFMPEG=/path/to/ffmpeg")]
    FfmpegNotFound,
    #[error("ffmpeg failed ({status}): {stderr}")]
    FfmpegFailed { status: String, stderr: String },
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("probe: {0}")]
    Probe(String),
}

pub type Result<T> = std::result::Result<T, Error>;
