//! Another project's recording joined onto the end of this one (a project dropped on an open
//! project). Unlike an appended video ([`crate::import::append_video`]) the other recording
//! brings its cursor, clicks and keys, so its cursor effects and click zooms keep working.
//!
//! - Screen: the other picture after this one's, scaled to this recording's size and frame
//!   rate. Both must have the same shape (aspect ratio); the screen is re-encoded once.
//! - Microphone, system audio and camera: each track is joined so the other part's sound and
//!   picture start where its screen starts (each track keeps its own offset from the screen),
//!   with silence or black where one part has none. A track only the other part has is added.
//! - Input logs: the other recording's events move to this recording's clock, and their
//!   positions from its recorded area onto this one's, so the cursor sits on the same spot of
//!   the picture. Cursor images this recording lacks are copied in.
//! - `metadata.json` grows by the other part and lists it in `sfAppended`, so it is taken out
//!   again ([`crate::import::trim_recording`]) once no clip uses it.
//!
//! The other recording is only read. A recording that was paused and resumed (several
//! sessions) cannot be joined yet, on either side.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};
use sf_project::merge::Appended;
use sf_project::schema::common::Rect;

use crate::import::{extend_metadata, probe, recorders, run_ffmpeg, swap_in, Probe};
use crate::{display, Error, Result};

const MONO: &str = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=mono";
const STEREO: &str = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
const CAMERA_FPS: u32 = 30;
/// The event logs of an input session: (metadata key, file name for a new one).
const LOGS: [(&str, &str); 3] = [("mouseMovesFilename", "mousemoves-0.json"), ("mouseClicksFilename", "mouseclicks-0.json"), ("keyStrokesFilename", "keystrokes-0.json")];

/// One recording, from its `metadata.json`.
struct Side {
    dir: PathBuf,
    doc: Value,
    /// The screen recorder's first session.
    display: Value,
    display_file: String,
    video: Probe,
    /// `processTimeStartMs` of the screen.
    start: f64,
    /// Recorded area in points, and where it moved to while recording (`[time, x, y]`).
    area: Rect,
    moves: Vec<[f64; 3]>,
}

impl Side {
    fn load(dir: &Path) -> Result<Side> {
        let path = dir.join(sf_project::recording::METADATA_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Io { path: path.clone(), source })?;
        let doc: Value = serde_json::from_str(&text).map_err(Error::Json)?;
        let rec = recorders(&doc).into_iter().find(|r| r["type"] == "display" || r["type"] == "window").ok_or_else(|| Error::InvalidConfig("the recording has no screen video".into()))?;
        if rec["sessions"].as_array().map_or(0, Vec::len) != 1 {
            return Err(Error::InvalidConfig("a recording that was paused and resumed cannot be joined yet".into()));
        }
        let display = rec["sessions"][0].clone();
        let display_file = display["outputFilename"].as_str().ok_or_else(|| Error::InvalidConfig("the screen video has no file".into()))?.to_string();
        let video = probe(&dir.join(&display_file))?;
        let rect = |v: &Value| serde_json::from_value::<Rect>(v.clone()).ok().filter(|r| r.width > 0.0 && r.height > 0.0);
        let area = rect(&display["bounds"]).or_else(|| rect(&rec["configuration"]["cropRect"])).unwrap_or(Rect { x: 0.0, y: 0.0, width: video.width as f64, height: video.height as f64 });
        let mut moves: Vec<[f64; 3]> = serde_json::from_value(rec["configuration"]["sfBoundsMoves"].clone()).unwrap_or_default();
        moves.sort_by(|a, b| a[0].total_cmp(&b[0]));
        Ok(Side { dir: dir.to_path_buf(), start: display["processTimeStartMs"].as_f64().unwrap_or(0.0), doc, display, display_file, video, area, moves })
    }

    /// The first recorder of one of `kinds` that has a session.
    fn recorder(&self, kinds: &[&str]) -> Option<Value> {
        recorders(&self.doc).into_iter().find(|r| kinds.contains(&r["type"].as_str().unwrap_or("")) && r["sessions"].get(0).is_some())
    }

    /// File and offset from the screen's start of the track of one of `kinds`.
    fn track(&self, kinds: &[&str]) -> Option<(String, f64)> {
        let s = self.recorder(kinds)?["sessions"][0].clone();
        let file = s["outputFilename"].as_str()?.to_string();
        self.dir.join(&file).is_file().then(|| (file, s["processTimeStartMs"].as_f64().map(|t| t - self.start).unwrap_or(0.0)))
    }

    /// The recorded area at process time `t`, after the moves made by then.
    fn area_at(&self, t: f64) -> Rect {
        let mut r = self.area;
        for m in self.moves.iter().take_while(|m| m[0] <= t) {
            (r.x, r.y) = (m[1], m[2]);
        }
        r
    }

    fn unix_start(&self) -> f64 {
        self.doc["sessions"][0]["unixStartMs"].as_f64().or_else(|| self.display["unixStartMs"].as_f64()).unwrap_or(0.0)
    }

    /// The next free channel number (`channel-N-…`).
    fn next_channel(&self) -> usize {
        recorders(&self.doc).iter().filter_map(|r| r["id"].as_str()?.trim_start_matches("channel-").split('-').next()?.parse::<usize>().ok()).max().unwrap_or(1) + 1
    }
}

/// `channel-1-display` for `channel-1-display-0.m3u8`.
fn channel_of(file: &str) -> Option<&str> {
    file.strip_suffix("-0.m3u8")
}

/// A session for a track added to this recording: the screen's times, so it has no offset.
fn new_session(display: &Value, file: &str) -> Value {
    let mut s = json!({ "outputFilename": file });
    for k in ["durationMs", "processTimeStartMs", "processTimeEndMs", "unixStartMs", "unixEndMs"] {
        s[k] = display[k].clone();
    }
    s
}

/// Join the recording in `other` onto the end of the one in `dir` (see the module docs).
/// `progress` gets ms of the joined video written. Returns where the other part went.
pub fn append_recording(dir: &Path, other: &Path, progress: impl Fn(f64), cancel: &AtomicBool) -> Result<Appended> {
    let a = Side::load(dir)?;
    let b = Side::load(other)?;
    let ratio = |p: &Probe| p.width as f64 / p.height as f64;
    if (ratio(&a.video) / ratio(&b.video) - 1.0).abs() > 0.01 {
        return Err(Error::InvalidConfig(format!(
            "the projects have different shapes ({}×{} and {}×{}); only recordings of the same shape can be joined",
            a.video.width, a.video.height, b.video.width, b.video.height
        )));
    }
    let (od, nd) = (a.video.duration_ms, b.video.duration_ms);
    let ns = nd / 1000.0;
    let fps = a.video.fps.round().clamp(1.0, 120.0);
    let (w, h) = (a.video.width + a.video.width % 2, a.video.height + a.video.height % 2);
    let mut doc = a.doc.clone();
    let mut at = Appended { start_ms: od, len_ms: nd, unix_shift_ms: a.unix_start() + od - b.unix_start(), ..Default::default() };

    let tmp = dir.join(".join");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|source| Error::Io { path: tmp.clone(), source })?;
    let mut inputs: Vec<PathBuf> = vec![a.dir.join(&a.display_file), b.dir.join(&b.display_file)];
    let mut input = |p: PathBuf| {
        inputs.push(p);
        inputs.len() - 1
    };
    let screen = channel_of(&a.display_file).unwrap_or(display::DISPLAY_CHANNEL).to_string();
    let mut graph = vec![
        format!("[0:v]fps={fps},scale={w}:{h},setsar=1,format=yuv420p[v0]"),
        format!("[1:v]fps={fps},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,format=yuv420p[v1]"),
        "[v0][v1]concat=n=2:v=1:a=0[v]".into(),
    ];
    let mut outputs: Vec<String> = vec!["-map".into(), "[v]".into()];
    outputs.extend(display::hls_encode_args_for(&screen, display::target_bitrate(w, h, 0.85), display::video_encoder(), 0));
    let mut next_channel = a.next_channel().max(b.next_channel());
    let mut added: Vec<Value> = vec![];
    let mut renamed: Vec<(String, String)> = vec![];

    // sound: this part's track padded or cut so the other part starts where its screen does
    for (kinds, format, layout, label) in [(&["microphone"][..], MONO, "mono", "mic"), (&["systemAudio"][..], STEREO, "stereo", "sys")] {
        let (mine, theirs) = (a.track(kinds), b.track(kinds));
        if mine.is_none() && theirs.is_none() {
            continue;
        }
        let (off_a, off_b) = (mine.as_ref().map_or(0.0, |t| t.1), theirs.as_ref().map_or(0.0, |t| t.1));
        let lead = ((od + off_b - off_a) / 1000.0).max(0.001);
        graph.push(match &mine {
            Some((f, _)) => format!("[{}:a]{format},apad,atrim=end={lead:.3},asetpts=N/SR/TB[{label}0]", input(a.dir.join(f))),
            None => format!("anullsrc=r=48000:cl={layout},atrim=duration={lead:.3}[{label}0]"),
        });
        graph.push(match &theirs {
            Some((f, _)) => format!("[{}:a]{format},asetpts=PTS-STARTPTS[{label}1]", input(b.dir.join(f))),
            None => format!("anullsrc=r=48000:cl={layout},atrim=duration={ns:.3}[{label}1]"),
        });
        graph.push(format!("[{label}0][{label}1]concat=n=2:v=0:a=1[{label}]"));
        let file = match &mine {
            Some((f, _)) => f.clone(),
            None => {
                let kind = kinds[0];
                let file = format!("channel-{next_channel}-{kind}-0.m4a");
                let config = b.recorder(kinds).map(|r| r["configuration"].clone()).unwrap_or_else(|| json!({}));
                added.push(json!({ "configuration": config, "id": format!("channel-{next_channel}-{kind}"), "sessions": [new_session(&a.display, &file)], "type": kind }));
                next_channel += 1;
                file
            }
        };
        outputs.extend(["-map".into(), format!("[{label}]"), "-c:a".into(), "aac".into(), "-b:a".into(), "160k".into(), file]);
        if label == "mic" {
            at.has_microphone = true;
            at.microphone_shift_ms = theirs.is_some().then_some(lead * 1000.0);
        } else {
            at.has_system_audio = true;
        }
    }

    // camera: black where a part has none, shown screen only there
    let cams = ["camera", "webcam"];
    let (mine, theirs) = (a.track(&cams), b.track(&cams));
    let mut camera_file = None;
    if mine.is_some() || theirs.is_some() {
        let size = mine.as_ref().map(|(f, _)| a.dir.join(f)).or_else(|| theirs.as_ref().map(|(f, _)| b.dir.join(f))).map(|p| probe(&p)).transpose()?.map(|p| (p.width + p.width % 2, p.height + p.height % 2)).unwrap_or((1280, 720));
        let (cw, ch) = size;
        let (off_a, off_b) = (mine.as_ref().map_or(0.0, |t| t.1), theirs.as_ref().map_or(0.0, |t| t.1));
        let lead = ((od + off_b - off_a) / 1000.0).max(0.001);
        let fit = format!("fps={CAMERA_FPS},scale={cw}:{ch}:force_original_aspect_ratio=decrease,pad={cw}:{ch}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,format=yuv420p");
        let black = |d: f64| format!("color=c=black:s={cw}x{ch}:r={CAMERA_FPS}:d={d:.3},format=yuv420p,setsar=1");
        graph.push(match &mine {
            Some((f, _)) => format!("[{}:v]{fit},tpad=stop_mode=clone:stop_duration={lead:.3},trim=end={lead:.3},setpts=PTS-STARTPTS[cam0]", input(a.dir.join(f))),
            None => {
                at.camera_gaps.push([0.0, od]);
                format!("{}[cam0]", black(lead))
            }
        });
        graph.push(match &theirs {
            Some((f, _)) => format!("[{}:v]{fit},setpts=PTS-STARTPTS[cam1]", input(b.dir.join(f))),
            None => {
                at.camera_gaps.push([od, od + nd]);
                format!("{}[cam1]", black(ns))
            }
        });
        graph.push("[cam0][cam1]concat=n=2:v=1:a=0[cam]".into());
        let channel = match &mine {
            Some((f, _)) => channel_of(f).map(String::from).unwrap_or_else(|| f.rsplit_once('.').map_or(f.clone(), |(s, _)| s.to_string()).trim_end_matches("-0").to_string()),
            None => {
                let c = format!("channel-{next_channel}-camera");
                let config = b.recorder(&cams).map(|r| r["configuration"].clone()).unwrap_or_else(|| json!({}));
                let kind = b.recorder(&cams).and_then(|r| r["type"].as_str().map(String::from)).unwrap_or_else(|| "camera".into());
                added.push(json!({ "configuration": config, "id": c, "sessions": [new_session(&a.display, &format!("{c}-0.m3u8"))], "type": kind }));
                c
            }
        };
        let file = format!("{channel}-0.m3u8");
        if let Some((old, _)) = &mine {
            if *old != file {
                renamed.push((old.clone(), file.clone()));
            }
        }
        outputs.extend(["-map".into(), "[cam]".into()]);
        outputs.extend(display::hls_encode_args_for(&channel, display::target_bitrate(cw, ch, 0.85), display::video_encoder(), 0));
        camera_file = Some((mine.map(|m| m.0), file));
    }

    let mut args: Vec<String> = ["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].map(String::from).to_vec();
    for i in &inputs {
        args.extend(["-i".into(), i.to_string_lossy().into_owned()]);
    }
    args.extend(["-filter_complex".into(), graph.join(";")]);
    args.extend(outputs);
    let joined = run_ffmpeg(&args, &tmp, od + nd, &progress, cancel, "join the projects").and_then(|()| join_input(&a, &b, &tmp, &mut doc, &mut added));
    if let Err(e) = joined {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }

    let new_screen = format!("{screen}-0.m3u8");
    let mut videos = vec![a.display_file.as_str()];
    if let Some((Some(old), _)) = &camera_file {
        videos.push(old);
    }
    swap_in(dir, &tmp, &videos)?;
    copy_cursor_images(&a, &b)?;

    // metadata: new file names, the added tracks, the joined part and the longer sessions
    if let Some(list) = doc["recorders"].as_array_mut() {
        for r in list.iter_mut() {
            // the cursor recorder has no sessions
            let Some(s) = r.get_mut("sessions").and_then(|s| s.get_mut(0)) else { continue };
            match s["outputFilename"].as_str() {
                Some(f) if f == a.display_file => s["outputFilename"] = new_screen.clone().into(),
                Some(f) => {
                    if let Some((_, new)) = renamed.iter().find(|(old, _)| old == f) {
                        s["outputFilename"] = new.clone().into();
                    }
                }
                None => {}
            }
        }
        list.extend(added);
        if let Some(screen) = list.iter_mut().find(|r| r["type"] == "display" || r["type"] == "window") {
            let mut parts = screen["configuration"]["sfAppended"].as_array().cloned().unwrap_or_default();
            parts.push(json!([od, nd]));
            screen["configuration"]["sfAppended"] = Value::Array(parts);
        }
    }
    extend_metadata(&mut doc, nd, None);
    let meta_path = dir.join(sf_project::recording::METADATA_FILE);
    let text = serde_json::to_string_pretty(&doc).map_err(Error::Json)?;
    std::fs::write(&meta_path, text).map_err(|source| Error::Io { path: meta_path, source })?;
    Ok(at)
}

/// Write the joined input logs (and `cursors.json`) into `tmp`. The other part's events move
/// to this recording's clock and their positions onto this recording's area; events of this
/// recording past its screen's end (left by an earlier cut) are dropped. Adds an input or
/// cursor recorder to `added` when only the other recording has one.
fn join_input(a: &Side, b: &Side, tmp: &Path, doc: &mut Value, added: &mut Vec<Value>) -> Result<()> {
    let Some(theirs) = b.recorder(&["input"]) else { return Ok(()) };
    let their_session = &theirs["sessions"][0];
    let their_start = their_session["processTimeStartMs"].as_f64().unwrap_or(b.start);
    let mine = a.recorder(&["input"]);
    let my_start = mine.as_ref().and_then(|r| r["sessions"][0]["processTimeStartMs"].as_f64()).unwrap_or(a.start);
    let end = my_start + a.video.duration_ms;
    // this recording's area once it stopped moving, which is where the other part plays
    let target = a.area_at(f64::INFINITY);

    let mut session = mine.as_ref().map(|r| r["sessions"][0].clone()).unwrap_or_else(|| new_session(&a.display, ""));
    if let Some(o) = session.as_object_mut() {
        o.remove("outputFilename");
    }
    for (key, default) in LOGS {
        let name = session[key].as_str().unwrap_or(default).to_string();
        session[key] = name.clone().into();
        let mut events: Vec<Value> = read_events(&a.dir.join(&name))?.into_iter().filter(|e| e["processTimeMs"].as_f64().is_some_and(|t| t < end)).collect();
        let Some(file) = their_session[key].as_str() else {
            write_events(&tmp.join(&name), &events)?;
            continue;
        };
        for mut e in read_events(&b.dir.join(file))? {
            let Some(t) = e["processTimeMs"].as_f64() else { continue };
            let moved = my_start + a.video.duration_ms + (t - their_start);
            e["processTimeMs"] = moved.into();
            if let Some(u) = e["unixTimeMs"].as_f64() {
                e["unixTimeMs"] = (u + moved - t).into();
            }
            if let (Some(x), Some(y)) = (e["x"].as_f64(), e["y"].as_f64()) {
                let from = b.area_at(t);
                e["x"] = (target.x + (x - from.x) * target.width / from.width).into();
                e["y"] = (target.y + (y - from.y) * target.height / from.height).into();
            }
            events.push(e);
        }
        write_events(&tmp.join(&name), &events)?;
    }
    if mine.is_none() {
        added.push(json!({ "configuration": theirs["configuration"].clone(), "id": "channel-0-input", "sessions": [session], "type": "input" }));
    }

    // cursor images: the other recording's ones this one lacks
    let Some(their_cursors) = b.recorder(&["cursor"]).or_else(|| recorders(&b.doc).into_iter().find(|r| r["type"] == "cursor")) else { return Ok(()) };
    let info = |r: &Value| r["cursorsInfoFile"].as_str().unwrap_or("cursors.json").to_string();
    let my_cursors = recorders(doc).into_iter().find(|r| r["type"] == "cursor");
    let mut list: Vec<Value> = my_cursors.as_ref().map(|r| read_events(&a.dir.join(info(r)))).transpose()?.unwrap_or_default();
    for c in read_events(&b.dir.join(info(&their_cursors)))? {
        if !list.iter().any(|x| x["id"] == c["id"]) {
            list.push(c);
        }
    }
    let name = my_cursors.as_ref().map(info).unwrap_or_else(|| "cursors.json".into());
    write_events(&tmp.join(name), &list)?;
    if my_cursors.is_none() {
        added.push(json!({ "configuration": {}, "cursorImagesFolder": "cursors", "cursorsInfoFile": "cursors.json", "id": "channel-0-cursor", "type": "cursor" }));
    }
    Ok(())
}

/// Copy the other recording's cursor images that this one lacks (after the swap, so the
/// folder may be new).
fn copy_cursor_images(a: &Side, b: &Side) -> Result<()> {
    let folder = |s: &Side| recorders(&s.doc).into_iter().find(|r| r["type"] == "cursor").and_then(|r| r["cursorImagesFolder"].as_str().map(String::from));
    let Some(from) = folder(b).map(|f| b.dir.join(f)) else { return Ok(()) };
    let to = a.dir.join(folder(a).unwrap_or_else(|| "cursors".into()));
    let Ok(entries) = std::fs::read_dir(&from) else { return Ok(()) };
    std::fs::create_dir_all(&to).map_err(|source| Error::Io { path: to.clone(), source })?;
    for e in entries.flatten() {
        let dest = to.join(e.file_name());
        if e.path().is_file() && !dest.exists() {
            std::fs::copy(e.path(), &dest).map_err(|source| Error::Io { path: dest, source })?;
        }
    }
    Ok(())
}

/// A JSON array file (an input log or `cursors.json`); empty when there is none.
fn read_events(path: &Path) -> Result<Vec<Value>> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(Error::Json),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(source) => Err(Error::Io { path: path.to_path_buf(), source }),
    }
}

/// One object per line, like the recorder writes them.
fn write_events(path: &Path, events: &[Value]) -> Result<()> {
    let lines: Vec<String> = events.iter().map(|e| serde_json::to_string(e).map_err(Error::Json)).collect::<Result<_>>()?;
    let text = format!("[\n{}\n]", lines.join(",\n"));
    std::fs::write(path, text).map_err(|source| Error::Io { path: path.to_path_buf(), source })
}

/// Write the input logs of the recording in `dir` cut at `keep_ms` of the screen into `tmp`
/// (for [`crate::import::trim_recording`]).
pub(crate) fn trim_input_logs(dir: &Path, tmp: &Path, doc: &Value, keep_ms: f64) -> Result<()> {
    let Some(input) = recorders(doc).into_iter().find(|r| r["type"] == "input") else { return Ok(()) };
    let s = &input["sessions"][0];
    let start = recorders(doc).into_iter().find(|r| r["type"] == "display" || r["type"] == "window").and_then(|r| r["sessions"][0]["processTimeStartMs"].as_f64());
    let Some(start) = s["processTimeStartMs"].as_f64().or(start) else { return Ok(()) };
    for (key, _) in LOGS {
        let Some(name) = s[key].as_str() else { continue };
        if !dir.join(name).is_file() {
            continue;
        }
        let kept: Vec<Value> = read_events(&dir.join(name))?.into_iter().filter(|e| e["processTimeMs"].as_f64().is_some_and(|t| t < start + keep_ms)).collect();
        write_events(&tmp.join(name), &kept)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_video;

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

    fn meta(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("metadata.json")).unwrap()).unwrap()
    }

    /// Give the imported recording in `dir` an input log, a recorded area and a cursor image,
    /// as a screen recording has.
    fn add_input(dir: &Path, area: Rect, events: Value) {
        let mut doc = meta(dir);
        let start = doc["sessions"][0]["processTimeStartMs"].as_f64().unwrap();
        let mut session = doc["sessions"][0].clone();
        session["mouseMovesFilename"] = "mousemoves-0.json".into();
        session["mouseClicksFilename"] = "mouseclicks-0.json".into();
        session["keyStrokesFilename"] = "keystrokes-0.json".into();
        let list = doc["recorders"].as_array_mut().unwrap();
        list.push(json!({ "configuration": {}, "id": "channel-0-input", "sessions": [session], "type": "input" }));
        list.push(json!({ "configuration": {}, "cursorImagesFolder": "cursors", "cursorsInfoFile": "cursors.json", "id": "channel-0-cursor", "type": "cursor" }));
        let screen = list.iter_mut().find(|r| r["type"] == "display").unwrap();
        screen["sessions"][0]["bounds"] = serde_json::to_value(area).unwrap();
        std::fs::write(dir.join("metadata.json"), doc.to_string()).unwrap();
        let mut events = events.as_array().unwrap().clone();
        for e in &mut events {
            e["processTimeMs"] = (e["processTimeMs"].as_f64().unwrap() + start).into();
        }
        write_events(&dir.join("mousemoves-0.json"), &events).unwrap();
        write_events(&dir.join("mouseclicks-0.json"), &[]).unwrap();
        write_events(&dir.join("keystrokes-0.json"), &[]).unwrap();
        std::fs::create_dir_all(dir.join("cursors")).unwrap();
        let id = if area.x == 0.0 { "arrow" } else { "custom" };
        std::fs::write(dir.join("cursors").join(format!("{id}.png")), b"png").unwrap();
        write_events(&dir.join("cursors.json"), &[json!({ "id": id })]).unwrap();
    }

    #[test]
    fn joins_two_recordings_with_their_cursor() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let (first, second) = (root.join("first.mov"), root.join("second.mov"));
        if clip(&first, "320x240", 3, true).is_none() || clip(&second, "640x480", 2, true).is_none() {
            return; // no ffmpeg with libx264
        }
        let (a, b) = (root.join("a"), root.join("b"));
        import_video(&first, &a, |_| {}, &AtomicBool::new(false)).unwrap();
        import_video(&second, &b, |_| {}, &AtomicBool::new(false)).unwrap();
        // the same shape at twice the size, recorded on another part of the screen
        add_input(&a, Rect { x: 0.0, y: 0.0, width: 320.0, height: 240.0 }, json!([{ "type": "mouseMoved", "processTimeMs": 1000.0, "unixTimeMs": 1.0, "x": 10.0, "y": 20.0 }]));
        add_input(&b, Rect { x: 1000.0, y: 500.0, width: 640.0, height: 480.0 }, json!([{ "type": "mouseMoved", "processTimeMs": 500.0, "unixTimeMs": 1.0, "x": 1320.0, "y": 740.0 }]));
        let before_b = std::fs::read_to_string(b.join("metadata.json")).unwrap();

        let at = append_recording(&a, &b, |_| {}, &AtomicBool::new(false)).unwrap();
        assert!((at.start_ms - 3000.0).abs() < 150.0 && (at.len_ms - 2000.0).abs() < 150.0, "{at:?}");
        assert!(at.has_microphone && at.microphone_shift_ms.is_some_and(|s| (s - at.start_ms).abs() < 150.0));
        let p = probe(&a.join("channel-1-display-0.m3u8")).unwrap();
        assert!((p.duration_ms - 5000.0).abs() < 300.0 && (p.width, p.height) == (320, 240), "{p:?}");

        // the other cursor sits at the middle of the picture, 0.5 s into the other part
        let doc = meta(&a);
        let start = doc["recorders"].as_array().unwrap().iter().find(|r| r["type"] == "input").unwrap()["sessions"][0]["processTimeStartMs"].as_f64().unwrap();
        let moves = read_events(&a.join("mousemoves-0.json")).unwrap();
        assert_eq!(moves.len(), 2);
        assert!((moves[1]["processTimeMs"].as_f64().unwrap() - start - at.start_ms - 500.0).abs() < 1.0);
        assert_eq!((moves[1]["x"].as_f64(), moves[1]["y"].as_f64()), (Some(160.0), Some(120.0)));
        // its cursor image came along; the joined part is recorded for a later cut
        assert!(a.join("cursors/custom.png").is_file() && a.join("cursors/arrow.png").is_file());
        assert_eq!(read_events(&a.join("cursors.json")).unwrap().len(), 2);
        let screen = doc["recorders"].as_array().unwrap().iter().find(|r| r["type"] == "display").unwrap().clone();
        assert_eq!(screen["configuration"]["sfAppended"].as_array().unwrap().len(), 1);
        assert!(!a.join(".join").exists());
        assert_eq!(std::fs::read_to_string(b.join("metadata.json")).unwrap(), before_b, "the other recording is only read");

        // cutting the joined part off again takes its events with it
        crate::import::trim_recording(&a, at.start_ms).unwrap();
        assert_eq!(read_events(&a.join("mousemoves-0.json")).unwrap().len(), 1);
    }

    #[test]
    fn refuses_another_shape() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let (first, second) = (root.join("first.mov"), root.join("second.mov"));
        if clip(&first, "320x240", 1, false).is_none() || clip(&second, "240x320", 1, false).is_none() {
            return;
        }
        let (a, b) = (root.join("a"), root.join("b"));
        import_video(&first, &a, |_| {}, &AtomicBool::new(false)).unwrap();
        import_video(&second, &b, |_| {}, &AtomicBool::new(false)).unwrap();
        let e = append_recording(&a, &b, |_| {}, &AtomicBool::new(false)).unwrap_err().to_string();
        assert!(e.contains("different shapes"), "{e}");
    }

    #[test]
    fn camera_and_sound_only_on_the_other_side_are_added() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let (first, second, cam) = (root.join("first.mov"), root.join("second.mov"), root.join("cam.mov"));
        if clip(&first, "320x240", 2, false).is_none() || clip(&second, "320x240", 2, true).is_none() || clip(&cam, "160x120", 2, false).is_none() {
            return;
        }
        let (a, b) = (root.join("a"), root.join("b"));
        import_video(&first, &a, |_| {}, &AtomicBool::new(false)).unwrap();
        import_video(&second, &b, |_| {}, &AtomicBool::new(false)).unwrap();
        // the other recording has a camera (HLS, like the recorder writes)
        let out = std::process::Command::new(crate::ffmpeg::ffmpeg_path().unwrap())
            .args(["-v", "error", "-i"])
            .arg(&cam)
            .args(["-c:v", "copy"])
            .args(display::hls_muxer_args("channel-3-camera-0"))
            .current_dir(&b)
            .status()
            .unwrap();
        assert!(out.success());
        let mut doc = meta(&b);
        let session = new_session(&doc["recorders"].as_array().unwrap().iter().find(|r| r["type"] == "display").unwrap()["sessions"][0].clone(), "channel-3-camera-0.m3u8");
        doc["recorders"].as_array_mut().unwrap().push(json!({ "configuration": {}, "id": "channel-3-camera", "sessions": [session], "type": "camera" }));
        std::fs::write(b.join("metadata.json"), doc.to_string()).unwrap();

        let at = append_recording(&a, &b, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(at.camera_gaps, vec![[0.0, at.start_ms]]);
        assert!(at.has_microphone && !at.has_system_audio);
        let doc = meta(&a);
        let rec = |kind: &str| doc["recorders"].as_array().unwrap().iter().find(|r| r["type"] == kind).cloned().unwrap_or_else(|| panic!("no {kind} track"));
        let camera = rec("camera")["sessions"][0]["outputFilename"].as_str().unwrap().to_string();
        let mic = rec("microphone")["sessions"][0]["outputFilename"].as_str().unwrap().to_string();
        let c = probe(&a.join(&camera)).unwrap();
        assert!((c.duration_ms - 4000.0).abs() < 300.0 && (c.width, c.height) == (160, 120), "{c:?}");
        assert!(a.join(&mic).is_file());
    }
}
