//! Preload the cursor catalog into a directory and poll the system cursor for 2 seconds.
fn main() {
    let dir = std::env::args().nth(1).expect("output dir");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = sf_recording::log::RecLog::open(&dir).unwrap();
    let mut rec = sf_recording::cursor::CursorRecorder::start(&dir, log, Box::new(|id| println!("cursor -> {id}"))).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    println!("current={} catalog={}", rec.current_id(), rec.cursors().len());
    rec.stop();
}
