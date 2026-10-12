//! The `screencapturekit` crate links a Swift bridge whose runtime dylibs are found through
//! `@rpath`. Link args emitted by a dependency's build script do not propagate to dependents,
//! so every binary that links `sf-recording` (tests, examples, and the app) needs these rpaths.
//! Apps linking this crate should carry the same two lines in their own `build.rs`.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        if let Ok(out) = std::process::Command::new("xcode-select").arg("-p").output() {
            if out.status.success() {
                let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
                println!("cargo:rustc-link-arg=-Wl,-rpath,{dev}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
            }
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
