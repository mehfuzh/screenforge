//! End-to-end smoke test: record a few seconds of the main display into a temp dir and
//! validate the output with ffprobe / sf-media / sf-project.
//!
//! Needs Screen Recording permission for the terminal (and Input Monitoring for the event
//! logs; without it the test records video only). Run with:
//!
//! ```sh
//! cargo test -p sf-recording --test smoke -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Duration;

use sf_project::recording::{load_cursors, InputLog, RecordingMetadata};
use sf_recording::{discover, permissions, CaptureTarget, Recorder, RecordingConfig};

fn ffprobe_ok(path: &Path) -> bool {
    let Ok(ffprobe) = sf_recording::ffmpeg::ffprobe_path() else { return false };
    std::process::Command::new(ffprobe)
        .args(["-v", "error", "-show_streams"])
        .arg(path)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("codec_name=h264"))
        .unwrap_or(false)
}

#[test]
#[ignore = "records the real screen; needs Screen Recording permission"]
fn records_main_display_for_three_seconds() {
    assert!(permissions::screen_recording().is_granted(), "grant Screen Recording to the terminal first");
    let d = discover();
    // SF_SMOKE_DISPLAY=<name> (e.g. eDP-1) records that display instead of the main one
    let display = match std::env::var("SF_SMOKE_DISPLAY") {
        Ok(name) => d.displays.iter().find(|x| x.name == name).expect("SF_SMOKE_DISPLAY names a display"),
        Err(_) => d.main_display().expect("a display"),
    };
    println!("display {:?}", display);

    // SF_SMOKE_OUT=<dir> keeps the recording for inspection instead of a temp dir.
    let tmp = tempfile::tempdir().unwrap();
    let out = match std::env::var("SF_SMOKE_OUT") {
        Ok(p) => {
            let _ = std::fs::remove_dir_all(&p);
            std::path::PathBuf::from(p)
        }
        Err(_) => tmp.path().join("recording"),
    };
    let mut cfg = RecordingConfig::new(&out, CaptureTarget::Display { display_id: display.id });
    cfg.capture_input = permissions::input_monitoring().is_granted();
    cfg.capture_system_audio = std::env::var("SF_SMOKE_SYSTEM_AUDIO").is_ok();
    cfg.microphone = std::env::var("SF_SMOKE_MIC").ok();
    // SF_SMOKE_CAMERA=<camera name> also records that webcam (needs Camera permission)
    cfg.camera = std::env::var("SF_SMOKE_CAMERA").ok();
    println!("input events: {}", cfg.capture_input);

    let mut rec = Recorder::start(cfg).expect("start");
    std::thread::sleep(Duration::from_millis(1500));
    rec.add_marker();
    rec.pause().expect("pause");
    std::thread::sleep(Duration::from_millis(300));
    rec.resume().expect("resume");
    std::thread::sleep(Duration::from_millis(1500));
    let elapsed = rec.elapsed_ms();
    assert!(elapsed > 2500.0 && elapsed < 4500.0, "elapsed {elapsed}");
    let meta = rec.finish().expect("finish");

    // metadata
    let loaded = RecordingMetadata::load(&out).expect("metadata.json parses");
    assert_eq!(meta, loaded);
    assert_eq!(loaded.state.as_deref(), Some("complete"));
    assert_eq!(loaded.sessions.len(), 2);
    let disp = loaded.display().expect("display recorder");
    assert_eq!(disp.sessions.len(), 2);
    assert_eq!(disp.display_id(), Some(display.id as u64));
    println!("metadata: {}", std::fs::read_to_string(out.join("metadata.json")).unwrap());

    // media: every session playlist decodes
    for (i, s) in disp.sessions.iter().enumerate() {
        let playlist = s.media_path(&out).expect("playlist exists");
        assert!(playlist.ends_with(format!("channel-1-display-{i}.m3u8")));
        assert!(out.join(format!("channel-1-display-{i}-0000.mp4")).exists(), "init segment");
        assert!(out.join(format!("channel-1-display-{i}-0001.m4s")).exists(), "first media segment");
        assert!(ffprobe_ok(&playlist), "ffprobe decodes {}", playlist.display());
        let mut reader = sf_media::VideoReader::open(&playlist, sf_media::VideoReaderOptions::default()).expect("VideoReader opens m3u8");
        let frame = reader.next_frame().expect("decode").expect("a frame");
        assert_eq!((frame.width, frame.height), (reader.native_width, reader.native_height));
        let expected_w = (display.bounds_points.width * display.scale).round().min(4096.0) as u32 & !1;
        assert_eq!(frame.width, expected_w);
        assert!((reader.duration_ms - s.duration_ms).abs() < 400.0, "video {} ms vs session {} ms", reader.duration_ms, s.duration_ms);
        let text = std::fs::read_to_string(&playlist).unwrap();
        assert!(text.contains("#EXT-X-PLAYLIST-TYPE:VOD") && text.contains("#EXT-X-ENDLIST") && text.contains("#EXT-X-MAP:URI="));
    }

    // audio: one .m4a per session whose duration matches the session
    for rec in loaded.recorders.iter().filter(|r| r.kind == "microphone" || r.kind == "systemAudio") {
        for s in &rec.sessions {
            let path = s.media_path(&out).expect("audio file");
            let info = sf_media::probe(&path).expect("probe m4a");
            let a = info.audio().expect("audio stream");
            assert_eq!(a.codec_name.as_deref(), Some("aac"));
            let dur = info.duration_ms().unwrap_or(0.0);
            assert!((dur - s.duration_ms).abs() < 150.0, "{}: audio {dur} ms vs session {} ms", rec.id, s.duration_ms);
        }
    }

    // camera: one HLS playlist per session, as long as the session
    if let Some(cam) = loaded.camera() {
        assert_eq!(cam.sessions.len(), 2);
        for (i, s) in cam.sessions.iter().enumerate() {
            let playlist = s.media_path(&out).expect("camera playlist");
            assert!(playlist.ends_with(format!("{}-{i}.m3u8", cam.id)));
            assert!(ffprobe_ok(&playlist), "ffprobe decodes {}", playlist.display());
            let reader = sf_media::VideoReader::open(&playlist, sf_media::VideoReaderOptions::default()).expect("VideoReader opens camera m3u8");
            assert!(reader.native_width > 0 && reader.native_width <= 1280);
            assert!((reader.duration_ms - s.duration_ms).abs() < 400.0, "camera {} ms vs session {} ms", reader.duration_ms, s.duration_ms);
        }
    } else {
        assert!(std::env::var("SF_SMOKE_CAMERA").is_err(), "camera requested but not recorded");
    }

    // input + cursors
    if loaded.input().is_some() {
        let input = loaded.input().unwrap();
        for s in &input.sessions {
            let log = InputLog::load(&out, s).expect("event logs parse");
            println!("session moves={} clicks={} keys={}", log.mouse_moves.len(), log.mouse_clicks.len(), log.key_strokes.len());
        }
        let cursors = load_cursors(&out).expect("cursors.json");
        assert!(cursors.iter().any(|c| c.id == "arrow"));
        for c in &cursors {
            assert!(out.join("cursors").join(format!("{}.png", c.id)).exists(), "png for {}", c.id);
        }
        println!("cursors: {:?}", cursors.iter().map(|c| c.id.as_str()).collect::<Vec<_>>());
    }
    assert!(out.join("polyrecorder.log").exists());
    println!("markers {:?}", meta.duration_ms());
}

/// Wayland: pick a window in the screen-sharing dialog and record it for three seconds. Move
/// the pointer over the window while it records. Run with:
///
/// ```sh
/// cargo test -p sf-recording --test smoke records_picked_window -- --ignored --nocapture
/// ```
#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens the screen-sharing dialog; pick a window there"]
fn records_picked_window() {
    assert!(sf_recording::portal::is_wayland(), "needs a Wayland session");
    let tmp = tempfile::tempdir().unwrap();
    let out = match std::env::var("SF_SMOKE_OUT") {
        Ok(p) => {
            let _ = std::fs::remove_dir_all(&p);
            std::path::PathBuf::from(p)
        }
        Err(_) => tmp.path().join("recording"),
    };
    let mut cfg = RecordingConfig::new(&out, CaptureTarget::PortalWindow);
    cfg.capture_input = true;
    let rec = Recorder::start(cfg).expect("start");
    let plan = rec.display_plan().clone();
    println!("plan {plan:?}");
    assert_eq!((plan.bounds.x, plan.bounds.y), (0.0, 0.0), "window-relative bounds");
    std::thread::sleep(Duration::from_secs(3));
    rec.finish().expect("finish");

    let loaded = RecordingMetadata::load(&out).expect("metadata.json parses");
    let disp = loaded.display().expect("display recorder");
    assert_eq!(disp.sessions.len(), 1);
    let playlist = disp.sessions[0].media_path(&out).expect("playlist exists");
    assert!(ffprobe_ok(&playlist), "ffprobe decodes {}", playlist.display());
    let mut reader = sf_media::VideoReader::open(&playlist, sf_media::VideoReaderOptions::default()).expect("VideoReader opens m3u8");
    let frame = reader.next_frame().expect("decode").expect("a frame");
    assert_eq!((frame.width, frame.height), (plan.width_px, plan.height_px));
    let input = loaded.input().expect("input recorder");
    let log = InputLog::load(&out, &input.sessions[0]).expect("event logs parse");
    println!("moves={} (window-relative points)", log.mouse_moves.len());
}
