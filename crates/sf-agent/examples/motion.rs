//! Motion sections of a video, as the agent's get_motion sees them:
//! `cargo run -p sf-agent --example motion -- <video>` (needs ffmpeg on PATH).
fn main() {
    let path = std::env::args().nth(1).expect("usage: motion <video>");
    let (w, h, fps) = (640usize, 360usize, 2.0);
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path, "-vf", &format!("fps={fps},scale={w}:{h},format=gray"), "-f", "rawvideo", "-"])
        .output()
        .expect("ffmpeg");
    let frames = out.stdout.chunks_exact(w * h).map(<[u8]>::to_vec);
    for (a, b, m) in sf_agent::motion::sections(frames, w, fps) {
        println!("{:6.1}s – {:6.1}s  {}", a / 1000.0, b / 1000.0, m.label());
    }
}
