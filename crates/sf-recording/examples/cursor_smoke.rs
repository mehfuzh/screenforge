//! Smoke test for the cursor rasteriser (no ImageIO involved).
fn main() {
    #[cfg(target_os = "macos")]
    {
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let (w, h, png) = sf_recording::cursor::arrow_cursor_png().expect("arrow cursor");
        std::fs::write(std::env::args().nth(1).unwrap_or_else(|| "arrow.png".into()), &png).unwrap();
        println!("OK {w}x{h} png {} bytes", png.len());

        // exercise the real recorder path that used to crash
        let dir = std::env::temp_dir().join(format!("sf-cursor-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = sf_recording::log::RecLog::open(&dir).unwrap();
        let mut rec = sf_recording::cursor::CursorRecorder::start(&dir, log, Box::new(|_| {})).expect("cursor recorder");
        std::thread::sleep(std::time::Duration::from_millis(800));
        rec.stop();
        let mut n = 0;
        for e in std::fs::read_dir(dir.join("cursors")).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "png") {
                n += 1;
            }
        }
        println!("cursor recorder wrote {n} png(s) to {}", dir.display());
    }
}

