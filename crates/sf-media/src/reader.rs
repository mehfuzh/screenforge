//! Sequential video decoding through `ffmpeg -f rawvideo` on a pipe.
//!
//! The reader decodes from `start_ms` forward and yields frames in order. Random access is
//! implemented by re-spawning ffmpeg with `-ss` (input seeking lands on the previous keyframe,
//! then ffmpeg decodes forward to the exact time). Callers that scrub should keep a small
//! frame cache in front of this.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};
use std::sync::Arc;

use crate::{ffmpeg::ffmpeg_cmd, probe, Error, Frame, PixelFormat, Result};

#[derive(Debug, Clone)]
pub struct VideoReaderOptions {
    pub format: PixelFormat,
    /// Scale output to this width (keeps aspect). `None` = native size.
    pub max_width: Option<u32>,
    pub start_ms: f64,
    /// Constant output frame rate. Screen recordings are variable-frame-rate (frames only
    /// when the screen changes), so the reader always resamples to a constant rate with
    /// ffmpeg's `fps` filter; `None` = 60.
    pub fps: Option<f64>,
}

impl Default for VideoReaderOptions {
    fn default() -> Self {
        VideoReaderOptions { format: PixelFormat::Rgba8, max_width: None, start_ms: 0.0, fps: None }
    }
}

pub struct VideoReader {
    path: PathBuf,
    opts: VideoReaderOptions,
    child: Child,
    stdout: ChildStdout,
    pub width: u32,
    pub height: u32,
    pub native_width: u32,
    pub native_height: u32,
    pub fps: f64,
    pub duration_ms: f64,
    frame_index: u64,
    buf: Vec<u8>,
}

impl VideoReader {
    pub fn open(path: &Path, opts: VideoReaderOptions) -> Result<Self> {
        let info = probe(path)?;
        let v = info.video().ok_or_else(|| Error::Probe(format!("{}: no video stream", path.display())))?;
        let (nw, nh) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
        let fps = opts.fps.unwrap_or(60.0);
        let duration_ms = info.duration_ms().unwrap_or(0.0);
        let (w, h) = match opts.max_width {
            Some(mw) if mw < nw => {
                let h = ((nh as f64 * mw as f64 / nw as f64).round() as u32) & !1;
                (mw & !1, h)
            }
            _ => (nw, nh),
        };
        let (child, stdout) = Self::spawn(path, &opts, w, h, fps)?;
        Ok(VideoReader {
            path: path.into(),
            opts,
            child,
            stdout,
            width: w,
            height: h,
            native_width: nw,
            native_height: nh,
            fps,
            duration_ms,
            frame_index: 0,
            buf: Vec::new(),
        })
    }

    fn spawn(path: &Path, opts: &VideoReaderOptions, w: u32, h: u32, fps: f64) -> Result<(Child, ChildStdout)> {
        let mut cmd = ffmpeg_cmd()?;
        if opts.start_ms > 0.0 {
            cmd.arg("-ss").arg(format!("{:.3}", opts.start_ms / 1000.0));
        }
        // HLS playlists written by Screen Studio are local VOD playlists referencing .m4s/.mp4.
        if path.extension().and_then(|e| e.to_str()) == Some("m3u8") {
            cmd.args(["-allowed_extensions", "ALL"]);
        }
        cmd.arg("-i").arg(path);
        let vf = [format!("fps={fps}"), format!("scale={w}:{h}:flags=bicubic")];
        cmd.args(["-an", "-sn", "-dn"]);
        cmd.arg("-vf").arg(vf.join(","));
        cmd.args(["-f", "rawvideo", "-pix_fmt", opts.format.ffmpeg_name(), "-"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().unwrap();
        Ok((child, stdout))
    }

    /// Re-open at a new start time.
    pub fn seek(&mut self, start_ms: f64) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.opts.start_ms = start_ms.max(0.0);
        let (child, stdout) = Self::spawn(&self.path, &self.opts, self.width, self.height, self.fps)?;
        self.child = child;
        self.stdout = stdout;
        self.frame_index = 0;
        Ok(())
    }

    /// Next frame, or `None` at end of stream.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        let n = self.width as usize * self.height as usize * self.opts.format.bytes_per_pixel();
        self.buf.resize(n, 0);
        let mut read = 0;
        while read < n {
            match self.stdout.read(&mut self.buf[read..]) {
                Ok(0) => break,
                Ok(k) => read += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        if read == 0 {
            return self.finish().map(|_| None);
        }
        if read < n {
            return Err(Error::Probe("truncated frame from ffmpeg".into()));
        }
        let pts_ms = self.opts.start_ms + self.frame_index as f64 * 1000.0 / self.fps;
        self.frame_index += 1;
        Ok(Some(Frame {
            width: self.width,
            height: self.height,
            format: self.opts.format,
            pts_ms,
            data: Arc::from(self.buf.as_slice()),
        }))
    }

    fn finish(&mut self) -> Result<()> {
        let status = self.child.wait()?;
        if !status.success() {
            let mut err = String::new();
            if let Some(mut e) = self.child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            if !err.trim().is_empty() {
                return Err(Error::FfmpegFailed { status: status.to_string(), stderr: err });
            }
        }
        Ok(())
    }
}

impl Drop for VideoReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
