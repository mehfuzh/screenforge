//! End-to-end tests over the whole pipeline: a synthetic recording (ffmpeg test pattern,
//! sine-tone microphone, scripted mouse/keyboard logs) is turned into a `.screenforge`
//! package, opened, edited, rendered on the GPU and exported, through both the library and
//! the `sf` binary.
//!
//! Needs ffmpeg/ffprobe on PATH (or bundled). Tests that need a GPU adapter print a note and
//! pass when none is available, like the `sf-render` tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use sf_project::schema::common::Rect;
use sf_project::v4::{Mask, Slice};
use sf_project::transcript::{Generator, TranscriptFile, TranscriptItem};
use sf_project::{Preset, ProjectPackage};
use sf_render::{Compositor, Gpu};
use sf_session::create::{create_project_from_recording, CreateOptions};
use sf_session::Session;

const DURATION_MS: f64 = 4000.0;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
/// Source-time ms of the two scripted clicks; each opens an automatic zoom.
const CLICKS_MS: [f64; 2] = [1000.0, 1300.0];

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
}

/// Record nothing, but write what the recorder would: HLS fMP4 screen video, an AAC
/// microphone track, input logs and `metadata.json`, then create a project from it.
fn fixture(auto_zooms: bool) -> Fixture {
    fixture_with(auto_zooms, None)
}

/// Like [`fixture`], with `speech` spoken (macOS `say`) on the microphone instead of a tone.
fn fixture_with(auto_zooms: bool, speech: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rec = root.join("recording");
    std::fs::create_dir_all(&rec).unwrap();
    write_media(&rec, speech);
    write_input_logs(&rec);
    write_metadata(&rec);
    let opts = CreateOptions { name: "E2E".into(), projects_folder: root.join("projects"), default_config: None, create_automatic_zooms: auto_zooms, markers_ms: vec![500.0] };
    let pkg = create_project_from_recording(&rec, opts).expect("create project");
    Fixture { project: pkg.path.clone(), root, _dir: dir }
}

fn ffmpeg(args: &[&str], cwd: &Path) {
    let ffmpeg = sf_media::ffmpeg::ffmpeg_path().expect("ffmpeg");
    let out = Command::new(ffmpeg).args(["-v", "error", "-y"]).args(args).current_dir(cwd).output().expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write_media(rec: &Path, speech: Option<&str>) {
    let video = format!("testsrc2=size={WIDTH}x{HEIGHT}:rate=60:duration={}", DURATION_MS / 1000.0);
    #[rustfmt::skip]
    ffmpeg(&[
        "-f", "lavfi", "-i", &video,
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "60",
        "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
        "-hls_fmp4_init_filename", "channel-1-display-0-0000.mp4",
        "-hls_segment_filename", "channel-1-display-0-%04d.m4s",
        "channel-1-display-0.m3u8",
    ], rec);
    let secs = format!("{}", DURATION_MS / 1000.0);
    match speech {
        Some(text) => {
            let out = Command::new("say").args(["-o", "speech.aiff", text]).current_dir(rec).output().expect("macOS `say`");
            assert!(out.status.success(), "say: {}", String::from_utf8_lossy(&out.stderr));
            ffmpeg(&["-i", "speech.aiff", "-af", "apad", "-t", &secs, "-c:a", "aac", "-ar", "48000", "-ac", "1", "channel-2-microphone-0.m4a"], rec);
            std::fs::remove_file(rec.join("speech.aiff")).unwrap();
        }
        None => {
            let tone = format!("sine=frequency=440:duration={secs}");
            ffmpeg(&["-f", "lavfi", "-i", &tone, "-c:a", "aac", "-ar", "48000", "-ac", "1", "channel-2-microphone-0.m4a"], rec);
        }
    }
}

fn write_input_logs(rec: &Path) {
    // a slow diagonal sweep, then the clicks near the top-left quarter
    let moves: Vec<_> = (0..=200)
        .map(|i| {
            let t = i as f64 * 20.0;
            let k = t / DURATION_MS;
            json!({"activeModifiers": [], "cursorId": "arrow", "processTimeMs": t, "type": "mouseMoved", "unixTimeMs": 1.0e12 + t, "x": 40.0 + 400.0 * k, "y": 40.0 + 300.0 * k})
        })
        .collect();
    let clicks: Vec<_> = CLICKS_MS
        .iter()
        .flat_map(|&t| {
            [("mouseDown", t), ("mouseUp", t + 80.0)].map(|(kind, t)| json!({"activeModifiers": [], "button": "left", "cursorId": "arrow", "processTimeMs": t, "type": kind, "unixTimeMs": 1.0e12 + t, "x": 160.0, "y": 120.0}))
        })
        .collect();
    let keys = [("keyDown", 2500.0), ("keyUp", 2560.0)].map(|(kind, t)| json!({"activeModifiers": [], "character": "a", "isARepeat": false, "processTimeMs": t, "type": kind, "unixTimeMs": 1.0e12 + t}));
    for (name, v) in [("mousemoves-0.json", json!(moves)), ("mouseclicks-0.json", json!(clicks)), ("keystrokes-0.json", json!(keys))] {
        std::fs::write(rec.join(name), v.to_string()).unwrap();
    }
}

fn write_metadata(rec: &Path) {
    let session = json!({"durationMs": DURATION_MS, "processTimeStartMs": 0.0, "processTimeEndMs": DURATION_MS, "unixStartMs": 1.0e12, "unixEndMs": 1.0e12 + DURATION_MS});
    let with = |extra: serde_json::Value| {
        let mut s = session.clone();
        s.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s
    };
    let meta = json!({
        "polyrecorderVersion": "e2e",
        "recorders": [
            {"id": "channel-0-input", "type": "input", "configuration": {"captureKeyStrokes": true},
             "sessions": [with(json!({"keyStrokesFilename": "keystrokes-0.json", "mouseClicksFilename": "mouseclicks-0.json", "mouseMovesFilename": "mousemoves-0.json"}))]},
            {"id": "channel-1-display", "type": "display", "configuration": {"displayId": 1},
             "sessions": [with(json!({"outputFilename": "channel-1-display-0.m3u8", "bounds": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT}, "displayRefreshRate": 60, "recordingScale": 1}))]},
            {"id": "channel-2-microphone", "type": "microphone", "configuration": {},
             "sessions": [with(json!({"outputFilename": "channel-2-microphone-0.m4a"}))]}
        ],
        "sessions": [session],
        "state": "complete"
    });
    std::fs::write(rec.join(sf_project::recording::METADATA_FILE), meta.to_string()).unwrap();
}

fn gpu() -> Option<Gpu> {
    match Gpu::headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("no GPU adapter ({e}); skipping");
            None
        }
    }
}

/// Render output time `at_ms`, stepping the springs from 0 like playback does.
fn render(session: &mut Session, comp: &mut Compositor, at_ms: f64, height: u32) -> (u32, u32, Vec<u8>) {
    let size = session.output_size(height);
    let mut t: f64 = 0.0;
    loop {
        let desc = session.frame_desc_at(t.min(at_ms), size).unwrap();
        if t >= at_ms {
            comp.render(&desc);
            break;
        }
        t += 1000.0 / 60.0;
    }
    (size.width, size.height, comp.read_pixels().unwrap())
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

fn distinct_colours(px: &[u8]) -> usize {
    let mut set = std::collections::HashSet::new();
    for p in px.chunks_exact(4).step_by(97) {
        set.insert([p[0] >> 4, p[1] >> 4, p[2] >> 4]);
    }
    set.len()
}

fn sf(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sf")).args(args).output().expect("run sf");
    assert!(out.status.success(), "sf {args:?} failed:\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn probe(path: &Path) -> sf_media::MediaInfo {
    sf_media::probe(path).unwrap_or_else(|e| panic!("probe {}: {e}", path.display()))
}

// ───────────────────────────── project creation ─────────────────────────────

#[test]
fn recording_becomes_a_package_with_slice_zooms_and_markers() {
    let f = fixture(true);
    let pkg = ProjectPackage::open(&f.project).unwrap();
    assert!(f.project.extension().is_some_and(|e| e == sf_project::PACKAGE_EXTENSION));
    assert!(!pkg.migrated);
    assert!(!f.root.join("recording").exists(), "the recording is moved into the package");
    assert!(pkg.recording_dir().join("channel-1-display-0.m3u8").is_file());

    let scene = &pkg.project.scenes[0];
    assert_eq!(scene.slices.len(), 1);
    assert_eq!((scene.slices[0].source_start_ms, scene.slices[0].source_end_ms), (0.0, DURATION_MS));
    assert!(!scene.zooms.is_empty(), "clicks produce automatic zooms");
    let z = &scene.zooms[0];
    assert!(z.source_start_ms <= CLICKS_MS[0] && z.source_end_ms > CLICKS_MS[1], "zoom {z:?} covers the clicks");
    assert_eq!(pkg.markers.len(), 1);

    let plain = fixture(false);
    assert!(ProjectPackage::open(&plain.project).unwrap().project.scenes[0].zooms.is_empty());
}

#[test]
fn save_and_reopen_round_trips_the_project() {
    let f = fixture(true);
    let mut pkg = ProjectPackage::open(&f.project).unwrap();
    pkg.project.config.styles.screen_border_radius = 24.0;
    pkg.project.scenes[0].masks.push(Mask::new(500.0, 1500.0, Mask::SENSITIVE_DATA, vec![Rect { x: 40.0, y: 40.0, width: 200.0, height: 120.0 }]));
    pkg.save().unwrap();
    assert!(f.project.join("project.json.bak").is_file(), "the previous project.json is kept");

    let mut again = ProjectPackage::open(&f.project).unwrap();
    assert!(again.project.last_saved_at.is_some());
    again.project.last_saved_at = pkg.project.last_saved_at;
    assert_eq!(again.project, pkg.project);
    assert_eq!(again.markers, pkg.markers);
}

// ───────────────────────────── session and rendering ─────────────────────────────

#[test]
fn session_reads_the_sources() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(true);
    let session = Session::open(&f.project, gpu).unwrap();
    assert_eq!((session.source.native_width, session.source.native_height), (WIDTH, HEIGHT));
    assert!((session.source.duration_ms - DURATION_MS).abs() < 100.0, "source duration {}", session.source.duration_ms);
    assert_eq!(session.source.microphone.len(), 1);
    assert!((session.duration_ms() - DURATION_MS).abs() < 1.0);
    assert!(!session.auto_zooms().is_empty());
    assert!(!session.timeline_zooms().is_empty());
}

#[test]
fn renders_the_recording_and_zooms_in_on_clicks() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(true);
    let mut session = Session::open(&f.project, gpu.clone()).unwrap();
    let mut comp = Compositor::new(gpu);

    let (w, h, start) = render(&mut session, &mut comp, 0.0, 360);
    assert_eq!(h, 360);
    assert_eq!(start.len(), (w * h * 4) as usize);
    assert!(distinct_colours(&start) > 8, "the frame shows the test pattern, not a flat fill");

    let (_, _, zoomed) = render(&mut session, &mut comp, 2000.0, 360);
    let (level, visible) = session.zoom_state();
    assert!(level > 1.2, "zoomed in on the clicks (level {level})");
    assert!(visible[2] < 0.9 && visible[3] < 0.9, "only part of the screen is visible: {visible:?}");

    // same instant with the zooms disabled
    for z in &mut session.pkg.project.scenes[0].zooms {
        z.is_disabled = true;
    }
    session.rebuild();
    let (_, _, flat) = render(&mut session, &mut comp, 2000.0, 360);
    assert!(session.zoom_state().0 < 1.05);
    assert!(mean_abs_diff(&zoomed, &flat) > 5.0, "the zoom changes the picture");
}

#[test]
fn background_padding_and_masks_change_the_frame() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(false);
    let mut session = Session::open(&f.project, gpu.clone()).unwrap();
    let mut comp = Compositor::new(gpu);
    let corner = |px: &[u8], w: u32| -> [u8; 3] {
        let i = ((4 * w + 4) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    let (bw, _, base) = render(&mut session, &mut comp, 1000.0, 360);

    session.pkg.project.config.output.padding_ratio01 = 0.15;
    session.pkg.project.config.styles.screen_border_radius = 20.0;
    session.rebuild();
    let (pw, _, padded) = render(&mut session, &mut comp, 1000.0, 360);
    assert_ne!(corner(&base, bw), corner(&padded, pw), "padding shows the background in the corner");

    session.pkg.project.scenes[0].masks.push(Mask::new(0.0, DURATION_MS, Mask::SENSITIVE_DATA, vec![Rect { x: 0.0, y: 0.0, width: WIDTH as f64 / 2.0, height: HEIGHT as f64 / 2.0 }]));
    session.rebuild();
    let (_, _, masked) = render(&mut session, &mut comp, 1000.0, 360);
    assert!(mean_abs_diff(&padded, &masked) > 2.0, "a sensitive-data mask blurs the screen");
}

#[test]
fn cuts_and_speed_ups_shorten_the_timeline() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(true);
    let mut pkg = ProjectPackage::open(&f.project).unwrap();
    let mut fast = Slice::new(2500.0, DURATION_MS);
    fast.time_scale = 0.5; // 2× speed
    // keep 0–1500, cut 1500–2500, play the rest at 2×
    pkg.project.scenes[0].slices = vec![Slice::new(0.0, 1500.0), fast];
    pkg.save().unwrap();

    let mut session = Session::open(&f.project, gpu.clone()).unwrap();
    assert!((session.duration_ms() - (1500.0 + 750.0)).abs() < 1.0, "duration {}", session.duration_ms());
    assert_eq!(session.timeline.output_to_source(1600.0).map(|s| s.round()), Some(2700.0));
    let mut comp = Compositor::new(gpu);
    let end = session.duration_ms() - 10.0;
    let (_, _, px) = render(&mut session, &mut comp, end, 240);
    assert!(distinct_colours(&px) > 8);
}

// ───────────────────────────── export ─────────────────────────────

fn export(project: &Path, opts: sf_export::ExportOptions) -> Option<PathBuf> {
    let gpu = gpu()?;
    let mut session = Session::open(project, gpu.clone()).unwrap();
    let mut comp = Compositor::new(gpu);
    let mut last = None;
    let out = sf_export::export(&mut session, &mut comp, &opts, |p| last = Some(p)).expect("export");
    let p = last.expect("progress is reported");
    assert_eq!(p.frame, p.total_frames);
    Some(out)
}

#[test]
fn exports_mp4_with_video_and_microphone_audio() {
    let f = fixture(true);
    let out = f.root.join("out.mp4");
    let opts = sf_export::ExportOptions { output: out.clone(), fps: 30.0, max_height: 360, ..Default::default() };
    let Some(path) = export(&f.project, opts) else { return };
    let info = probe(&path);
    let v = info.video().expect("video stream");
    assert_eq!(v.codec_name.as_deref(), Some("h264"));
    assert_eq!(v.height, Some(360));
    assert!((v.fps().unwrap() - 30.0).abs() < 0.5);
    let a = info.audio().expect("microphone audio is mixed in");
    assert_eq!(a.codec_name.as_deref(), Some("aac"));
    let d = info.duration_ms().unwrap();
    assert!((d - DURATION_MS).abs() < 200.0, "duration {d}");
}

#[test]
fn exports_a_range_without_audio() {
    let f = fixture(true);
    let opts = sf_export::ExportOptions { output: f.root.join("range.mp4"), fps: 30.0, max_height: 240, include_audio: false, range_ms: Some((1000.0, 2000.0)), ..Default::default() };
    let Some(path) = export(&f.project, opts) else { return };
    let info = probe(&path);
    assert!(info.audio().is_none());
    let d = info.duration_ms().unwrap();
    assert!((d - 1000.0).abs() < 150.0, "duration {d}");
}

fn export_short(codec: sf_media::VideoCodec, name: &str, expect: &str) -> Option<sf_media::MediaInfo> {
    let f = fixture(false);
    let opts = sf_export::ExportOptions { output: f.root.join(name), fps: 15.0, max_height: 240, codec, range_ms: Some((0.0, 1000.0)), ..Default::default() };
    let path = export(&f.project, opts)?;
    let info = probe(&path);
    assert_eq!(info.video().and_then(|v| v.codec_name.as_deref()), Some(expect), "{name}");
    Some(info)
}

#[test]
fn exports_gif() {
    if let Some(info) = export_short(sf_media::VideoCodec::Gif, "out.gif", "gif") {
        assert!(info.audio().is_none(), "GIFs have no audio");
    }
}

#[test]
fn exports_webm() {
    // WebM only allows Vorbis/Opus, so the microphone must not be muxed as AAC
    if let Some(info) = export_short(sf_media::VideoCodec::Vp9, "out.webm", "vp9") {
        assert_eq!(info.audio().and_then(|a| a.codec_name.as_deref()), Some("opus"));
    }
}

#[test]
fn exports_prores() {
    export_short(sf_media::VideoCodec::ProRes4444, "out.mov", "prores");
}

#[test]
fn export_follows_edited_timeline() {
    let f = fixture(false);
    let mut pkg = ProjectPackage::open(&f.project).unwrap();
    let mut fast = Slice::new(2000.0, DURATION_MS);
    fast.time_scale = 0.5;
    pkg.project.scenes[0].slices = vec![Slice::new(0.0, 1000.0), fast];
    pkg.save().unwrap();
    let opts = sf_export::ExportOptions { output: f.root.join("edited.mp4"), fps: 30.0, max_height: 240, ..Default::default() };
    let Some(path) = export(&f.project, opts) else { return };
    let info = probe(&path);
    let d = info.duration_ms().unwrap();
    assert!((d - 2000.0).abs() < 200.0, "1 s + 2 s at 2× ≈ 2 s, got {d}");
    assert!(info.audio().is_some(), "audio follows the slices");
}

// ───────────────────────────── presets ─────────────────────────────

#[test]
fn preset_create_and_apply_restore_the_style() {
    let f = fixture(false);
    let mut pkg = ProjectPackage::open(&f.project).unwrap();
    pkg.project.config.styles.screen_border_radius = 30.0;
    pkg.project.config.cursor.size = 64.0;
    let preset_path = f.root.join(format!("Rounded.{}", sf_project::PRESET_EXTENSION));
    Preset::create(&preset_path, "Rounded", &pkg.project.config, None).unwrap();

    pkg.project.config.styles.screen_border_radius = 0.0;
    pkg.project.config.cursor.size = 24.0;
    let crop = Rect { x: 0.1, y: 0.1, width: 0.5, height: 0.5 };
    pkg.project.config.crop.rect01 = crop;
    let preset = Preset::open(&preset_path).unwrap();
    assert_eq!(preset.name(), "Rounded");
    let dir = pkg.path.clone();
    preset.apply(&mut pkg.project, &dir).unwrap();
    assert_eq!(pkg.project.config.styles.screen_border_radius, 30.0);
    assert_eq!(pkg.project.config.cursor.size, 64.0);
    assert_eq!(pkg.project.config.crop.rect01, crop, "applying a preset keeps the crop");
    assert!(Preset::list(&f.root).iter().any(|p| p.name() == "Rounded"));
}

// ───────────────────────────── the sf binary ─────────────────────────────

#[test]
fn cli_inspect_describes_the_project() {
    let f = fixture(true);
    let p = f.project.to_str().unwrap();
    let out = sf(&["inspect", p]);
    assert!(out.contains("slices=1"), "{out}");
    assert!(out.contains("duration=4.0s"), "{out}");
    assert!(out.contains("channel-2-microphone"), "{out}");
    let json: serde_json::Value = serde_json::from_str(&sf(&["inspect", "--json", p])).unwrap();
    assert_eq!(json["scenes"][0]["slices"].as_array().unwrap().len(), 1);
}

#[test]
fn cli_opens_and_migrates_the_v3_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join(format!("v3.{}", sf_project::PACKAGE_EXTENSION));
    std::fs::create_dir_all(&pkg).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/v3");
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), pkg.join(e.file_name())).unwrap();
    }
    let p = pkg.to_str().unwrap();
    assert!(sf(&["inspect", p]).contains("migrated from v3"));
    assert!(sf(&["resave", p]).contains("migrated: true"));
    assert!(pkg.join("project.json.bak").is_file());
    let after = ProjectPackage::open(&pkg).unwrap();
    assert!(!after.migrated, "resave writes the v4 format");
    assert!(sf(&["inspect", p]).contains("format: v4"));
}

#[test]
fn cli_frame_and_export() {
    if gpu().is_none() {
        return;
    }
    let f = fixture(true);
    let p = f.project.to_str().unwrap();
    let png = f.root.join("frame.png");
    sf(&["frame", p, "--at-ms", "1500", "--height", "240", "-o", png.to_str().unwrap()]);
    let img = image::open(&png).unwrap();
    assert_eq!(img.height(), 240);

    let mp4 = f.root.join("cli.mp4");
    sf(&["export", p, "-o", mp4.to_str().unwrap(), "--fps", "24", "--height", "240", "--range", "0", "1000"]);
    let info = probe(&mp4);
    assert_eq!(info.video().and_then(|v| v.height), Some(240));
    assert!(info.audio().is_some());

    sf(&["preview", p]);
    assert!(f.project.join(sf_project::package::QUICKLOOK_PREVIEW).is_file());
}

#[test]
fn cli_preset_round_trip() {
    let f = fixture(false);
    let p = f.project.to_str().unwrap();
    let preset = f.root.join(format!("Mine.{}", sf_project::PRESET_EXTENSION));
    sf(&["preset-create", p, preset.to_str().unwrap()]);
    assert!(sf(&["presets", "--folder", f.root.to_str().unwrap()]).contains("Mine"));
    assert!(sf(&["preset-apply", p, preset.to_str().unwrap()]).contains("applied Mine"));
}

#[test]
fn cli_license_keys_sign_and_verify() {
    let keys = sf(&["license", "keygen"]);
    let get = |k: &str| keys.lines().find_map(|l| l.strip_prefix(k)).unwrap().to_string();
    let (sk, pk) = (get("SF_LICENSE_SIGNING_KEY="), get("ISSUER_PUBLIC_KEY_B64="));
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sf")).args(args).env("SF_LICENSE_SIGNING_KEY", &sk).env("SF_LICENSE_PUBLIC_KEY", &pk).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for plan in ["monthly", "lifetime"] {
        let key = run(&["license", "sign", "--email", "e2e@example.com", "--plan", plan]);
        let status = run(&["license", "verify", &key]);
        assert!(status.starts_with("Valid"), "{plan}: {status}");
        assert!(status.contains("e2e@example.com"), "{status}");
    }
    // a key signed by another issuer is rejected
    let other = sf(&["license", "keygen"]);
    let other_sk = other.lines().find_map(|l| l.strip_prefix("SF_LICENSE_SIGNING_KEY=")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_sf")).args(["license", "sign", "--email", "x@example.com"]).env("SF_LICENSE_SIGNING_KEY", other_sk).output().unwrap();
    let forged = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!run(&["license", "verify", &forged]).starts_with("Valid"));
}

// ───────────────────────────── captions ─────────────────────────────

/// Write a microphone transcript: `(start_ms, end_ms, word)`.
fn write_transcript(project: &Path, words: &[(f64, f64, &str)]) {
    let items = words.iter().enumerate().map(|(i, (s, e, t))| TranscriptItem::new(i as u32, *s, *e, *t)).collect();
    let gen = Generator { kind: "whisper".into(), model: Some("base".into()), language: Some("en".into()), prompt: None, extra: Default::default() };
    TranscriptFile::microphone(0, gen, items).save(project).unwrap();
}

const WORDS: [(f64, f64, &str); 4] = [(800.0, 1100.0, "Captions"), (1100.0, 1400.0, "are"), (1400.0, 1700.0, "working"), (1700.0, 2100.0, "now.")];

/// Mean difference between two frames over rows `[y0, y1)` (fractions of the height).
fn band_diff(a: &[u8], b: &[u8], w: u32, h: u32, y0: f64, y1: f64) -> f64 {
    let row = (w * 4) as usize;
    let (r0, r1) = ((y0 * h as f64) as usize, (y1 * h as f64) as usize);
    mean_abs_diff(&a[r0 * row..r1 * row], &b[r0 * row..r1 * row])
}

/// Render `at_ms` with the captions config changed by `edit`.
fn render_with(session: &mut Session, comp: &mut Compositor, at_ms: f64, edit: impl FnOnce(&mut sf_project::v4::CaptionsConfig)) -> (u32, u32, Vec<u8>) {
    edit(&mut session.pkg.project.config.captions);
    session.rebuild();
    render(session, comp, at_ms, 360)
}

#[test]
fn captions_are_drawn_at_the_bottom_and_follow_the_settings() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(false);
    write_transcript(&f.project, &WORDS);
    let mut session = Session::open(&f.project, gpu.clone()).unwrap();
    assert_eq!(session.captions.lines.len(), 1, "one sentence, one line");
    let mut comp = Compositor::new(gpu);

    let (w, h, off) = render_with(&mut session, &mut comp, 1500.0, |c| c.enable_transcript = false);
    let (_, _, on) = render_with(&mut session, &mut comp, 1500.0, |c| c.enable_transcript = true);
    assert!(band_diff(&off, &on, w, h, 0.75, 1.0) > 3.0, "a caption box at the bottom");
    assert!(band_diff(&off, &on, w, h, 0.0, 0.5) < 0.01, "nothing at the top");

    // before the first word and after the line has gone: no caption
    let (_, _, before) = render_with(&mut session, &mut comp, 600.0, |_| {});
    let (_, _, before_off) = render_with(&mut session, &mut comp, 600.0, |c| c.enable_transcript = false);
    assert!(mean_abs_diff(&before, &before_off) < 0.01);
    let (_, _, after) = render_with(&mut session, &mut comp, 3500.0, |c| c.enable_transcript = true);
    let (_, _, after_off) = render_with(&mut session, &mut comp, 3500.0, |c| c.enable_transcript = false);
    assert!(mean_abs_diff(&after, &after_off) < 0.01);

    // position: top centre
    let (_, _, top) = render_with(&mut session, &mut comp, 1500.0, |c| {
        c.enable_transcript = true;
        c.position01 = sf_project::schema::common::Point::new(0.5, 0.0);
    });
    assert!(band_diff(&off, &top, w, h, 0.0, 0.25) > 3.0, "moved to the top");
    assert!(band_diff(&off, &top, w, h, 0.75, 1.0) < 0.01);

    // bigger text covers more of the frame
    let (_, _, small) = render_with(&mut session, &mut comp, 1500.0, |c| {
        c.position01 = sf_project::schema::common::Point::new(0.5, 1.0);
        c.size_ratio = 0.05;
    });
    let (_, _, big) = render_with(&mut session, &mut comp, 1500.0, |c| c.size_ratio = 0.1);
    assert!(mean_abs_diff(&off, &big) > mean_abs_diff(&off, &small) * 1.5);

    // the serif and mono fonts draw differently from sans
    let (_, _, serif) = render_with(&mut session, &mut comp, 1500.0, |c| {
        c.size_ratio = 0.05;
        c.font = "serif".into();
    });
    let (_, _, mono) = render_with(&mut session, &mut comp, 1500.0, |c| c.font = "mono".into());
    assert!(mean_abs_diff(&small, &serif) > 0.05 && mean_abs_diff(&small, &mono) > 0.05);
}

#[test]
fn word_by_word_reveals_progressively() {
    let Some(gpu) = gpu() else { return };
    let f = fixture(false);
    write_transcript(&f.project, &WORDS);
    let mut session = Session::open(&f.project, gpu.clone()).unwrap();
    let mut comp = Compositor::new(gpu);
    let mut drawn = vec![];
    for t in [900.0, 1500.0, 2000.0] {
        let (w, h, off) = render_with(&mut session, &mut comp, t, |c| c.enable_transcript = false);
        let (_, _, px) = render_with(&mut session, &mut comp, t, |c| {
            c.enable_transcript = true;
            c.words_reveal = "word-by-word".into();
            c.line_growth = "centered".into();
        });
        drawn.push(band_diff(&off, &px, w, h, 0.75, 1.0));
    }
    assert!(drawn[0] < drawn[1] && drawn[1] < drawn[2], "more words, more caption: {drawn:?}");
}

#[test]
fn export_burns_in_captions() {
    let f = fixture(false);
    write_transcript(&f.project, &WORDS);
    let frame_at = |name: &str, enable: bool| -> Option<image::RgbaImage> {
        let mut pkg = ProjectPackage::open(&f.project).unwrap();
        pkg.project.config.captions.enable_transcript = enable;
        pkg.save().unwrap();
        let opts = sf_export::ExportOptions { output: f.root.join(name), fps: 30.0, max_height: 360, include_audio: false, range_ms: Some((1000.0, 2000.0)), ..Default::default() };
        let mp4 = export(&f.project, opts)?;
        let png = f.root.join(format!("{name}.png"));
        ffmpeg(&["-ss", "0.5", "-i", mp4.to_str().unwrap(), "-frames:v", "1", png.to_str().unwrap()], &f.root);
        Some(image::open(&png).unwrap().to_rgba8())
    };
    let Some(with) = frame_at("with.mp4", true) else { return };
    let without = frame_at("without.mp4", false).unwrap();
    let (w, h) = with.dimensions();
    assert!(band_diff(with.as_raw(), without.as_raw(), w, h, 0.75, 1.0) > 3.0, "the caption is in the exported video");
    assert!(band_diff(with.as_raw(), without.as_raw(), w, h, 0.0, 0.5) < 1.5, "only the caption differs");
}

#[test]
fn cli_srt_follows_the_edited_timeline() {
    if gpu().is_none() {
        return;
    }
    let f = fixture(false);
    let p = f.project.to_str().unwrap();
    let srt = f.root.join("c.srt");
    // no transcript yet
    let out = Command::new(env!("CARGO_BIN_EXE_sf")).args(["srt", p, "-o", srt.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("sf transcribe"));

    write_transcript(&f.project, &[(300.0, 700.0, "Before"), (700.0, 900.0, "the"), (900.0, 1100.0, "cut."), (1600.0, 1900.0, "Gone."), (2600.0, 3000.0, "After"), (3000.0, 3400.0, "it.")]);
    let mut pkg = ProjectPackage::open(&f.project).unwrap();
    let mut fast = Slice::new(2500.0, DURATION_MS);
    fast.time_scale = 0.5;
    pkg.project.scenes[0].slices = vec![Slice::new(0.0, 1500.0), fast];
    pkg.save().unwrap();

    sf(&["srt", p, "-o", srt.to_str().unwrap()]);
    let text = std::fs::read_to_string(&srt).unwrap();
    let cues: Vec<&str> = text.split("\n\n").filter(|c| !c.trim().is_empty()).collect();
    assert_eq!(cues.len(), 2, "{text}");
    assert!(cues[0].starts_with("1\n00:00:00,300 --> ") && cues[0].ends_with("Before the cut."), "{text}");
    // 2.6 s source → 1.5 s + 0.1 s / 2 = 1.55 s output
    assert!(cues[1].starts_with("2\n00:00:01,550 --> ") && cues[1].ends_with("After it."), "{text}");
    assert!(!text.contains("Gone"));
}

/// Real speech through Whisper. Downloads the base model (142 MB) on first run and needs the
/// macOS `say` command:
/// `cargo test -p sf-cli --test e2e -- --ignored transcribes`
#[test]
#[ignore = "downloads the Whisper base model; needs macOS `say`"]
fn transcribes_speech_into_captions() {
    let f = fixture_with(false, Some("Hello world. Captions are working."));
    let p = f.project.to_str().unwrap();
    let out = sf(&["transcribe", p, "--model", "base", "--language", "en"]);
    assert!(out.contains("microphone-session-0.json"), "{out}");
    let t = TranscriptFile::load_microphone(&f.project, 0).unwrap().expect("transcript written");
    let text = t.transcript.iter().map(|i| i.text.to_lowercase()).collect::<Vec<_>>().join(" ");
    for i in &t.transcript {
        println!("{:>6.0} {:>6.0}  {}", i.start_ms, i.end_ms, i.text);
    }
    assert!(text.contains("hello") && text.contains("captions"), "{text}");
    assert!(t.transcript.windows(2).all(|w| w[0].start_ms <= w[1].start_ms), "words in order");
    assert!(t.transcript.iter().all(|i| i.end_ms >= i.start_ms && i.end_ms <= DURATION_MS + 50.0));

    let srt = f.root.join("speech.srt");
    sf(&["srt", p, "-o", srt.to_str().unwrap()]);
    assert!(std::fs::read_to_string(&srt).unwrap().to_lowercase().contains("captions"));
}
