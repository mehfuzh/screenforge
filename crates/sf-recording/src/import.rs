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
                bounds_moves: vec![],
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

/// The tracks of a recording's first session, from its `metadata.json`.
struct Tracks {
    display: String,
    microphone: Option<String>,
    system_audio: Option<String>,
}

fn recorders(doc: &Value) -> Vec<Value> {
    doc["recorders"].as_array().cloned().unwrap_or_default()
}

fn first_output(doc: &Value, kind: &str) -> Option<String> {
    recorders(doc).iter().find(|r| r["type"] == kind).and_then(|r| r["sessions"][0]["outputFilename"].as_str().map(String::from))
}

fn tracks(doc: &Value) -> Result<Tracks> {
    let display = recorders(doc).into_iter().find(|r| r["type"] == "display").ok_or_else(|| Error::InvalidConfig("the recording has no screen video".into()))?;
    if display["sessions"].as_array().map_or(0, Vec::len) != 1 {
        return Err(Error::InvalidConfig("a recording that was paused and resumed cannot be extended yet".into()));
    }
    Ok(Tracks {
        display: display["sessions"][0]["outputFilename"].as_str().ok_or_else(|| Error::InvalidConfig("the screen video has no file".into()))?.to_string(),
        microphone: first_output(doc, "microphone"),
        system_audio: first_output(doc, "systemAudio"),
    })
}

/// `metadata.json` after `added_ms` more of every track (and a microphone track when the
/// recording had none and `new_mic` names its file).
fn extend_metadata(doc: &mut Value, added_ms: f64, new_mic: Option<&str>) {
    let grow = |s: &mut Value| {
        for k in ["durationMs", "processTimeEndMs", "unixEndMs"] {
            if let Some(v) = s[k].as_f64() {
                s[k] = Value::from(v + added_ms);
            }
        }
    };
    if let Some(list) = doc["recorders"].as_array_mut() {
        for r in list.iter_mut() {
            if let Some(s) = r["sessions"].get_mut(0) {
                grow(s);
            }
        }
    }
    if let Some(s) = doc["sessions"].get_mut(0) {
        grow(s);
    }
    if let Some(file) = new_mic {
        let mut session = doc["sessions"][0].clone();
        session["outputFilename"] = Value::from(file);
        let index = file.trim_start_matches("channel-").split('-').next().unwrap_or("2").to_string();
        if let Some(list) = doc["recorders"].as_array_mut() {
            list.push(serde_json::json!({ "configuration": { "deviceId": "imported" }, "id": format!("channel-{index}-microphone"), "sessions": [session], "type": "microphone" }));
        }
    }
    *doc = crate::metadata::sort_keys(doc.take());
}

/// Replace the recording's files in `dir` with those written to `tmp`: the old screen video's
/// playlist and segments go, every file in `tmp` moves in (audio tracks overwrite theirs).
fn swap_in(dir: &Path, tmp: &Path, display: &str) -> Result<()> {
    let base = display.trim_end_matches(".m3u8").to_string();
    for f in std::fs::read_dir(dir).map_err(|source| Error::Io { path: dir.to_path_buf(), source })?.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{base}-")) || name == display {
            let _ = std::fs::remove_file(f.path());
        }
    }
    for f in std::fs::read_dir(tmp).map_err(|source| Error::Io { path: tmp.to_path_buf(), source })?.flatten() {
        let to = dir.join(f.file_name());
        std::fs::rename(f.path(), &to).map_err(|source| Error::Io { path: to, source })?;
    }
    let _ = std::fs::remove_dir_all(tmp);
    Ok(())
}

/// Cut the recording in `dir` back to its first `keep_ms`: every track is shortened without
/// re-encoding, `metadata.json` shrinks to match and forgets the appended parts past the cut.
/// Used to take out videos appended with [`append_video`] that no clip uses any more.
pub fn trim_recording(dir: &Path, keep_ms: f64) -> Result<()> {
    let meta_path = dir.join(sf_project::recording::METADATA_FILE);
    let text = std::fs::read_to_string(&meta_path).map_err(|source| Error::Io { path: meta_path.clone(), source })?;
    let mut doc: Value = serde_json::from_str(&text).map_err(Error::Json)?;
    let t = tracks(&doc)?;
    let old = probe(&dir.join(&t.display))?;
    if keep_ms <= 0.0 || keep_ms >= old.duration_ms - 1.0 {
        return Ok(());
    }
    let tmp = dir.join(".trim");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|source| Error::Io { path: tmp.clone(), source })?;
    let keep = format!("{:.3}", keep_ms / 1000.0);
    let mut jobs: Vec<Vec<String>> = vec![];
    let mut video: Vec<String> = ["-i", &dir.join(&t.display).to_string_lossy(), "-t", &keep, "-map", "0:v:0", "-c:v", "copy"].map(String::from).to_vec();
    video.extend(display::hls_muxer_args(t.display.trim_end_matches(".m3u8")));
    jobs.push(video);
    for a in [&t.microphone, &t.system_audio].into_iter().flatten() {
        jobs.push(["-i", &dir.join(a).to_string_lossy(), "-t", &keep, "-map", "0:a:0", "-c:a", "copy", a].map(String::from).to_vec());
    }
    for job in jobs {
        let mut cmd = std::process::Command::new(crate::ffmpeg::ffmpeg_path()?);
        crate::ffmpeg::hide_console(&mut cmd);
        let out = cmd.args(["-y", "-nostdin", "-hide_banner", "-loglevel", "error"]).args(&job).current_dir(&tmp).output().map_err(|source| Error::Io { path: dir.to_path_buf(), source });
        match out {
            Ok(o) if o.status.success() => {}
            other => {
                let _ = std::fs::remove_dir_all(&tmp);
                let why = other.map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string()).unwrap_or_else(|e| e.to_string());
                return Err(Error::Capture(format!("ffmpeg could not shorten the recording: {why}")));
            }
        }
    }
    swap_in(dir, &tmp, &t.display)?;
    if let Some(display) = doc["recorders"].as_array_mut().and_then(|l| l.iter_mut().find(|r| r["type"] == "display")) {
        let parts: Vec<Value> = display["configuration"]["sfAppended"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p[0].as_f64().is_some_and(|s| s < keep_ms)).collect();
        if parts.is_empty() {
            if let Some(c) = display["configuration"].as_object_mut() {
                c.remove("sfAppended");
            }
        } else {
            display["configuration"]["sfAppended"] = Value::Array(parts);
        }
    }
    extend_metadata(&mut doc, keep_ms - old.duration_ms, None);
    let text = serde_json::to_string_pretty(&doc).map_err(Error::Json)?;
    std::fs::write(&meta_path, text).map_err(|source| Error::Io { path: meta_path, source })
}

/// Append the video `src` to the recording in `dir`: its picture after the screen video's end
/// (letterboxed to the recording's size and frame rate), its sound after the microphone's
/// (silence where either part has none; a microphone track is added when the recording had
/// none), the system audio padded with silence. The screen video is re-encoded once, the files
/// are swapped when everything succeeded, and `metadata.json` is extended. Returns
/// `(start ms, length ms)` of the appended part in source time.
pub fn append_video(dir: &Path, src: &Path, progress: impl Fn(f64), cancel: &AtomicBool) -> Result<(f64, f64)> {
    let meta_path = dir.join(sf_project::recording::METADATA_FILE);
    let text = std::fs::read_to_string(&meta_path).map_err(|source| Error::Io { path: meta_path.clone(), source })?;
    let mut doc: Value = serde_json::from_str(&text).map_err(Error::Json)?;
    let t = tracks(&doc)?;
    let old = probe(&dir.join(&t.display))?;
    let new = probe(src)?;
    let (od, nd) = (old.duration_ms / 1000.0, new.duration_ms / 1000.0);
    let fps = old.fps.round().clamp(1.0, 120.0);
    let (w, h) = (old.width + old.width % 2, old.height + old.height % 2);

    // a microphone track exists afterwards when either part has sound
    let new_mic = match (&t.microphone, new.has_audio) {
        (None, true) => {
            let next = recorders(&doc).iter().filter_map(|r| r["id"].as_str()?.trim_start_matches("channel-").split('-').next()?.parse::<usize>().ok()).max().unwrap_or(1) + 1;
            Some(AudioChannel { kind: "microphone", channel_index: next, device_id: None }.output_filename(0))
        }
        _ => None,
    };
    let mic_out = t.microphone.clone().or_else(|| new_mic.clone());

    let tmp = dir.join(".append");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|source| Error::Io { path: tmp.clone(), source })?;
    let mut args: Vec<String> = ["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].map(String::from).to_vec();
    let mut inputs = vec![dir.join(&t.display), src.to_path_buf()];
    let mic_in = t.microphone.as_ref().map(|m| {
        inputs.push(dir.join(m));
        inputs.len() - 1
    });
    let sys_in = t.system_audio.as_ref().map(|m| {
        inputs.push(dir.join(m));
        inputs.len() - 1
    });
    for i in &inputs {
        args.extend(["-i".into(), i.to_string_lossy().into_owned()]);
    }
    let mut graph = vec![
        format!("[0:v]fps={fps},scale={w}:{h},setsar=1,format=yuv420p[v0]"),
        format!("[1:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,fps={fps},setsar=1,format=yuv420p[v1]"),
        "[v0][v1]concat=n=2:v=1:a=0[v]".to_string(),
    ];
    if mic_out.is_some() {
        let mono = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=mono";
        graph.push(match mic_in {
            Some(i) => format!("[{i}:a]{mono}[ao]"),
            None => format!("anullsrc=r=48000:cl=mono,atrim=duration={od:.3}[ao]"),
        });
        graph.push(if new.has_audio { format!("[1:a:0]{mono}[an]") } else { format!("anullsrc=r=48000:cl=mono,atrim=duration={nd:.3}[an]") });
        graph.push("[ao][an]concat=n=2:v=0:a=1[mic]".into());
    }
    if let Some(i) = sys_in {
        graph.push(format!("[{i}:a]apad=whole_dur={:.3}[sys]", od + nd));
    }
    args.extend(["-filter_complex".into(), graph.join(";")]);
    args.extend(["-map".into(), "[v]".into()]);
    args.extend(display::hls_encode_args_for(display::DISPLAY_CHANNEL, display::target_bitrate(w, h, 0.85), display::video_encoder(), 0));
    if let Some(m) = &mic_out {
        args.extend(["-map", "[mic]", "-c:a", "aac", "-b:a", "160k"].map(String::from));
        args.push(m.clone());
    }
    if let Some(s) = &t.system_audio {
        args.extend(["-map", "[sys]", "-c:a", "aac", "-b:a", "160k"].map(String::from));
        args.push(s.clone());
    }

    let run = (|| {
        let mut cmd = std::process::Command::new(crate::ffmpeg::ffmpeg_path()?);
        crate::ffmpeg::hide_console(&mut cmd);
        let mut child = cmd.args(&args).current_dir(&tmp).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|source| Error::Io { path: src.to_path_buf(), source })?;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Capture("cancelled".into()));
                }
                if let Some(us) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")).and_then(|v| v.trim().parse::<f64>().ok()) {
                    progress((us / 1000.0).min(old.duration_ms + new.duration_ms));
                }
            }
        }
        let out = child.wait_with_output().map_err(|source| Error::Io { path: src.to_path_buf(), source })?;
        if !out.status.success() {
            return Err(Error::Capture(format!("ffmpeg could not add the video: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        Ok(())
    })();
    if let Err(e) = run {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }

    swap_in(dir, &tmp, &t.display)?;
    // where the added video sits, so it can be taken out again once no clip uses it
    if let Some(display) = doc["recorders"].as_array_mut().and_then(|l| l.iter_mut().find(|r| r["type"] == "display")) {
        let mut parts = display["configuration"]["sfAppended"].as_array().cloned().unwrap_or_default();
        parts.push(serde_json::json!([old.duration_ms, new.duration_ms]));
        display["configuration"]["sfAppended"] = Value::Array(parts);
    }
    extend_metadata(&mut doc, new.duration_ms, new_mic.as_deref());
    let text = serde_json::to_string_pretty(&doc).map_err(Error::Json)?;
    std::fs::write(&meta_path, text).map_err(|source| Error::Io { path: meta_path, source })?;
    Ok((old.duration_ms, new.duration_ms))
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

    /// A test clip made with ffmpeg (`None` when it lacks libx264).
    fn clip(path: &Path, size: &str, seconds: u32, sound: bool) -> Option<()> {
        let ffmpeg = crate::ffmpeg::ffmpeg_path().ok()?;
        let mut cmd = std::process::Command::new(ffmpeg);
        cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size={size}:rate=30:duration={seconds}")]);
        if sound {
            cmd.args(["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={seconds}"), "-c:a", "aac", "-shortest"]);
        }
        cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(path).status().ok()?.success().then_some(())
    }

    #[test]
    fn appends_a_video_after_the_recording() {
        let root = std::env::temp_dir().join(format!("sf-append-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (first, second) = (root.join("first.mov"), root.join("second.mp4"));
        if clip(&first, "320x240", 3, true).is_none() || clip(&second, "120x200", 2, false).is_none() {
            return;
        }
        let rec = root.join("rec");
        import_video(&first, &rec, |_| {}, &AtomicBool::new(false)).unwrap();
        let (start, len) = append_video(&rec, &second, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!((start - 3000.0).abs() < 150.0 && (len - 2000.0).abs() < 150.0, "{start} {len}");
        // the screen video now runs on into the appended part, at the recording's size
        let p = probe(&rec.join("channel-1-display-0.m3u8")).unwrap();
        assert!((p.duration_ms - 5000.0).abs() < 300.0, "{}", p.duration_ms);
        assert_eq!((p.width, p.height), (320, 240));
        // the microphone runs on (silence where the second clip had none) and the metadata grew
        let mic = probe_audio_ms(&rec.join("channel-2-microphone-0.m4a"));
        assert!((mic - 5000.0).abs() < 300.0, "{mic}");
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(rec.join("metadata.json")).unwrap()).unwrap();
        assert!((meta["sessions"][0]["durationMs"].as_f64().unwrap() - 5000.0).abs() < 300.0);
        assert!(!rec.join(".append").exists());

        // a silent recording gets a microphone track when the appended video has sound
        let (quiet, loud) = (root.join("quiet.mov"), root.join("loud.mov"));
        clip(&quiet, "320x240", 2, false).unwrap();
        clip(&loud, "320x240", 2, true).unwrap();
        let rec2 = root.join("rec2");
        import_video(&quiet, &rec2, |_| {}, &AtomicBool::new(false)).unwrap();
        append_video(&rec2, &loud, |_| {}, &AtomicBool::new(false)).unwrap();
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(rec2.join("metadata.json")).unwrap()).unwrap();
        let mic = meta["recorders"].as_array().unwrap().iter().find(|r| r["type"] == "microphone").expect("a microphone track");
        let file = mic["sessions"][0]["outputFilename"].as_str().unwrap();
        assert!((probe_audio_ms(&rec2.join(file)) - 4000.0).abs() < 300.0);
        let _ = std::fs::remove_dir_all(root);
    }

    fn probe_audio_ms(path: &Path) -> f64 {
        let out = std::process::Command::new(crate::ffmpeg::ffprobe_path().unwrap()).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap() * 1000.0
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
