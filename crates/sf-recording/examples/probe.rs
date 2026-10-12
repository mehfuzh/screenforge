//! Print permissions and discovered devices (`cargo run -p sf-recording --example probe`).
fn main() {
    println!(
        "screen={:?} input={:?} mic={:?}",
        sf_recording::permissions::screen_recording(),
        sf_recording::permissions::input_monitoring(),
        sf_recording::permissions::microphone()
    );
    let d = sf_recording::discover();
    for disp in &d.displays {
        println!("display {disp:?}");
    }
    for w in &d.windows {
        println!("window {} [{}] {:?} {:?}", w.id, w.app_name, w.title, w.bounds);
    }
    for m in &d.microphones {
        println!("mic {} id={} default={}", m.name, m.id, m.is_default);
    }
}
