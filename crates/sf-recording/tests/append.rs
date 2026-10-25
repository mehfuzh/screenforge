//! Appending a video to a recording and taking it out again, with real files (needs ffmpeg).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use sf_project::recording::RecordingMetadata;
use sf_recording::import;

/// A `secs` long test video with a tone at `path`.
fn make_video(path: &Path, secs: u32) {
    let ffmpeg = sf_recording::ffmpeg::ffmpeg_path().expect("ffmpeg");
    let ok = std::process::Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", &format!("testsrc=size=320x240:rate=30:duration={secs}"), "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}")])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(path)
        .status()
        .expect("run ffmpeg")
        .success();
    assert!(ok, "could not make {}", path.display());
}

/// Length of any media file, from ffprobe.
fn media_ms(path: &Path) -> f64 {
    let out = std::process::Command::new(sf_recording::ffmpeg::ffprobe_path().unwrap())
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap() * 1000.0
}

fn display_ms(dir: &Path) -> f64 {
    let m = RecordingMetadata::load(dir).unwrap();
    let file = dir.join(m.display().unwrap().sessions[0].output_filename.clone().unwrap());
    import::probe(&file).unwrap().duration_ms
}

#[test]
fn an_appended_video_can_be_taken_out_again() {
    if sf_recording::ffmpeg::ffmpeg_path().is_err() {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (first, second) = (tmp.path().join("first.mp4"), tmp.path().join("second.mp4"));
    make_video(&first, 3);
    make_video(&second, 2);
    let dir = tmp.path().join("recording");
    let cancel = AtomicBool::new(false);
    import::import_video(&first, &dir, |_| {}, &cancel).unwrap();

    let (start, len) = import::append_video(&dir, &second, |_| {}, &cancel).unwrap();
    assert!((display_ms(&dir) - 5000.0).abs() < 150.0, "appended: {}", display_ms(&dir));
    let meta = RecordingMetadata::load(&dir).unwrap();
    assert_eq!(meta.display().unwrap().appended(), vec![[start, len]]);

    import::trim_recording(&dir, start).unwrap();
    assert!((display_ms(&dir) - start).abs() < 150.0, "trimmed to {} of {start}", display_ms(&dir));
    let meta = RecordingMetadata::load(&dir).unwrap();
    let d = meta.display().unwrap();
    assert!(d.appended().is_empty(), "the part is forgotten");
    assert!((d.sessions[0].duration_ms - start).abs() < 1.0);
    let mic = meta.microphone().unwrap();
    let mic_ms = media_ms(&dir.join(mic.sessions[0].output_filename.clone().unwrap()));
    assert!((mic_ms - start).abs() < 150.0, "microphone trimmed to {mic_ms}");
    assert!(!dir.join(".trim").exists());
    // cut without re-encoding: it still decodes cleanly to the end
    let playlist = dir.join(d.sessions[0].output_filename.clone().unwrap());
    let out = std::process::Command::new(sf_recording::ffmpeg::ffmpeg_path().unwrap()).args(["-v", "error", "-i"]).arg(&playlist).args(["-f", "null", "-"]).output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
}
