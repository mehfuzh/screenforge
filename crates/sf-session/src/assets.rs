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
                            out.push(format!("{}/{}", c.file_name().to_string_lossy(), f.file_name().to_string_lossy()));
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

/// macOS wallpapers that ship with the system (`/System/Library/Desktop Pictures`) or that the
/// user downloaded in System Settings. They are HEIC; `sips` converts each once to a JPEG under
/// `~/Library/Caches/dev.screenforge.ScreenForge/system-wallpapers/backgrounds/macOS/`, which
/// is then an ordinary assets root. Nothing is bundled or redistributed.
pub mod system_wallpapers {
    use std::path::{Path, PathBuf};

    const CATEGORY: &str = "macOS";

    pub fn cache_root() -> Option<PathBuf> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let home = std::env::var_os("HOME")?;
        let root = PathBuf::from(home).join("Library/Caches/dev.screenforge.ScreenForge/system-wallpapers");
        std::fs::create_dir_all(root.join("backgrounds").join(CATEGORY)).ok()?;
        Some(root)
    }

    fn sources() -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("/System/Library/Desktop Pictures")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Application Support/com.apple.mobileAssetDesktop"));
        }
        let mut out = vec![];
        for d in dirs {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
                    if matches!(ext.as_deref(), Some("heic" | "jpg" | "jpeg" | "png")) {
                        out.push(p);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// The wallpaper new recordings start with: the newest macOS landscape on this Mac (as
    /// `macOS/<name>.jpg`), or any converted macOS wallpaper. `None` until one is converted.
    pub fn default_name() -> Option<String> {
        let dir = cache_root()?.join("backgrounds").join(CATEGORY);
        let mut names: Vec<String> = std::fs::read_dir(&dir).ok()?.flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".jpg") && !n.starts_with('.'))
            .collect();
        names.sort();
        const PREFERRED: [&str; 7] = ["Tahoe", "Sequoia", "Sonoma", "Ventura", "Monterey", "Big Sur", "Catalina"];
        let pick = PREFERRED.iter().find_map(|p| names.iter().find(|n| n.starts_with(p))).or(names.first())?;
        Some(format!("{CATEGORY}/{pick}"))
    }

    /// Convert any wallpapers not converted yet (background thread; cheap when up to date).
    pub fn sync_in_background(root: PathBuf) {
        std::thread::Builder::new()
            .name("system-wallpapers".into())
            .spawn(move || sync(&root))
            .ok();
    }

    pub fn sync(root: &Path) {
        let dest_dir = root.join("backgrounds").join(CATEGORY);
        for src in sources() {
            let Some(stem) = src.file_stem().and_then(|s| s.to_str()) else { continue };
            let dest = dest_dir.join(format!("{stem}.jpg"));
            if dest.is_file() {
                continue;
            }
            let tmp = dest_dir.join(format!(".{stem}.tmp.jpg"));
            let ok = std::process::Command::new("/usr/bin/sips")
                .args(["-s", "format", "jpeg", "-s", "formatOptions", "85", "-Z", "2880"])
                .arg(&src)
                .arg("--out")
                .arg(&tmp)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                let _ = std::fs::rename(&tmp, &dest);
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}
