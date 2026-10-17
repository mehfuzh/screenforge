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

/// Windows: pointer positions recorded through the hooks must be in points (pixels / DPI
/// scale). Moves the cursor programmatically and reads the event log back.
#[cfg(target_os = "windows")]
#[test]
#[ignore = "moves the real mouse pointer"]
fn hook_positions_are_in_points() {
    use sf_recording::input::InputRecorder;
    use sf_recording::log::RecLog;
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEINPUT};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    sf_recording::win::ensure_dpi_aware();
    // SetCursorPos bypasses the hooks; SendInput goes through them like a real mouse
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f64, GetSystemMetrics(SM_CYSCREEN) as f64) };
    let send = |x: i32, y: i32| {
        let mi = MOUSEINPUT { dx: (x as f64 * 65535.0 / sw).round() as i32, dy: (y as f64 * 65535.0 / sh).round() as i32, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, time: 0, dwExtraInfo: 0 };
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi } };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    };
    let tmp = tempfile::tempdir().unwrap();
    let log = RecLog::open(tmp.path()).unwrap();
    let rec = InputRecorder::start(log, false).unwrap();
    rec.begin_session(0, tmp.path()).unwrap();
    // move the pointer ourselves (the hooks see injected moves too) and note where Windows
    // says it is in physical pixels right after
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut physical = POINT::default();
    for i in 0..5 {
        send(1000 + i * 3, 500 + i * 2);
        std::thread::sleep(Duration::from_millis(40));
        let _ = unsafe { GetCursorPos(&mut physical) };
    }
    std::thread::sleep(Duration::from_millis(200));
    rec.end_session().unwrap();
    let scale = sf_recording::win::scale_at(physical.x as f64, physical.y as f64);
    let moves = sf_project::recording::load_events(&tmp.path().join("mousemoves-0.json")).unwrap();
    println!("scale={scale} physical=({},{}) moves={:?}", physical.x, physical.y, moves.iter().map(|m| (m.x, m.y)).collect::<Vec<_>>());
    assert!(!moves.is_empty(), "no mouse moves recorded");
    let last = moves.last().unwrap();
    let (ex, ey) = (physical.x as f64 / scale, physical.y as f64 / scale);
    assert!((last.x.unwrap() - ex).abs() < 3.0 && (last.y.unwrap() - ey).abs() < 3.0, "got {:?} {:?}, expected ({ex},{ey}) at scale {scale}", last.x, last.y);
}
