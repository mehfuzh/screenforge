//! The CLI links `sf-recording` (for `sf join`), whose `screencapturekit` dependency links a
//! Swift bridge found through `@rpath`. Link args from a dependency's build script do not
//! propagate to dependents, so this binary (and its tests) carry the rpaths themselves, like
//! the app (`apps/studio/build.rs`).
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
