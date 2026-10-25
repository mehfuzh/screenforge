//! Display recorder: ScreenCaptureKit → frame repeater → ffmpeg HLS/fMP4 writer.
//!
//! Output per session `S`: `channel-1-display-S.m3u8`, `channel-1-display-S-0000.mp4`
//! (init segment) and `channel-1-display-S-NNNN.m4s` (~2 s segments, H.264 High, yuv420p,
//! bt709), i.e. the layout polyrecorder's `AVAssetWriter` produces.
//!
//! ScreenCaptureKit only delivers a frame when the screen changes, so a repeater thread
//! keeps the last frame and feeds ffmpeg at a constant frame rate; if the encoder falls
//! behind, missed ticks are caught up with duplicate frames so the media duration always
//! equals wall time (and therefore the input event timeline).

use sf_project::schema::common::Rect;

use crate::config::{CaptureTarget, RecordingConfig};
use crate::discover::Discovery;
use crate::{Error, Result};

/// Screen Studio caps the output width at 4096 px.
pub const MAX_WIDTH_PX: u32 = 4096;
/// Average bitrate polyrecorder chose for 2400×1800 at quality 0.85.
const REFERENCE_BITRATE: f64 = 16_943_358.0;
const REFERENCE_PIXELS: f64 = 2400.0 * 1800.0;

/// Geometry of the display capture, derived from the config and the display list.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayPlan {
    pub display_id: u32,
    /// Display-relative crop in points (`None` = whole display).
    pub crop: Option<Rect>,
    /// Recorded region in global points.
    pub bounds: Rect,
    /// Backing scale of the display.
    pub scale: f64,
    pub width_px: u32,
    pub height_px: u32,
    /// Points per output pixel (`bounds.width / width_px`).
    pub recording_scale: f64,
    pub refresh_rate: f64,
}

fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// Work out what to capture and at which pixel size.
pub fn plan(cfg: &RecordingConfig, discovery: &Discovery) -> Result<DisplayPlan> {
    let (display, region): (_, Option<Rect>) = match &cfg.target {
        CaptureTarget::Display { display_id } => {
            (discovery.display(*display_id).ok_or_else(|| Error::InvalidConfig(format!("unknown display {display_id}")))?, None)
        }
        CaptureTarget::Area { display_id, rect_points } => (
            discovery.display(*display_id).ok_or_else(|| Error::InvalidConfig(format!("unknown display {display_id}")))?,
            Some(*rect_points),
        ),
        CaptureTarget::Window { window_id } => {
            let w = discovery.window(*window_id).ok_or_else(|| Error::InvalidConfig(format!("unknown window {window_id}")))?;
            let cx = w.bounds.x + w.bounds.width / 2.0;
            let cy = w.bounds.y + w.bounds.height / 2.0;
            let d = discovery
                .displays
                .iter()
                .find(|d| {
                    let b = &d.bounds_points;
                    cx >= b.x && cx < b.x + b.width && cy >= b.y && cy < b.y + b.height
                })
                .or_else(|| discovery.main_display())
                .ok_or_else(|| Error::InvalidConfig("no display".into()))?;
            (d, Some(w.bounds))
        }
        CaptureTarget::Device { .. } => return Err(Error::InvalidConfig("a device recording is sized by the device, not planned".into())),
        CaptureTarget::PortalWindow => {
            return Err(Error::InvalidConfig("the window is picked in the screen-sharing dialog (see window_stream_plan)".into()))
        }
    };
    let db = display.bounds_points;
    let (bounds, crop) = match region {
        None => (db, None),
        Some(r) => {
            let b = intersect(&r, &db).ok_or_else(|| Error::InvalidConfig("capture area is outside the display".into()))?;
            // integral points like Screen Studio's picker produces
            let b = Rect { x: b.x.floor(), y: b.y.floor(), width: b.width.round().max(2.0), height: b.height.round().max(2.0) };
            (b, Some(Rect { x: b.x - db.x, y: b.y - db.y, width: b.width, height: b.height }))
        }
    };
    let scale = display.scale.max(1.0);
    let mut width_px = (bounds.width * scale).round() as u32;
    let mut height_px = (bounds.height * scale).round() as u32;
    if width_px > MAX_WIDTH_PX {
        let f = MAX_WIDTH_PX as f64 / width_px as f64;
        width_px = MAX_WIDTH_PX;
        height_px = (height_px as f64 * f).round() as u32;
    }
    width_px = (width_px & !1).max(2);
    height_px = (height_px & !1).max(2);
    Ok(DisplayPlan {
        display_id: display.id,
        crop,
        bounds,
        scale,
        width_px,
        height_px,
        recording_scale: bounds.width / width_px as f64,
        refresh_rate: display.refresh_rate,
    })
}

/// Where a window recording's crop goes when the window is at `window` (global points): the
/// same `size`, its top-left on the window's, kept inside `display` (global points). The result
/// is display-relative, like [`DisplayPlan::crop`].
pub fn follow_crop(window: Rect, size: (f64, f64), display: Rect) -> Rect {
    let (w, h) = (size.0.min(display.width), size.1.min(display.height));
    let x = (window.x - display.x).clamp(0.0, display.width - w).floor();
    let y = (window.y - display.y).clamp(0.0, display.height - h).floor();
    Rect { x, y, width: w, height: h }
}

/// Geometry for a window stream from the screen-cast portal: `width`×`height` points recorded
/// at `scale` pixels per point. Its position on screen is unknown, so the bounds (and the
/// cursor positions recorded with them) are relative to the window's top-left corner.
pub fn window_stream_plan(width: u32, height: u32, scale: f64, discovery: &Discovery) -> Result<DisplayPlan> {
    let display = discovery.main_display().ok_or_else(|| Error::InvalidConfig("no display".into()))?;
    let bounds = Rect { x: 0.0, y: 0.0, width: (width.max(2) & !1) as f64, height: (height.max(2) & !1) as f64 };
    let scale = scale.max(1.0);
    let mut width_px = (bounds.width * scale).round() as u32;
    let mut height_px = (bounds.height * scale).round() as u32;
    if width_px > MAX_WIDTH_PX {
        height_px = (height_px as f64 * MAX_WIDTH_PX as f64 / width_px as f64).round() as u32;
        width_px = MAX_WIDTH_PX;
    }
    width_px = (width_px & !1).max(2);
    height_px = (height_px & !1).max(2);
    Ok(DisplayPlan {
        display_id: display.id,
        // a crop marks the recording as a window/area one, like a macOS window recording
        crop: Some(bounds),
        bounds,
        scale,
        width_px,
        height_px,
        recording_scale: bounds.width / width_px as f64,
        refresh_rate: display.refresh_rate,
    })
}

/// Target H.264 bitrate: Screen Studio's ~17 Mb/s for 2400×1800 scaled by pixel count and
/// quality, clamped to a sane range.
/// H.264 encoder for live captures: VideoToolbox when this ffmpeg has it, else x264
/// (`SF_VIDEO_ENCODER=<name>` overrides, e.g. `h264_nvenc` on Windows).
pub fn video_encoder() -> &'static str {
    static FORCED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if let Some(name) = FORCED.get_or_init(|| std::env::var("SF_VIDEO_ENCODER").ok().filter(|n| crate::ffmpeg::has_encoder(n))) {
        return name.as_str();
    }
    if cfg!(target_os = "macos") && crate::ffmpeg::has_encoder("h264_videotoolbox") { "h264_videotoolbox" } else { "libx264" }
}

pub fn target_bitrate(width: u32, height: u32, quality: f64) -> u64 {
    let px = width as f64 * height as f64;
    let q = (quality / 0.85).clamp(0.3, 1.5);
    (REFERENCE_BITRATE * px / REFERENCE_PIXELS * q).clamp(1.5e6, 80e6) as u64
}

/// ffmpeg arguments for one HLS session fed tightly packed BGRA frames on stdin (relative
/// file names; run with cwd = output dir).
pub fn hls_args(width: u32, height: u32, fps: u32, bitrate: u64, encoder: &str, session: usize) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-f", "rawvideo", "-pix_fmt", "bgra", "-video_size"].map(String::from).to_vec();
    args.extend([format!("{width}x{height}"), "-framerate".into(), fps.to_string(), "-i".into(), "pipe:0".into()]);
    args.extend(hls_encode_args(bitrate, encoder, session));
    args
}

/// [`hls_args`] for any raw stdin input, with an optional filter chain to the output size.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub(crate) fn hls_raw_args(input: &crate::repeater::RawInput, fps: u32, bitrate: u64, encoder: &str, base: &str, session: usize) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-f", "rawvideo", "-pix_fmt"].map(String::from).to_vec();
    args.extend([input.pix_fmt.into(), "-video_size".into(), format!("{}x{}", input.width, input.height)]);
    args.extend(["-framerate".into(), fps.to_string(), "-i".into(), "pipe:0".into()]);
    if let Some(vf) = &input.filter {
        args.extend(["-vf".into(), vf.clone()]);
    }
    args.extend(hls_encode_args_for(base, bitrate, encoder, session));
    args
}

/// Encoder + HLS muxer arguments shared by every capture input.
pub fn hls_encode_args(bitrate: u64, encoder: &str, session: usize) -> Vec<String> {
    hls_encode_args_for(DISPLAY_CHANNEL, bitrate, encoder, session)
}

/// Recorder id of the display channel (file names start with it).
pub const DISPLAY_CHANNEL: &str = "channel-1-display";

/// [`hls_encode_args`] for any channel: files are `<channel>-<session>.m3u8` and segments.
pub fn hls_encode_args_for(channel: &str, bitrate: u64, encoder: &str, session: usize) -> Vec<String> {
    let base = format!("{channel}-{session}");
    let mut args: Vec<String> = vec!["-c:v".into(), encoder.into()];
    match encoder {
        "libx264" => args.extend(["-preset", "veryfast", "-tune", "zerolatency"].map(String::from)),
        "h264_videotoolbox" => args.extend(["-allow_sw", "1", "-realtime", "1"].map(String::from)),
        "h264_nvenc" => args.extend(["-preset", "p4", "-tune", "ll"].map(String::from)),
        "h264_qsv" => args.extend(["-preset", "veryfast"].map(String::from)),
        "h264_amf" => args.extend(["-usage", "lowlatency"].map(String::from)),
        _ => {}
    }
    args.extend(
        [
            "-profile:v", "high", "-b:v", &bitrate.to_string(), "-g", "120", "-pix_fmt", "yuv420p",
            "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv",
            // videotoolbox does not always put the colour description into the SPS VUI; rewrite it.
            "-bsf:v", "h264_metadata=video_full_range_flag=0:colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.extend(hls_muxer_args(&base));
    args
}

/// The HLS muxer writing `<base>.m3u8`, its init segment and ~2 s fMP4 segments.
pub fn hls_muxer_args(base: &str) -> Vec<String> {
    [
        "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
        "-hls_flags", "independent_segments", "-hls_list_size", "0", "-start_number", "1",
        "-hls_fmp4_init_filename", &format!("{base}-0000.mp4"),
        "-hls_segment_filename", &format!("{base}-%04d.m4s"),
        &format!("{base}.m3u8"),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(target_os = "macos")]
pub use imp::DisplayRecorder;
#[cfg(target_os = "linux")]
pub use linux::DisplayRecorder;

/// Linux on X11: ffmpeg grabs the region straight from the X server (`x11grab`), one ffmpeg
/// per session, writing the same HLS layout as the macOS recorder. The cursor is left out of
/// the video (`-draw_mouse 0`); the editor draws it from the input log. Wayland sessions use
/// the ScreenCast portal instead ([`crate::portal`]).
#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use parking_lot::Mutex;

    use super::{hls_encode_args, target_bitrate, DisplayPlan};
    use crate::config::RecordingConfig;
    use crate::ffmpeg::FfmpegProc;
    use crate::log::RecLog;
    use crate::{Error, Result};

    const COMPONENT: &str = "DisplayRecorder";

    use crate::portal::{is_wayland, PickedWindow, PointerSink, PortalRecorder};

    /// The capture backend for this session.
    pub enum DisplayRecorder {
        X11(X11Recorder),
        Portal(PortalRecorder),
    }

    impl DisplayRecorder {
        pub fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, _system_audio: Option<()>) -> Result<Self> {
            if is_wayland() {
                return PortalRecorder::start(cfg, plan, log).map(DisplayRecorder::Portal);
            }
            X11Recorder::start(cfg, plan, log).map(DisplayRecorder::X11)
        }

        /// Record a window picked in the screen-sharing dialog (Wayland).
        pub fn start_window(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, window: PickedWindow) -> Result<Self> {
            PortalRecorder::start_window(cfg, plan, log, window).map(DisplayRecorder::Portal)
        }

        /// True when this backend reports cursor positions itself (see [`Self::set_pointer_sink`]).
        pub fn provides_pointer(&self) -> bool {
            matches!(self, DisplayRecorder::Portal(p) if p.cursor_metadata)
        }

        pub fn set_pointer_sink(&self, sink: PointerSink) {
            if let DisplayRecorder::Portal(p) = self {
                p.set_pointer_sink(sink);
            }
        }

        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            match self {
                DisplayRecorder::X11(r) => r.begin_session(index, dir),
                DisplayRecorder::Portal(r) => r.begin_session(index, dir),
            }
        }

        pub fn end_session(&self) -> Result<u64> {
            match self {
                DisplayRecorder::X11(r) => r.end_session(),
                DisplayRecorder::Portal(r) => r.end_session(),
            }
        }

        pub fn frames_received(&self) -> u64 {
            match self {
                DisplayRecorder::X11(r) => r.frames_received(),
                DisplayRecorder::Portal(r) => r.frames_received(),
            }
        }

        pub fn error(&self) -> Option<String> {
            match self {
                DisplayRecorder::X11(r) => r.error(),
                DisplayRecorder::Portal(r) => r.error(),
            }
        }

        pub fn stop(&mut self) {
            match self {
                DisplayRecorder::X11(r) => r.stop(),
                DisplayRecorder::Portal(r) => r.stop(),
            }
        }
    }

    pub struct X11Recorder {
        plan: DisplayPlan,
        fps: u32,
        quality: f64,
        display: String,
        log: RecLog,
        proc: Mutex<Option<(FfmpegProc, std::time::Instant)>>,
        error: Mutex<Option<String>>,
    }

    /// Why recording cannot work in this session, if it cannot. Wayland records through the
    /// portal but the app (and display discovery) still needs XWayland.
    pub fn unsupported_reason() -> Option<String> {
        if std::env::var_os("DISPLAY").is_none() {
            return Some(if is_wayland() {
                "XWayland is not available. ScreenForge needs it on Wayland (it is on by default in GNOME and KDE).".into()
            } else {
                "No X11 display found.".into()
            });
        }
        None
    }

    impl X11Recorder {
        fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog) -> Result<Self> {
            if let Some(why) = unsupported_reason() {
                return Err(Error::Capture(why));
            }
            let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
            log.notice(COMPONENT, format!("x11grab on {display}: {plan:?}"));
            Ok(X11Recorder { plan, fps: cfg.fps, quality: cfg.video_quality, display, log, proc: Mutex::new(None), error: Mutex::new(None) })
        }

        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let b = self.plan.bounds;
            let (w, h) = (self.plan.width_px, self.plan.height_px);
            // x11grab needs even sizes for yuv420p
            let (gw, gh) = ((b.width as u32) & !1, (b.height as u32) & !1);
            let mut args: Vec<String> = vec![
                "-y".into(),
                "-f".into(),
                "x11grab".into(),
                "-draw_mouse".into(),
                "0".into(),
                "-framerate".into(),
                self.fps.to_string(),
                "-video_size".into(),
                format!("{gw}x{gh}"),
                "-i".into(),
                format!("{}+{},{}", self.display, b.x as i64, b.y as i64),
            ];
            if (gw, gh) != (w, h) {
                args.extend(["-vf".into(), format!("scale={w}:{h}:flags=bicubic")]);
            }
            args.extend(hls_encode_args(target_bitrate(w, h, self.quality), "libx264", index));
            let proc = FfmpegProc::spawn(&args, dir, self.log.clone(), COMPONENT)?;
            *self.proc.lock() = Some((proc, std::time::Instant::now()));
            Ok(())
        }

        /// Stop the session's ffmpeg; returns the approximate number of frames recorded.
        pub fn end_session(&self) -> Result<u64> {
            let Some((proc, started)) = self.proc.lock().take() else { return Ok(0) };
            let frames = (started.elapsed().as_secs_f64() * self.fps as f64) as u64;
            if let Err(e) = proc.interrupt() {
                *self.error.lock() = Some(e.to_string());
                return Err(e);
            }
            Ok(frames)
        }

        pub fn frames_received(&self) -> u64 {
            self.proc.lock().as_ref().map(|(_, t)| (t.elapsed().as_secs_f64() * self.fps as f64) as u64).unwrap_or(0)
        }

        pub fn error(&self) -> Option<String> {
            self.error.lock().clone()
        }

        pub fn stop(&mut self) {
            if let Some((proc, _)) = self.proc.lock().take() {
                let _ = proc.interrupt();
            }
        }
    }

    impl Drop for DisplayRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }
}
#[cfg(target_os = "linux")]
pub use linux::unsupported_reason;
#[cfg(target_os = "windows")]
pub use windows_impl::DisplayRecorder;

/// Windows: ffmpeg grabs the region itself, one ffmpeg per session, through the Desktop
/// Duplication API (`ddagrab`, GPU-side and cheap) or, when that is unavailable, GDI
/// (`gdigrab`). The cursor is left out of the video; the editor draws it from the input log.
/// `SF_WIN_CAPTURE=gdigrab|ddagrab` forces a backend.
#[cfg(target_os = "windows")]
mod windows_impl {
    use std::path::Path;

    use parking_lot::Mutex;

    use super::{hls_encode_args, target_bitrate, DisplayPlan};
    use crate::config::RecordingConfig;
    use crate::ffmpeg::FfmpegProc;
    use crate::log::RecLog;
    use crate::{Error, Result};

    const COMPONENT: &str = "DisplayRecorder";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Backend {
        /// Desktop Duplication on DXGI output `output_idx` of the default adapter.
        Dda { output_idx: u32 },
        Gdi,
    }

    pub struct DisplayRecorder {
        pub plan: DisplayPlan,
        fps: u32,
        quality: f64,
        backend: Backend,
        /// Region in physical pixels of the virtual screen.
        region_px: (i64, i64, u32, u32),
        /// Region relative to the monitor's top-left corner, in pixels (ddagrab).
        monitor_offset_px: (i64, i64),
        excluded: Vec<u32>,
        log: RecLog,
        proc: Mutex<Option<(FfmpegProc, std::time::Instant)>>,
        error: Mutex<Option<String>>,
    }

    /// Does `ddagrab` work on this machine (cached)? A one-frame test capture: the filter is
    /// listed by every recent build but needs a D3D11 device and a desktop session.
    fn ddagrab_works(output_idx: u32, log: &RecLog) -> bool {
        use std::sync::OnceLock;
        static CACHE: OnceLock<Mutex<std::collections::HashMap<u32, bool>>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(ok) = cache.lock().get(&output_idx) {
            return *ok;
        }
        let ok = crate::ffmpeg::has_filter("ddagrab")
            && crate::ffmpeg::ffmpeg_cmd()
                .map(|mut c| {
                    c.args(["-f", "lavfi", "-i", &format!("ddagrab=output_idx={output_idx}:framerate=5:draw_mouse=0,hwdownload,format=bgra"), "-frames:v", "2", "-f", "null", "-"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    c.status().map(|s| s.success()).unwrap_or(false)
                })
                .unwrap_or(false);
        log.notice(COMPONENT, format!("ddagrab probe on output {output_idx}: {}", if ok { "ok" } else { "unavailable" }));
        cache.lock().insert(output_idx, ok);
        ok
    }

    impl DisplayRecorder {
        pub fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, _system_audio: Option<()>) -> Result<Self> {
            let monitor = crate::win::monitors().into_iter().find(|m| m.id == plan.display_id).ok_or_else(|| Error::InvalidConfig(format!("display {} not found", plan.display_id)))?;
            let s = monitor.scale;
            let b = plan.bounds;
            // physical region; the plan's pixel size is bounds × scale, so this is it (even-sized)
            let x = (b.x * s).round() as i64;
            let y = (b.y * s).round() as i64;
            let (w, h) = ((b.width * s).round() as u32 & !1, (b.height * s).round() as u32 & !1);
            let region_px = (x, y, w.max(2), h.max(2));
            let monitor_offset_px = (x - monitor.rect_px.x.round() as i64, y - monitor.rect_px.y.round() as i64);
            let forced = std::env::var("SF_WIN_CAPTURE").ok();
            let dda = match forced.as_deref() {
                Some("gdigrab") => None,
                Some("ddagrab") => crate::win::dxgi_output_index(monitor.handle),
                _ => crate::win::dxgi_output_index(monitor.handle).filter(|i| ddagrab_works(*i, &log)),
            };
            let backend = match dda {
                Some(output_idx) => Backend::Dda { output_idx },
                None => {
                    if !crate::ffmpeg::has_device("gdigrab") {
                        return Err(Error::Capture("this ffmpeg has neither ddagrab nor gdigrab".into()));
                    }
                    Backend::Gdi
                }
            };
            // our own windows (toolbar, HUD, camera bubble) disappear from the capture
            let excluded: Vec<u32> = cfg.exclude_window_ids.iter().copied().filter(|id| crate::win::set_excluded_from_capture(*id, true)).collect();
            log.notice(COMPONENT, format!("{backend:?} region={region_px:?} monitorOffset={monitor_offset_px:?} scale={s} excludedWindows={} plan={plan:?}", excluded.len()));
            Ok(DisplayRecorder { plan, fps: cfg.fps.max(1), quality: cfg.video_quality, backend, region_px, monitor_offset_px, excluded, log, proc: Mutex::new(None), error: Mutex::new(None) })
        }

        fn input_args(&self) -> Vec<String> {
            let (_, _, gw, gh) = self.region_px;
            match self.backend {
                Backend::Dda { output_idx } => {
                    let (ox, oy) = self.monitor_offset_px;
                    let graph = format!("ddagrab=output_idx={output_idx}:framerate={}:draw_mouse=0:offset_x={ox}:offset_y={oy}:video_size={gw}x{gh},hwdownload,format=bgra", self.fps);
                    vec!["-y".into(), "-f".into(), "lavfi".into(), "-i".into(), graph]
                }
                Backend::Gdi => {
                    let (x, y, _, _) = self.region_px;
                    vec![
                        "-y".into(),
                        "-f".into(),
                        "gdigrab".into(),
                        "-draw_mouse".into(),
                        "0".into(),
                        "-framerate".into(),
                        self.fps.to_string(),
                        "-offset_x".into(),
                        x.to_string(),
                        "-offset_y".into(),
                        y.to_string(),
                        "-video_size".into(),
                        format!("{gw}x{gh}"),
                        "-i".into(),
                        "desktop".into(),
                    ]
                }
            }
        }

        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let (w, h) = (self.plan.width_px, self.plan.height_px);
            let (_, _, gw, gh) = self.region_px;
            let mut args = self.input_args();
            if (gw, gh) != (w, h) {
                args.extend(["-vf".into(), format!("scale={w}:{h}:flags=bicubic")]);
            }
            args.extend(hls_encode_args(target_bitrate(w, h, self.quality), super::video_encoder(), index));
            let proc = FfmpegProc::spawn_grab(&args, dir, self.log.clone(), COMPONENT)?;
            *self.proc.lock() = Some((proc, std::time::Instant::now()));
            Ok(())
        }

        /// Stop the session's ffmpeg; returns the approximate number of frames recorded.
        pub fn end_session(&self) -> Result<u64> {
            let Some((proc, started)) = self.proc.lock().take() else { return Ok(0) };
            let frames = (started.elapsed().as_secs_f64() * self.fps as f64) as u64;
            if let Err(e) = proc.interrupt() {
                *self.error.lock() = Some(e.to_string());
                return Err(e);
            }
            Ok(frames)
        }

        pub fn frames_received(&self) -> u64 {
            self.proc.lock().as_ref().map(|(_, t)| (t.elapsed().as_secs_f64() * self.fps as f64) as u64).unwrap_or(0)
        }

        pub fn error(&self) -> Option<String> {
            self.error.lock().clone()
        }

        pub fn stop(&mut self) {
            if let Some((proc, _)) = self.proc.lock().take() {
                let _ = proc.interrupt();
            }
            for id in self.excluded.drain(..) {
                crate::win::set_excluded_from_capture(id, false);
            }
        }
    }

    impl Drop for DisplayRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use crossbeam_channel::{bounded, unbounded, Sender};
    use parking_lot::Mutex;
    use screencapturekit::cg::CGRect;
    use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus};
    use screencapturekit::cv::CVPixelBufferLockFlags;
    use screencapturekit::shareable_content::SCShareableContent;
    use screencapturekit::stream::configuration::pixel_format::PixelFormat;
    use screencapturekit::stream::configuration::SCStreamConfiguration;
    use screencapturekit::stream::content_filter::SCContentFilter;
    use screencapturekit::stream::delegate_trait::StreamCallbacks;
    use screencapturekit::stream::output_trait::SCStreamOutputTrait;
    use screencapturekit::stream::output_type::SCStreamOutputType;
    use screencapturekit::stream::sc_stream::SCStream;

    use super::{target_bitrate, DisplayPlan};
    use crate::audio::SystemAudioHandler;
    use crate::config::{CaptureTarget, RecordingConfig};
    use crate::log::RecLog;
    use crate::permissions;
    use crate::repeater::{Cmd, FrameStore, RawInput, Repeater};
    use crate::{Error, Result};

    const COMPONENT: &str = "DisplayRecorder";

    struct FrameHandler {
        store: Arc<FrameStore>,
    }

    impl SCStreamOutputTrait for FrameHandler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }
            if matches!(sample.frame_status(), Some(SCFrameStatus::Idle | SCFrameStatus::Blank | SCFrameStatus::Suspended | SCFrameStatus::Stopped)) {
                return;
            }
            let Some(pb) = sample.pixel_buffer() else { return };
            let Ok(guard) = pb.lock(CVPixelBufferLockFlags::READ_ONLY) else { return };
            let (w, h, stride) = (guard.width(), guard.height(), guard.bytes_per_row());
            if w != self.store.width || h != self.store.height {
                return;
            }
            let Some(src) = (unsafe { guard.as_slice() }) else { return };
            let row = w * 4;
            let mut buf = Vec::with_capacity(row * h);
            for y in 0..h {
                let start = y * stride;
                if start + row > src.len() {
                    return;
                }
                buf.extend_from_slice(&src[start..start + row]);
            }
            self.store.put(buf);
        }
    }

    /// Where the frames come from.
    enum Source {
        Screen(SCStream),
        Device(crate::device::DeviceCapture),
    }

    /// ScreenCaptureKit stream (or a connected device's screen) plus the HLS writer.
    pub struct DisplayRecorder {
        source: Source,
        store: Arc<FrameStore>,
        tx: Sender<Cmd>,
        repeater: Option<JoinHandle<()>>,
        error: Arc<Mutex<Option<String>>>,
        follow: Option<Follow>,
        pub plan: DisplayPlan,
        pub encoder: &'static str,
    }

    /// How often a window recording looks for the window having moved.
    const FOLLOW_INTERVAL: Duration = Duration::from_millis(33);

    /// A window recording's capture following the window: a thread that moves the stream's
    /// source rect to the window and notes each move.
    struct Follow {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
        moves: Arc<Mutex<Vec<[f64; 3]>>>,
    }

    impl Follow {
        fn start(stream: SCStream, config: SCStreamConfiguration, window_id: u32, plan: &DisplayPlan, log: RecLog) -> Option<Self> {
            let mut crop = plan.crop?;
            let display = crate::discover::displays().into_iter().find(|d| d.id == plan.display_id)?.bounds_points;
            let (stop, moves) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(Vec::new())));
            let (stop_t, moves_t) = (stop.clone(), moves.clone());
            let thread = std::thread::Builder::new()
                .name("sf-window-follow".into())
                .spawn(move || {
                    let mut config = config;
                    while !stop_t.load(Ordering::Relaxed) {
                        std::thread::sleep(FOLLOW_INTERVAL);
                        // gone (closed, or on another Space): keep the last place
                        let Some(window) = crate::discover::window_bounds(window_id) else { continue };
                        let next = super::follow_crop(window, (crop.width, crop.height), display);
                        if next == crop {
                            continue;
                        }
                        config.set_source_rect(CGRect::new(next.x, next.y, next.width, next.height));
                        if let Err(e) = stream.update_configuration(&config) {
                            log.warning(COMPONENT, format!("Could not follow the window: {e}"));
                            break;
                        }
                        crop = next;
                        moves_t.lock().push([crate::clock::process_time_ms(), display.x + next.x, display.y + next.y]);
                    }
                })
                .ok()?;
            Some(Follow { stop, thread: Some(thread), moves })
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl DisplayRecorder {
        /// Build the content filter and stream and start capturing (frames are kept, not
        /// written, until [`begin_session`](Self::begin_session)). `system_audio` attaches a
        /// system-audio handler to the same stream.
        pub fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, system_audio: Option<SystemAudioHandler>) -> Result<Self> {
            if !permissions::screen_recording().is_granted() {
                return Err(Error::PermissionDenied("screenRecording"));
            }
            let content = SCShareableContent::get().map_err(|e| Error::Capture(format!("shareable content: {e}")))?;
            let display = content
                .displays()
                .into_iter()
                .find(|d| d.display_id() == plan.display_id)
                .ok_or_else(|| Error::InvalidConfig(format!("display {} not capturable", plan.display_id)))?;
            let windows = content.windows();
            let excluded: Vec<_> = windows
                .iter()
                .filter(|w| {
                    cfg.exclude_window_ids.contains(&w.window_id())
                        || (cfg.hide_desktop_icons
                            && w.window_layer() < 0
                            && w.owning_application().map(|a| a.bundle_identifier() == "com.apple.finder").unwrap_or(false))
                })
                .collect();
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&excluded)
                .build()
                .map_err(|e| Error::Capture(format!("content filter: {e}")))?;

            let mut config = SCStreamConfiguration::new()
                .with_width(plan.width_px)
                .with_height(plan.height_px)
                .with_pixel_format(PixelFormat::BGRA)
                .with_fps(cfg.fps.max(1))
                .with_queue_depth(8)
                .with_shows_cursor(false);
            if let Some(c) = &plan.crop {
                config = config.with_source_rect(CGRect::new(c.x, c.y, c.width, c.height));
            }
            if system_audio.is_some() {
                config = config
                    .with_captures_audio(true)
                    .with_sample_rate(48_000)
                    .with_channel_count(2)
                    .with_excludes_current_process_audio(true);
            }

            let follow_config = config.clone();
            let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let err_slot = error.clone();
            let err_log = log.clone();
            let callbacks = StreamCallbacks::new().on_error(move |e| {
                err_log.error(COMPONENT, format!("Stream error: {e}"));
                *err_slot.lock() = Some(e.to_string());
            });
            let mut stream = SCStream::new_with_delegate(&filter, &config, callbacks).map_err(|e| Error::Capture(format!("SCStream: {e}")))?;
            let store = Arc::new(FrameStore::new(plan.width_px as usize, plan.height_px as usize));
            stream
                .add_output_handler(FrameHandler { store: store.clone() }, SCStreamOutputType::Screen)
                .map_err(|e| Error::Capture(format!("output handler: {e}")))?;
            if let Some(h) = system_audio {
                stream.add_output_handler(h, SCStreamOutputType::Audio).map_err(|e| Error::Capture(format!("audio handler: {e}")))?;
            }
            log.debug(
                COMPONENT,
                format!(
                    "Creating SCStream sourceRect={:?} width={} height={} queueDepth=8 fps={} excludedWindows={}",
                    plan.crop, plan.width_px, plan.height_px, cfg.fps, excluded.len()
                ),
            );
            stream.start_capture().map_err(|e| Error::Capture(format!("start capture: {e}")))?;

            // Wait for the first frame so a session never starts without content.
            if !store.wait_first(Duration::from_secs(3)) {
                let _ = stream.stop_capture();
                return Err(Error::Capture("no frames received from ScreenCaptureKit".into()));
            }

            let encoder = super::video_encoder();
            let bitrate = target_bitrate(plan.width_px, plan.height_px, cfg.video_quality);
            log.notice(COMPONENT, format!("Capture started. encoder={encoder} bitrate={bitrate} size={}x{} recordingScale={}", plan.width_px, plan.height_px, plan.recording_scale));

            let (tx, rx) = unbounded();
            let rep = Repeater {
                rx,
                store: store.clone(),
                log: log.clone(),
                component: COMPONENT,
                fps: cfg.fps.max(1),
                input: RawInput { pix_fmt: "bgra", width: plan.width_px, height: plan.height_px, filter: None },
                channel: super::DISPLAY_CHANNEL.into(),
                bitrate,
                encoder,
            };
            let repeater = std::thread::Builder::new()
                .name("sf-frame-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            let follow_id = match cfg.target {
                CaptureTarget::Window { window_id } => Some(window_id),
                CaptureTarget::Area { .. } => cfg.follow_window,
                _ => None,
            };
            let follow = follow_id.and_then(|id| Follow::start(stream.clone(), follow_config, id, &plan, log.clone()));
            if let Some(id) = follow_id {
                log.notice(COMPONENT, format!("Following window {id}: {}", if follow.is_some() { "on" } else { "could not start" }));
            }
            Ok(DisplayRecorder { source: Source::Screen(stream), store, tx, repeater: Some(repeater), error, follow, plan, encoder })
        }

        /// Record a connected iPhone or iPad's screen ([`crate::CaptureTarget::Device`]). Its
        /// frames feed the same repeater; the plan is the device's screen size (points = pixels).
        pub fn start_device(cfg: &RecordingConfig, id: &str, log: RecLog) -> Result<Self> {
            let capture = crate::device::DeviceCapture::start(id)?;
            let store = capture.store();
            let (w, h) = (store.width as u32, store.height as u32);
            // H.264 needs even sizes: pad an odd edge by a pixel
            let (ew, eh) = (w + w % 2, h + h % 2);
            let filter = (ew != w || eh != h).then(|| format!("pad={ew}:{eh}"));
            let plan = DisplayPlan {
                display_id: 0,
                crop: None,
                bounds: sf_project::schema::common::Rect { x: 0.0, y: 0.0, width: ew as f64, height: eh as f64 },
                scale: 1.0,
                width_px: ew,
                height_px: eh,
                recording_scale: 1.0,
                refresh_rate: 60.0,
            };
            let encoder = super::video_encoder();
            let bitrate = target_bitrate(ew, eh, cfg.video_quality);
            log.notice(COMPONENT, format!("Device capture started. device={:?} encoder={encoder} bitrate={bitrate} size={w}x{h}", capture.name));
            let (tx, rx) = unbounded();
            let rep = Repeater {
                rx,
                store: store.clone(),
                log: log.clone(),
                component: COMPONENT,
                fps: cfg.fps.max(1),
                input: RawInput { pix_fmt: "bgra", width: w, height: h, filter },
                channel: super::DISPLAY_CHANNEL.into(),
                bitrate,
                encoder,
            };
            let repeater = std::thread::Builder::new()
                .name("sf-frame-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            Ok(DisplayRecorder { source: Source::Device(capture), store, tx, repeater: Some(repeater), error: Arc::new(Mutex::new(None)), follow: None, plan, encoder })
        }

        /// The connected device being recorded, if this is a device recording.
        pub fn device_name(&self) -> Option<&str> {
            match &self.source {
                Source::Device(d) => Some(&d.name),
                Source::Screen(_) => None,
            }
        }

        /// Spawn ffmpeg for session `index` and start feeding frames.
        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::Begin { index, dir: dir.into(), reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
        }

        /// Stop feeding, finish the playlist. Returns the number of frames written.
        pub fn end_session(&self) -> Result<u64> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::End { reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
        }

        /// Frames delivered by ScreenCaptureKit so far.
        pub fn frames_received(&self) -> u64 {
            self.store.received.load(Ordering::Relaxed)
        }

        /// Last stream error reported by ScreenCaptureKit, if any.
        pub fn error(&self) -> Option<String> {
            self.error.lock().clone()
        }

        /// A window recording's moves so far: `[processTimeMs, x, y]`, the recorded rect's
        /// top-left in global points from then on.
        pub fn bounds_moves(&self) -> Vec<[f64; 3]> {
            self.follow.as_ref().map(|f| f.moves.lock().clone()).unwrap_or_default()
        }

        /// Stop the stream and the repeater.
        pub fn stop(&mut self) {
            if let Some(f) = self.follow.as_mut() {
                f.stop();
            }
            let _ = self.tx.send(Cmd::Stop);
            if let Some(t) = self.repeater.take() {
                let _ = t.join();
            }
            match &self.source {
                Source::Screen(s) => {
                    let _ = s.stop_capture();
                }
                Source::Device(d) => d.stop(),
            }
        }
    }

    impl Drop for DisplayRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{DisplayInfo, WindowInfo};

    fn disc() -> Discovery {
        Discovery {
            displays: vec![DisplayInfo {
                id: 3,
                bounds_points: Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1440.0 },
                scale: 2.0,
                refresh_rate: 60.0,
                is_main: true,
                is_builtin: false,
                name: "Studio Display".into(),
            }],
            windows: vec![WindowInfo {
                id: 7,
                title: "w".into(),
                app_name: "a".into(),
                bundle_id: "b".into(),
                pid: 1,
                is_on_screen: true,
                bounds: Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 },
            }],
            microphones: vec![],
            cameras: vec![],
        }
    }

    #[test]
    fn full_display_is_capped_like_screen_studio() {
        let cfg = RecordingConfig::new("/tmp/x", CaptureTarget::Display { display_id: 3 });
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!((p.width_px, p.height_px), (4096, 2304));
        assert!((p.recording_scale - 0.625).abs() < 1e-9);
        assert!(p.crop.is_none());
    }

    #[test]
    fn crop_follows_the_window_inside_the_display() {
        let display = Rect { x: 1440.0, y: 0.0, width: 2560.0, height: 1440.0 };
        // moved right and down: same size, on the window's corner, display-relative
        assert_eq!(follow_crop(Rect { x: 1640.0, y: 300.0, width: 900.0, height: 700.0 }, (800.0, 600.0), display), Rect { x: 200.0, y: 300.0, width: 800.0, height: 600.0 });
        // dragged partly off the display: stops at its edges
        assert_eq!(follow_crop(Rect { x: 3800.0, y: -40.0, width: 800.0, height: 600.0 }, (800.0, 600.0), display), Rect { x: 1760.0, y: 0.0, width: 800.0, height: 600.0 });
        assert_eq!(follow_crop(Rect { x: 1000.0, y: 1200.0, width: 800.0, height: 600.0 }, (800.0, 600.0), display), Rect { x: 0.0, y: 840.0, width: 800.0, height: 600.0 });
    }

    #[test]
    fn window_becomes_display_crop() {
        let cfg = RecordingConfig::new("/tmp/x", CaptureTarget::Window { window_id: 7 });
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!((p.width_px, p.height_px), (2400, 1800));
        assert_eq!(p.recording_scale, 0.5);
        assert_eq!(p.crop.unwrap(), Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 });
        assert_eq!(p.bounds, Rect { x: 534.0, y: 259.0, width: 1200.0, height: 900.0 });
    }

    #[test]
    fn window_stream_is_window_relative() {
        let p = window_stream_plan(1201, 801, 2.0, &disc()).unwrap();
        assert_eq!(p.bounds, Rect { x: 0.0, y: 0.0, width: 1200.0, height: 800.0 });
        assert_eq!(p.crop, Some(p.bounds));
        assert_eq!((p.width_px, p.height_px), (2400, 1600));
        assert_eq!(p.width_px as f64 * p.recording_scale, p.bounds.width);
    }

    #[test]
    fn area_is_clamped_to_display() {
        let cfg = RecordingConfig::new(
            "/tmp/x",
            CaptureTarget::Area { display_id: 3, rect_points: Rect { x: -10.0, y: 0.0, width: 100.0, height: 50.4 } },
        );
        let p = plan(&cfg, &disc()).unwrap();
        assert_eq!(p.crop.unwrap(), Rect { x: 0.0, y: 0.0, width: 90.0, height: 50.0 });
        assert_eq!((p.width_px, p.height_px), (180, 100));
    }

    #[test]
    fn bitrate_scales_with_pixels() {
        assert_eq!(target_bitrate(2400, 1800, 0.85), 16_943_358);
        assert!(target_bitrate(1200, 900, 0.85) < 5_000_000);
        let args = hls_args(2400, 1800, 60, 1, "h264_videotoolbox", 2);
        assert!(args.iter().any(|a| a == "channel-1-display-2-0000.mp4"));
        assert!(args.last().unwrap() == "channel-1-display-2.m3u8");
    }
}
