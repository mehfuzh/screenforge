//! Bundled assets: wallpapers, cursor sets, device frames, click sounds.
//!
//! Resolution order:
//! 1. `$SF_ASSETS` (a directory with `backgrounds/`, `cursors/`, `devices/`, `sounds/`)
//! 2. `<exe>/../Resources/assets`, `<exe>/assets`, `<repo>/assets`
//! 3. The assets inside an installed Screen Studio (`/Applications/Screen Studio.app/…/app.asar`),
//!    extracted on demand into `~/Library/Caches/ScreenForge/assets`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

#[derive(Clone, Default)]
pub struct Assets {
    pub roots: Vec<PathBuf>,
    asar: Option<Arc<AsarAssets>>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assets").field("roots", &self.roots).field("asar", &self.asar.as_ref().map(|a| a.archive.display().to_string())).finish()
    }
}

impl Assets {
    pub fn discover() -> Self {
        let mut roots = vec![];
        if let Ok(p) = std::env::var("SF_ASSETS") {
            roots.push(PathBuf::from(p));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                roots.push(dir.join("../Resources/assets"));
                roots.push(dir.join("assets"));
            }
        }
        roots.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"));
        // the Mac's own wallpapers, converted once into a cache (read locally, never shipped)
        if let Some(dir) = system_wallpapers::cache_root() {
            system_wallpapers::sync_in_background(dir.clone());
            roots.push(dir);
        }
        let roots: Vec<PathBuf> = roots.into_iter().filter(|p| p.is_dir()).collect();
        // SF_NO_SCREEN_STUDIO=1: behave like a Mac without Screen Studio (bundled assets only)
        let asar = if std::env::var_os("SF_NO_SCREEN_STUDIO").is_some() { None } else { AsarAssets::find().map(Arc::new) };
        Assets { roots, asar }
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.asar.is_none()
    }

    /// Where the assets come from, for the UI.
    pub fn source_description(&self) -> String {
        if let Some(r) = self.roots.first() {
            return r.display().to_string();
        }
        if let Some(a) = &self.asar {
            return format!("installed app bundle ({})", a.archive.display());
        }
        "none".into()
    }

    fn find(&self, rel: &str) -> Option<PathBuf> {
        if let Some(p) = self.roots.iter().map(|r| r.join(rel)).find(|p| p.is_file()) {
            return Some(p);
        }
        self.asar.as_ref().and_then(|a| a.extract(&format!("assets/{rel}")))
    }

    /// `backgrounds/<name>` (e.g. `macOS/san-francisco-dark.jpg`).
    pub fn background(&self, name: &str) -> Option<PathBuf> {
        self.find(&format!("backgrounds/{name}"))
    }

    pub fn cursor(&self, set: &str, cursor_id: &str) -> Option<PathBuf> {
        self.find(&format!("cursors/sets/{set}/{cursor_id}.png")).or_else(|| self.find(&format!("cursors/common/{cursor_id}.png")))
    }

    pub fn device_frame(&self, key: &str) -> Option<PathBuf> {
        self.find(&format!("devices/{key}.png"))
    }

    pub fn click_sound(&self, set: &str, kind: &str) -> Option<PathBuf> {
        self.find(&format!("sounds/clicks/{set}/{kind}.wav")).or_else(|| self.find(&format!("sounds/clicks/{set}/{kind}.mp3")))
    }

    pub fn list_backgrounds(&self) -> Vec<String> {
        let mut out = vec![];
        for r in &self.roots {
            let base = r.join("backgrounds");
            if let Ok(cats) = std::fs::read_dir(&base) {
                for c in cats.flatten().filter(|e| e.path().is_dir()) {
                    if let Ok(files) = std::fs::read_dir(c.path()) {
                        for f in files.flatten() {
                            let name = format!("{}/{}", c.file_name().to_string_lossy(), f.file_name().to_string_lossy());
                            if !out.contains(&name) {
                                out.push(name);
                            }
                        }
                    }
                }
            }
        }
        if let Some(a) = &self.asar {
            for name in a.list("assets/backgrounds/") {
                if (name.ends_with(".jpg") || name.ends_with(".png")) && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
    }

    /// Bundled background audio tracks as `<category>/<Name>.mp3`.
    pub fn list_background_audio(&self) -> Vec<String> {
        let mut out = vec![];
        for r in &self.roots {
            if let Ok(cats) = std::fs::read_dir(r.join("background-audio")) {
                for c in cats.flatten().filter(|e| e.path().is_dir()) {
                    if let Ok(files) = std::fs::read_dir(c.path()) {
                        for f in files.flatten() {
                            // the same track can exist in several roots (app bundle, source tree)
                            let name = format!("{}/{}", c.file_name().to_string_lossy(), f.file_name().to_string_lossy());
                            if !out.contains(&name) {
                                out.push(name);
                            }
                        }
                    }
                }
            }
        }
        if let Some(a) = &self.asar {
            for n in a.list("assets/background-audio/") {
                if (n.ends_with(".mp3") || n.ends_with(".m4a") || n.ends_with(".wav")) && !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        out.sort();
        out
    }

    /// Resolve a `backgroundAudioFileName`: `<category>/<file>` or a bare file name searched
    /// across the bundled categories.
    pub fn background_audio(&self, name: &str) -> Option<PathBuf> {
        if let Some(p) = self.find(&format!("background-audio/{name}")) {
            return Some(p);
        }
        let want = background_audio_key(name);
        self.list_background_audio().into_iter().find(|n| background_audio_key(n) == want).and_then(|n| self.find(&format!("background-audio/{n}")))
    }
}

/// Comparable key for a background track name: no category, no extension, no bundler hash
/// suffix (Screen Studio saves `Sunny Lo‑Fi-4NWaIWSgBl.mp3` for `lo-fi/Sunny Lo‑Fi.mp3`),
/// hyphen variants unified, case-insensitive.
pub fn background_audio_key(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.rsplit_once('.').map(|(a, _)| a).unwrap_or(base);
    let stem = match stem.rsplit_once('-') {
        Some((head, tail)) if tail.len() >= 6 && tail.len() <= 12 && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && tail.chars().any(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) => head,
        _ => stem,
    };
    stem.replace(['\u{2011}', '\u{2010}', '\u{2013}'], "-").to_lowercase()
}

/// Display name of a background track: the key's casing kept, hash and extension removed.
pub fn background_audio_label(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.rsplit_once('.').map(|(a, _)| a).unwrap_or(base);
    let key = background_audio_key(name);
    let cleaned = stem.replace(['\u{2011}', '\u{2010}', '\u{2013}'], "-");
    cleaned.chars().take(key.chars().count()).collect()
}

impl Assets {

    pub fn list_device_frames(&self) -> Vec<String> {
        let mut out = vec![];
        for r in &self.roots {
            if let Ok(files) = std::fs::read_dir(r.join("devices")) {
                for f in files.flatten() {
                    if let Some(stem) = f.path().file_stem() {
                        out.push(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
        if let Some(a) = &self.asar {
            for n in a.list("assets/devices/") {
                if let Some(stem) = n.strip_suffix(".png") {
                    if !out.iter().any(|x| x == stem) {
                        out.push(stem.to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }
}

// ───────────── asar ─────────────

/// Minimal reader for Electron `app.asar` archives (JSON index + concatenated file data).
pub struct AsarAssets {
    pub archive: PathBuf,
    cache_dir: PathBuf,
    /// path inside the archive → (offset, size)
    index: BTreeMap<String, (u64, u64)>,
    extracted: Mutex<BTreeMap<String, PathBuf>>,
}

impl AsarAssets {
    pub fn find() -> Option<Self> {
        let candidates = [
            PathBuf::from("/Applications/Screen Studio.app/Contents/Resources/app.asar"),
            dirs_home().join("Applications/Screen Studio.app/Contents/Resources/app.asar"),
        ];
        let archive = candidates.into_iter().find(|p| p.is_file())?;
        let cache_dir = directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("assets")).unwrap_or_else(|| std::env::temp_dir().join("screenforge-assets"));
        let index = read_index(&archive).ok()?;
        Some(AsarAssets { archive, cache_dir, index, extracted: Mutex::new(BTreeMap::new()) })
    }

    /// Entries under `prefix` (relative names, without the prefix).
    pub fn list(&self, prefix: &str) -> Vec<String> {
        self.index.keys().filter_map(|k| k.strip_prefix(prefix).map(str::to_string)).collect()
    }

    /// Extract `path` (archive relative) into the cache and return the file path.
    pub fn extract(&self, path: &str) -> Option<PathBuf> {
        if let Some(p) = self.extracted.lock().get(path) {
            return Some(p.clone());
        }
        let &(offset, size) = self.index.get(path)?;
        let dest = self.cache_dir.join(path);
        if !dest.is_file() || dest.metadata().map(|m| m.len() != size).unwrap_or(true) {
            std::fs::create_dir_all(dest.parent()?).ok()?;
            let data = read_range(&self.archive, offset, size).ok()?;
            std::fs::write(&dest, data).ok()?;
        }
        self.extracted.lock().insert(path.to_string(), dest.clone());
        Some(dest)
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn read_index(archive: &Path) -> std::io::Result<BTreeMap<String, (u64, u64)>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(archive)?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head)?;
    let header_size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
    let json_len = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
    let mut json = vec![0u8; json_len];
    f.seek(SeekFrom::Start(16))?;
    f.read_exact(&mut json)?;
    let base = 8 + header_size;
    let v: serde_json::Value = serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut index = BTreeMap::new();
    walk(&v, String::new(), base, &mut index);
    Ok(index)
}

fn walk(node: &serde_json::Value, prefix: String, base: u64, out: &mut BTreeMap<String, (u64, u64)>) {
    let Some(files) = node.get("files").and_then(|f| f.as_object()) else { return };
    for (name, entry) in files {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if entry.get("files").is_some() {
            walk(entry, path, base, out);
        } else if entry.get("unpacked").and_then(|u| u.as_bool()) != Some(true) {
            let size = entry.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            let offset = entry.get("offset").and_then(|o| o.as_str()).and_then(|o| o.parse::<u64>().ok()).unwrap_or(0);
            out.insert(path, (base + offset, size));
        }
    }
}

fn read_range(archive: &Path, offset: u64, size: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(archive)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_installed_screen_studio_assets_if_present() {
        let Some(a) = AsarAssets::find() else {
            eprintln!("Screen Studio not installed; skipping");
            return;
        };
        let bgs = a.list("assets/backgrounds/");
        assert!(bgs.iter().any(|b| b.starts_with("macOS/")), "{bgs:?}");
        let p = a.extract("assets/backgrounds/macOS/tahoe-light.jpg").expect("extract");
        let bytes = std::fs::read(&p).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8]), "should be a JPEG");
    }
}

#[cfg(test)]
mod background_audio_tests {
    use super::*;
    #[test]
    fn screen_studio_hashed_names_match_bundled_files() {
        assert_eq!(background_audio_key("Sunny Lo\u{2011}Fi-4NWaIWSgBl.mp3"), background_audio_key("lo-fi/Sunny Lo\u{2011}Fi.mp3"));
        assert_eq!(background_audio_label("Sunny Lo\u{2011}Fi-4NWaIWSgBl.mp3"), "Sunny Lo-Fi");
        assert_eq!(background_audio_key("commercial/Smile Piano.mp3"), "smile piano");
        // a real word after a hyphen is not mistaken for a hash
        assert_eq!(background_audio_key("lo-fi/Lean-groove.mp3"), "lean-groove");
    }
}

/// The desktop's own wallpapers as a wallpaper category, read locally and never bundled or
/// redistributed:
///
/// - macOS: the system wallpapers (`/System/Library/Desktop Pictures`) and those downloaded in
///   System Settings. They are HEIC; `sips` converts each once to a JPEG under
///   `~/Library/Caches/dev.screenforge.ScreenForge/system-wallpapers/backgrounds/macOS/`.
/// - Linux: `/usr/share/backgrounds` and `~/.local/share/backgrounds`, in a category named
///   after the distribution (`NAME` in `/etc/os-release`, e.g. "Ubuntu"). JPEG and PNG are
///   symlinked into `~/.cache/screenforge/system-wallpapers/backgrounds/<category>/`; WebP is
///   converted once to JPEG.
/// - Windows: `C:\Windows\Web\Wallpaper\*` and `C:\Windows\Web\Screen`, copied into
///   `%LOCALAPPDATA%\screenforge\system-wallpapers\backgrounds\Windows\`.
///
/// The cache is then an ordinary assets root.
pub mod system_wallpapers {
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "macos")]
    pub fn category() -> Option<String> {
        Some("macOS".into())
    }

    #[cfg(target_os = "linux")]
    pub fn category() -> Option<String> {
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        Some(NAME.get_or_init(|| distro_name(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())).clone())
    }

    #[cfg(target_os = "windows")]
    pub fn category() -> Option<String> {
        Some("Windows".into())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    pub fn category() -> Option<String> {
        None
    }

    /// `NAME` from os-release text, usable as a folder name ("Linux" when missing).
    pub fn distro_name(os_release: &str) -> String {
        let name = os_release
            .lines()
            .find_map(|l| l.strip_prefix("NAME="))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').trim().replace('/', "-"))
            .filter(|v| !v.is_empty() && !v.starts_with('.'));
        name.unwrap_or_else(|| "Linux".into())
    }

    pub fn cache_root() -> Option<PathBuf> {
        let category = category()?;
        let root = if cfg!(target_os = "macos") {
            PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/dev.screenforge.ScreenForge/system-wallpapers")
        } else if cfg!(target_os = "windows") {
            PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("screenforge/system-wallpapers")
        } else {
            let cache = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
            cache.join("screenforge/system-wallpapers")
        };
        std::fs::create_dir_all(root.join("backgrounds").join(category)).ok()?;
        Some(root)
    }

    fn source_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            let mut dirs = vec![PathBuf::from("/System/Library/Desktop Pictures")];
            dirs.extend(home.map(|h| h.join("Library/Application Support/com.apple.mobileAssetDesktop")));
            dirs
        } else if cfg!(target_os = "windows") {
            let windir = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            let wallpaper = windir.join("Web").join("Wallpaper");
            // one folder per theme (Windows, Spotlight, ThemeA…)
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(&wallpaper).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
            dirs.sort();
            dirs.insert(0, wallpaper);
            dirs.push(windir.join("Web").join("Screen"));
            dirs
        } else {
            let mut dirs = vec![PathBuf::from("/usr/share/backgrounds")];
            dirs.extend(home.map(|h| h.join(".local/share/backgrounds")));
            dirs
        }
    }

    fn is_image(p: &Path) -> bool {
        let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        let known: &[&str] = if cfg!(target_os = "macos") { &["heic", "jpg", "jpeg", "png"] } else { &["jpg", "jpeg", "png", "webp"] };
        ext.is_some_and(|e| known.contains(&e.as_str()))
    }

    /// Wallpaper files, each once (symlinked aliases such as Ubuntu's
    /// `ubuntu-default-greyscale-wallpaper.png` resolve to the file they point at).
    fn sources() -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = vec![];
        for d in source_dirs() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if !p.is_file() || !is_image(&p) {
                        continue;
                    }
                    let real = std::fs::canonicalize(&p).unwrap_or(p);
                    if is_image(&real) && !out.contains(&real) {
                        out.push(real);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Name of `src` in the cache: JPEG and PNG keep theirs, others become JPEG.
    fn cached_name(src: &Path) -> Option<String> {
        let name = src.file_name()?.to_str()?;
        let ext = src.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        if cfg!(target_os = "linux") && matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png")) {
            return Some(name.to_string());
        }
        Some(format!("{}.jpg", src.file_stem()?.to_str()?))
    }

    /// The wallpaper new recordings start with, as `<category>/<name>`, or `None` until one
    /// is in the cache. macOS: the newest landscape on this Mac. Linux: the current desktop
    /// wallpaper (the dark style's one when the desktop is dark).
    pub fn default_name() -> Option<String> {
        let category = category()?;
        let dir = cache_root()?.join("backgrounds").join(&category);
        let mut names: Vec<String> = std::fs::read_dir(&dir).ok()?.flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        if cfg!(target_os = "macos") {
            names.retain(|n| n.ends_with(".jpg"));
            const PREFERRED: [&str; 7] = ["Tahoe", "Sequoia", "Sonoma", "Ventura", "Monterey", "Big Sur", "Catalina"];
            let pick = PREFERRED.iter().find_map(|p| names.iter().find(|n| n.starts_with(p))).or(names.first())?;
            return Some(format!("{category}/{pick}"));
        }
        if cfg!(target_os = "windows") {
            // img0 is the default wallpaper of every Windows release
            let pick = names.iter().find(|n| n.starts_with("img0")).or(names.first())?;
            return Some(format!("{category}/{pick}"));
        }
        let current = current_desktop_wallpaper().and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))?;
        let want = cached_name(&current)?;
        names.iter().find(|n| **n == want).map(|n| format!("{category}/{n}"))
    }

    /// GNOME's current wallpaper file (`org.gnome.desktop.background`).
    fn current_desktop_wallpaper() -> Option<PathBuf> {
        let get = |key: &str| -> Option<String> {
            let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.background", key]).output().ok()?;
            let v = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
            (out.status.success() && !v.is_empty()).then_some(v)
        };
        let dark = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("prefer-dark"));
        let uri = if dark { get("picture-uri-dark").or_else(|| get("picture-uri")) } else { get("picture-uri") }?;
        file_uri_path(&uri)
    }

    /// The path of a `file://` URI (percent-decoded), or a plain path as is.
    pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
        let raw = uri.strip_prefix("file://").unwrap_or(uri);
        if !raw.starts_with('/') {
            return None;
        }
        let bytes = raw.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(b) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        Some(PathBuf::from(String::from_utf8(out).ok()?))
    }

    /// Convert any wallpapers not converted yet (background thread; cheap when up to date).
    pub fn sync_in_background(root: PathBuf) {
        std::thread::Builder::new()
            .name("system-wallpapers".into())
            .spawn(move || sync(&root))
            .ok();
    }

    pub fn sync(root: &Path) {
        let Some(category) = category() else { return };
        let dest_dir = root.join("backgrounds").join(category);
        // drop links to wallpapers that were uninstalled
        if let Ok(rd) = std::fs::read_dir(&dest_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_symlink() && !p.exists() {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        for src in sources() {
            let Some(name) = cached_name(&src) else { continue };
            let dest = dest_dir.join(&name);
            if dest.is_file() {
                continue;
            }
            let tmp = dest_dir.join(format!(".{name}.tmp.jpg"));
            let ok = convert(&src, &dest, &tmp);
            if ok {
                let _ = std::fs::rename(&tmp, &dest);
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    /// Put `src` into the cache via `tmp` (renamed to `dest` on success).
    #[cfg(target_os = "macos")]
    fn convert(src: &Path, _dest: &Path, tmp: &Path) -> bool {
        std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "jpeg", "-s", "formatOptions", "85", "-Z", "2880"])
            .arg(src)
            .arg("--out")
            .arg(tmp)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Longest side kept in the cache (macOS resizes with `sips -Z 2880`; Windows wallpapers
    /// are 4K and would otherwise cost ~35 MB of texture memory each in the picker).
    #[cfg(not(target_os = "macos"))]
    const MAX_SIDE: u32 = 2880;

    #[cfg(not(target_os = "macos"))]
    fn convert(src: &Path, dest: &Path, tmp: &Path) -> bool {
        if cfg!(target_os = "windows") {
            let Ok(img) = image::open(src) else { return false };
            let img = if img.width().max(img.height()) > MAX_SIDE { img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle) } else { img };
            let Ok(file) = std::fs::File::create(tmp) else { return false };
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 88);
            return enc.encode_image(&img.to_rgb8()).is_ok();
        }
        if dest.extension() == src.extension() {
            #[cfg(unix)]
            return std::os::unix::fs::symlink(src, tmp).is_ok();
            #[cfg(not(unix))]
            return std::fs::copy(src, tmp).is_ok();
        }
        let Ok(img) = image::open(src) else { return false };
        let Ok(file) = std::fs::File::create(tmp) else { return false };
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 90);
        enc.encode_image(&img.to_rgb8()).is_ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Fills the real cache on this machine (`cargo test -p sf-session sync_system -- --ignored`).
        #[test]
        #[ignore = "writes the wallpaper cache of this machine"]
        fn sync_system_wallpapers_here() {
            let root = cache_root().expect("a cache root");
            sync(&root);
            assert!(default_name().is_some(), "no wallpaper found in {}", root.display());
        }

        #[test]
        fn distro_name_from_os_release() {
            assert_eq!(distro_name("PRETTY_NAME=\"Ubuntu 26.04 LTS\"\nNAME=\"Ubuntu\"\nID=ubuntu\n"), "Ubuntu");
            assert_eq!(distro_name("NAME=Fedora Linux\n"), "Fedora Linux");
            assert_eq!(distro_name("ID=arch\n"), "Linux");
        }

        #[test]
        fn file_uris() {
            assert_eq!(file_uri_path("file:///usr/share/backgrounds/warty-final-ubuntu.png"), Some(PathBuf::from("/usr/share/backgrounds/warty-final-ubuntu.png")));
            assert_eq!(file_uri_path("file:///home/me/My%20Pictures/a.jpg"), Some(PathBuf::from("/home/me/My Pictures/a.jpg")));
            assert_eq!(file_uri_path("/tmp/x.png"), Some(PathBuf::from("/tmp/x.png")));
            assert_eq!(file_uri_path("https://example.com/a.png"), None);
        }

        #[test]
        fn cached_names() {
            if cfg!(target_os = "linux") {
                assert_eq!(cached_name(Path::new("/b/warty.png")).as_deref(), Some("warty.png"));
                assert_eq!(cached_name(Path::new("/b/Snowy.webp")).as_deref(), Some("Snowy.jpg"));
            }
        }
    }
}
