//! List the iPhones and iPads ScreenForge can record (connected by cable, unlocked, trusted):
//! `cargo run -p sf-recording --example devices`
fn main() {
    let found = sf_recording::device::devices();
    if found.is_empty() {
        println!("No iPhone or iPad found. Connect one with a cable, unlock it and tap Trust.");
    }
    for d in found {
        println!("{}  ({})", d.name, d.id);
    }
}
