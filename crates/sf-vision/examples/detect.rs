//! `cargo run -p sf-vision --example detect -- a.png b.jpg …`: print the face found in each image.
fn main() -> anyhow::Result<()> {
    let mut det = sf_vision::FaceDetector::new()?;
    for path in std::env::args().skip(1) {
        let img = image::open(&path)?.to_rgba8();
        let (w, h) = img.dimensions();
        let t = std::time::Instant::now();
        let face = det.detect(img.as_raw(), w, h, false);
        println!("{path}: {w}×{h} → {face:?} in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(())
}
