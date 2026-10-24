//! ONNX Runtime's prebuilt library checks the macOS version at run time
//! (`__isPlatformVersionAtLeast`), which lives in clang's runtime library; rustc links without
//! it, so it is added here.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "--print-runtime-dir"]).output();
    match out {
        Ok(o) if o.status.success() => {
            let dir = String::from_utf8_lossy(&o.stdout).trim().to_string();
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        _ => println!("cargo:warning=clang's runtime directory was not found; linking ONNX Runtime may fail"),
    }
}
