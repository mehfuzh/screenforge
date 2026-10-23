//! A video file (`.mov`, `.mp4`, `.m4v`) turned into a recording directory, so it opens as a
//! project like anything recorded: its picture becomes the `display` channel (HLS/fMP4, the
//! same layout as [`crate::display`] writes) and its sound the microphone channel, so captions,
//! the transcript and voice tools work on it. There is no input log: no cursor or click zooms.
//!
//! H.264 and HEVC are copied as they are (fast, no quality loss); anything else, or a video with
//! a rotation flag (phones record sideways and say how to turn it), is re-encoded upright.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use sf_project::schema::common::Rect;

use crate::metadata::{AudioChannel, DisplayChannel, MetadataBuilder, SessionTimes};
use crate::{display, Error, RecordingMetadata, Result};

/// File extensions that can be imported.
pub const VIDEO_EXTENSIONS: [&str; 3] = ["mov", "mp4", "m4v"];

/// True for a file this module can import.
pub fn is_video(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// What ffprobe says about a video.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// Upright size (after the rotation flag).
    pub width: u32,
    pub height: u32,
    pub duration_ms: f64,
    pub fps: f64,
    pub codec: String,
    /// Degrees the player turns the picture (0, 90, 180, 270).
    pub rotation: i32,
    pub has_audio: bool,
}

impl Probe {
    /// The video can be copied into the recording without re-encoding.
    pub fn copyable(&self) -> bool {
        matches!(self.codec.as_str(), "h264" | "hevc") && self.rotation == 0
    }
}

fn fraction(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0).then_some(n / d)
}

/// Read ffprobe's JSON (`-show_streams -show_format`).
pub fn parse_probe(json: &Value) -> Result<Probe> {
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let v = streams.iter().find(|s| s["codec_type"] == "video").ok_or_else(|| Error::InvalidConfig("the file has no video".into()))?;
    let (w, h) = (v["width"].as_u64().unwrap_or(0) as u32, v["height"].as_u64().unwrap_or(0) as u32);
    if w == 0 || h == 0 {
        return Err(Error::InvalidConfig("the video has no picture size".into()));
    }
    // the display matrix (side data) or the older `rotate` tag
    let rotation = v["side_data_list"].as_array().and_then(|l| l.iter().find_map(|d| d["rotation"].as_f64())).or_else(|| v["tags"]["rotate"].as_str().and_then(|r| r.parse().ok())).unwrap_or(0.0);
    let rotation = (rotation.round() as i32).rem_euclid(360);
    let (width, height) = if rotation == 90 || rotation == 270 { (h, w) } else { (w, h) };
    let duration_s = json["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).or_else(|| v["duration"].as_str().and_then(|d| d.parse().ok())).unwrap_or(0.0);
    if duration_s <= 0.0 {
        return Err(Error::InvalidConfig("the video has no duration".into()));
    }
    Ok(Probe {
        width,
        height,
        duration_ms: duration_s * 1000.0,
        fps: v["avg_frame_rate"].as_str().and_then(fraction).filter(|f| *f > 0.0).unwrap_or(60.0),
        codec: v["codec_name"].as_str().unwrap_or("").to_string(),
        rotation,
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

pub fn probe(path: &Path) -> Result<Probe> {
    let mut cmd = std::process::Command::new(crate::ffmpeg::ffprobe_path()?);
    crate::ffmpeg::hide_console(&mut cmd);
    let out = cmd.args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"]).arg(path).output().map_err(|source| Error::Io { path: path.to_path_buf(), source })?;
    if !out.status.success() {
        return Err(Error::InvalidConfig(format!("not a video ffmpeg can read: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    parse_probe(&serde_json::from_slice(&out.stdout).map_err(|e| Error::InvalidConfig(format!("ffprobe: {e}")))?)
}

/// The ffmpeg arguments (cwd = the recording directory) for one pass writing both channels.
pub fn ffmpeg_args(src: &Path, p: &Probe, quality: f64) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats", "-i"].map(String::from).to_vec();
    args.push(src.to_string_lossy().into_owned());
    args.extend(["-map", "0:v:0"].map(String::from));
    let session = 0;
    if p.copyable() {
        let base = format!("{}-{session}", display::DISPLAY_CHANNEL);
        args.extend(["-c:v", "copy"].map(String::from));
        if p.codec == "hevc" {
            args.extend(["-tag:v", "hvc1"].map(String::from));
        }
        args.extend(
            [
                "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
                "-hls_flags", "independent_segments", "-hls_list_size", "0", "-start_number", "1",
                "-hls_fmp4_init_filename", &format!("{base}-0000.mp4"),
                "-hls_segment_filename", &format!("{base}-%04d.m4s"),
                &format!("{base}.m3u8"),
            ]
            .map(String::from),
        );
    } else {
        // re-encoded upright (ffmpeg applies the rotation flag when it decodes)
        let bitrate = display::target_bitrate(p.width, p.height, quality);
        args.extend(display::hls_encode_args_for(display::DISPLAY_CHANNEL, bitrate, display::video_encoder(), session));
    }
    if p.has_audio {
        let mic = AudioChannel { kind: "microphone", channel_index: 2, device_id: None };
        args.extend(["-map", "0:a:0", "-vn", "-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "1"].map(String::from));
        args.push(mic.output_filename(session));
    }
    args
}

/// Write `src` into `dir` (created; must not exist or be empty) as a recording. `progress` gets
/// the ms of video written so far; setting `cancel` stops it and removes `dir`.
pub fn import_video(src: &Path, dir: &Path, progress: impl Fn(f64), cancel: &AtomicBool) -> Result<RecordingMetadata> {
    let p = probe(src)?;
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(Error::OutputExists(dir.to_path_buf()));
    }
    std::fs::create_dir_all(dir).map_err(|source| Error::Io { path: dir.to_path_buf(), source })?;
    let r = (|| {
        let mut cmd = std::process::Command::new(crate::ffmpeg::ffmpeg_path()?);
        crate::ffmpeg::hide_console(&mut cmd);
        let mut child = cmd.args(ffmpeg_args(src, &p, 0.85)).current_dir(dir).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|source| Error::Io { path: src.to_path_buf(), source })?;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Capture("cancelled".into()));
                }
                // out_time_us (and the misnamed out_time_ms) are microseconds
                if let Some(us) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")).and_then(|v| v.trim().parse::<f64>().ok()) {
                    progress((us / 1000.0).min(p.duration_ms));
                }
            }
        }
        let out = child.wait_with_output().map_err(|source| Error::Io { path: src.to_path_buf(), source })?;
        if !out.status.success() {
            return Err(Error::Capture(format!("ffmpeg could not convert the video: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        let now = chrono::Utc::now().timestamp_millis() as f64;
        let meta = MetadataBuilder {
            version: format!("screenforge-import-{}", env!("CARGO_PKG_VERSION")),
            input: None,
            display: DisplayChannel {
                display_id: 0,
                crop_rect: None,
                excluded_window_ids: vec![],
                exclude_finder_desktop_icons: false,
                quality: 0.85,
                max_key_frame_interval: 120,
                bounds: Rect { x: 0.0, y: 0.0, width: p.width as f64, height: p.height as f64 },
                display_refresh_rate: p.fps,
                recording_scale: 1.0,
            },
            audio: if p.has_audio { vec![AudioChannel { kind: "microphone", channel_index: 2, device_id: Some("imported".into()) }] } else { vec![] },
            camera: None,
            sessions: vec![SessionTimes { index: 0, process_start_ms: 0.0, process_end_ms: p.duration_ms, unix_start_ms: now - p.duration_ms, unix_end_ms: now }],
        };
        meta.write(dir, "complete")
    })();
    if r.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ffprobe(codec: &str, w: u64, h: u64, rotation: Option<f64>, audio: bool) -> Value {
        let mut v = json!({ "codec_type": "video", "codec_name": codec, "width": w, "height": h, "avg_frame_rate": "60000/1001" });
        if let Some(r) = rotation {
            v["side_data_list"] = json!([{ "side_data_type": "Display Matrix", "rotation": r }]);
        }
        let mut streams = vec![v];
        if audio {
            streams.push(json!({ "codec_type": "audio", "codec_name": "aac" }));
        }
        json!({ "streams": streams, "format": { "duration": "12.5" } })
    }

    #[test]
    fn probes_size_rotation_and_audio() {
        let p = parse_probe(&ffprobe("h264", 2972, 1892, None, true)).unwrap();
        assert_eq!((p.width, p.height, p.duration_ms, p.has_audio), (2972, 1892, 12_500.0, true));
        assert!(p.copyable() && (p.fps - 59.94).abs() < 0.01);
        // a phone video stored sideways: upright size swapped, re-encoded
        let p = parse_probe(&ffprobe("hevc", 1920, 1080, Some(-90.0), false)).unwrap();
        assert_eq!((p.width, p.height, p.rotation), (1080, 1920, 270));
        assert!(!p.copyable());
        assert!(!parse_probe(&ffprobe("prores", 1920, 1080, None, false)).unwrap().copyable());
        assert!(parse_probe(&json!({ "streams": [{ "codec_type": "audio" }], "format": { "duration": "3" } })).is_err());
    }

    #[test]
    fn copies_or_encodes_into_the_recording_layout() {
        let src = Path::new("/videos/demo.mov");
        let copy = ffmpeg_args(src, &parse_probe(&ffprobe("h264", 1280, 720, None, true)).unwrap(), 0.85).join(" ");
        assert!(copy.contains("-map 0:v:0 -c:v copy") && copy.contains("channel-1-display-0.m3u8") && copy.contains("-hls_fmp4_init_filename channel-1-display-0-0000.mp4"));
        assert!(copy.ends_with("-ac 1 channel-2-microphone-0.m4a"));
        let hevc = ffmpeg_args(src, &parse_probe(&ffprobe("hevc", 1280, 720, None, false)).unwrap(), 0.85).join(" ");
        assert!(hevc.contains("-tag:v hvc1") && !hevc.contains("microphone"));
        let encode = ffmpeg_args(src, &parse_probe(&ffprobe("vp9", 1280, 720, None, false)).unwrap(), 0.85).join(" ");
        assert!(!encode.contains("copy") && encode.contains("channel-1-display-0.m3u8"));
    }

    #[test]
    fn imports_a_real_video() {
        let Ok(ffmpeg) = crate::ffmpeg::ffmpeg_path() else { return };
        let root = std::env::temp_dir().join(format!("sf-import-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("clip.mov");
        let made = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=3", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&src)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            return; // ffmpeg without libx264
        }
        let dir = root.join("rec");
        let last = std::sync::Mutex::new(0.0);
        let meta = import_video(&src, &dir, |ms| *last.lock().unwrap() = ms, &AtomicBool::new(false)).unwrap();
        assert!((meta.duration_ms() - 3000.0).abs() < 200.0, "{}", meta.duration_ms());
        assert!(dir.join("channel-1-display-0.m3u8").is_file() && dir.join("channel-1-display-0-0000.mp4").is_file());
        assert!(dir.join("channel-2-microphone-0.m4a").is_file());
        assert!(*last.lock().unwrap() > 1000.0, "progress was reported");
        // a cancelled import leaves nothing behind
        let gone = root.join("cancelled");
        assert!(import_video(&src, &gone, |_| {}, &AtomicBool::new(true)).is_err());
        assert!(!gone.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_video_files_are_imported() {
        let dir = std::env::temp_dir().join(format!("sf-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mov, txt) = (dir.join("a.MOV"), dir.join("a.txt"));
        std::fs::write(&mov, b"x").unwrap();
        std::fs::write(&txt, b"x").unwrap();
        assert!(is_video(&mov) && !is_video(&txt) && !is_video(&dir.join("missing.mp4")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
