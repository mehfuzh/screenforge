//! Locating and describing the recorded sources inside a package.

use std::path::PathBuf;

use anyhow::{Context, Result};
use sf_engine::cursor::CursorTrack;
use sf_project::recording::{InputLog, RecorderSession};
use sf_project::schema::common::Rect;
use sf_project::ProjectPackage;

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub media_path: PathBuf,
    pub native_width: u32,
    pub native_height: u32,
    pub fps: f64,
    pub duration_ms: f64,
    /// Recorded region in screen points (display-relative).
    pub bounds: Rect,
    pub session: RecorderSession,
    pub process_time_start_ms: f64,
    pub microphone: Vec<PathBuf>,
    pub system_audio: Vec<PathBuf>,
    /// Webcam recordings (`webcam` / `camera` recorders), or the `SF_DEBUG_CAMERA` file.
    pub camera: Vec<PathBuf>,
    /// Add to a screen source time to get the camera file time (the recorders may not have
    /// started at the same process time).
    pub camera_time_offset_ms: f64,
}

/// Environment variable naming a video file to use as a fake webcam channel (for testing
/// camera layouts with projects that have none).
pub const DEBUG_CAMERA_ENV: &str = "SF_DEBUG_CAMERA";

impl SourceInfo {
    pub fn from_package(pkg: &ProjectPackage) -> Result<Self> {
        let rec = pkg.recording.as_ref().context("package has no recording/metadata.json")?;
        let dir = pkg.recording_dir();
        let display = rec.display().context("no display/window recorder in metadata")?;
        let session = display.sessions.first().context("display recorder has no sessions")?.clone();
        let media_path = session.media_path(&dir).context("display media file missing")?;
        let info = sf_media::probe(&media_path)?;
        let v = info.video().context("display media has no video stream")?;
        let bounds = session.bounds.or_else(|| display.crop_rect()).unwrap_or(Rect { x: 0.0, y: 0.0, width: v.width.unwrap_or(1) as f64, height: v.height.unwrap_or(1) as f64 });
        let collect = |kind: &str| -> Vec<PathBuf> {
            rec.recorders_of(kind).flat_map(|r| r.sessions.iter().filter_map(|s| s.media_path(&dir))).collect()
        };
        let camera_recorder = rec.recorders_of("webcam").chain(rec.recorders_of("camera")).find(|r| r.sessions.iter().any(|s| s.media_path(&dir).is_some()));
        let mut camera: Vec<PathBuf> = camera_recorder.map(|r| r.sessions.iter().filter_map(|s| s.media_path(&dir)).collect()).unwrap_or_default();
        let mut camera_time_offset_ms = camera_recorder
            .and_then(|r| r.sessions.first())
            .map(|cs| session.process_time_start_ms - cs.process_time_start_ms)
            .filter(|o| o.is_finite())
            .unwrap_or(0.0);
        if let Some(p) = std::env::var_os(DEBUG_CAMERA_ENV).map(PathBuf::from).filter(|p| p.is_file()) {
            tracing::info!("using {} as the camera channel ({DEBUG_CAMERA_ENV})", p.display());
            camera = vec![p];
            camera_time_offset_ms = 0.0;
        }
        Ok(SourceInfo {
            media_path,
            native_width: v.width.unwrap_or(0),
            native_height: v.height.unwrap_or(0),
            fps: v.fps().unwrap_or(60.0),
            duration_ms: info.duration_ms().unwrap_or(session.duration_ms),
            bounds,
            process_time_start_ms: session.process_time_start_ms,
            session,
            microphone: collect("microphone"),
            system_audio: collect("systemAudio"),
            camera,
            camera_time_offset_ms,
        })
    }

    /// Pixel size of the visible crop.
    pub fn content_size_px(&self, crop01: &Rect) -> (u32, u32) {
        (((self.native_width as f64) * crop01.width).round().max(2.0) as u32, ((self.native_height as f64) * crop01.height).round().max(2.0) as u32)
    }

    pub fn cursor_track(&self, pkg: &ProjectPackage) -> Result<CursorTrack> {
        let rec = pkg.recording.as_ref().context("no recording")?;
        let input = rec.input().context("no input recorder")?;
        let s = input.sessions.first().context("input recorder has no sessions")?;
        let log = InputLog::load(&pkg.recording_dir(), s)?;
        Ok(CursorTrack::build(&log.mouse_moves, &log.mouse_clicks, &log.key_strokes, self.bounds, s.process_time_start_ms, pkg.project.config.cursor.remove_shake_treshold))
    }
}
