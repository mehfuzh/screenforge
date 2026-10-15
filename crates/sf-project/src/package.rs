//! Reading and writing `.screenforge` package directories (and importing `.screenstudio` ones).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recording::RecordingMetadata;
use crate::schema::{migrate, v4::Project};
use crate::{superjson, Error, Result};

pub const PACKAGE_EXTENSION: &str = "screenforge";
pub const PRESET_EXTENSION: &str = "screenforgepreset";
/// Screen Studio's package extensions. These use the same layout as ours and are imported
/// (copied to a `.screenforge` package) rather than edited in place.
pub const SCREEN_STUDIO_EXTENSION: &str = "screenstudio";
pub const SCREEN_STUDIO_PRESET_EXTENSION: &str = "screenstudiopreset";

pub const META_FILE: &str = "meta.json";
pub const PROJECT_FILE: &str = "project.json";
pub const PROJECT_BACKUP_FILE: &str = "project.json.bak";
pub const MARKERS_FILE: &str = "recording-markers.json";
pub const DATA_DIR: &str = "data";
pub const RECORDING_DIR: &str = "recording";
pub const QUICKLOOK_PREVIEW: &str = "QuickLook/Preview.png";

/// Version string we write into `meta.json`. Screen Studio refuses to open packages whose
/// `requiredVersion` is newer than itself, so we keep the same values 4.0.4 writes.
pub const WRITER_VERSION: &str = "4.0.4-4926";
pub const REQUIRED_VERSION: &str = "4.0.0";

/// `meta.json`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMeta {
    pub version: String,
    pub required_version: String,
    pub created_at: DateTime<Utc>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl PackageMeta {
    pub fn new_now() -> Self {
        PackageMeta {
            version: WRITER_VERSION.into(),
            required_version: REQUIRED_VERSION.into(),
            created_at: Utc::now(),
            extra: Default::default(),
        }
    }
}

/// A marker the user set while recording (⌘-key / widget button); stored as source time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ProjectPackage {
    pub path: PathBuf,
    pub meta: PackageMeta,
    pub project: Project,
    pub markers: Vec<Value>,
    /// Lazily loaded; `None` when the package has no `recording/metadata.json` (e.g. presets).
    pub recording: Option<RecordingMetadata>,
    /// True when the file was migrated from an older schema on load.
    pub migrated: bool,
}

impl ProjectPackage {
    pub fn is_package(path: &Path) -> bool {
        path.is_dir() && path.join(PROJECT_FILE).is_file()
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !Self::is_package(&path) {
            return Err(Error::NotAPackage(path));
        }
        let meta: PackageMeta = read_superjson(&path.join(META_FILE)).unwrap_or_else(|_| PackageMeta::new_now());

        let project_path = path.join(PROJECT_FILE);
        let raw = std::fs::read_to_string(&project_path).map_err(|source| Error::Io { path: project_path.clone(), source })?;
        let (json, _) = superjson::to_value_with_meta(&raw).map_err(|source| Error::Json { path: project_path.clone(), source })?;
        let migrated = !migrate::detect_version(&json).starts_with("4.");
        let mut project = migrate::load_any(json)?;

        // Per-key scratch files under data/ override what's inline in project.json.
        if let Ok(rd) = std::fs::read_dir(path.join(DATA_DIR)) {
            for entry in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(entry.path()) {
                    if let Ok(v) = superjson::from_str::<Value>(&s) {
                        project.data.insert(entry.file_name().to_string_lossy().into_owned(), v);
                    }
                }
            }
        }

        let markers: Vec<Value> = read_superjson(&path.join(MARKERS_FILE)).unwrap_or_default();
        let rec_dir = path.join(RECORDING_DIR);
        let recording = if rec_dir.join(crate::recording::METADATA_FILE).exists() {
            Some(RecordingMetadata::load(&rec_dir)?)
        } else {
            None
        };

        Ok(ProjectPackage { path, meta, project, markers, recording, migrated })
    }

    /// Create a new package directory on disk with an empty project.
    pub fn create(path: impl AsRef<Path>, name: &str) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        for sub in [DATA_DIR, RECORDING_DIR, "QuickLook"] {
            let p = path.join(sub);
            std::fs::create_dir_all(&p).map_err(|source| Error::Io { path: p, source })?;
        }
        let pkg = ProjectPackage {
            path,
            meta: PackageMeta::new_now(),
            project: Project::new(name),
            markers: vec![],
            recording: None,
            migrated: false,
        };
        pkg.save()?;
        Ok(pkg)
    }

    pub fn recording_dir(&self) -> PathBuf {
        self.path.join(RECORDING_DIR)
    }

    /// Write `project.json` (keeping the previous one as `project.json.bak`), `meta.json`,
    /// `recording-markers.json` and the `data/` files, exactly like Screen Studio.
    pub fn save(&self) -> Result<()> {
        let project_path = self.path.join(PROJECT_FILE);
        if project_path.exists() {
            let bak = self.path.join(PROJECT_BACKUP_FILE);
            std::fs::copy(&project_path, &bak).map_err(|source| Error::Io { path: bak, source })?;
        }
        let mut project = self.project.clone();
        project.last_saved_at = Some(Utc::now());
        write_atomic(&project_path, &superjson::to_string_pretty(&project, Project::DATE_PATHS)?)?;
        write_atomic(&self.path.join(META_FILE), &superjson::to_string_pretty(&self.meta, &["createdAt"])?)?;
        write_atomic(&self.path.join(MARKERS_FILE), &superjson::to_string_pretty(&self.markers, &[])?)?;
        let data_dir = self.path.join(DATA_DIR);
        std::fs::create_dir_all(&data_dir).map_err(|source| Error::Io { path: data_dir.clone(), source })?;
        for (k, v) in &project.data {
            write_atomic(&data_dir.join(k), &superjson::to_string_pretty(v, &[])?)?;
        }
        Ok(())
    }
}

/// True for a path with Screen Studio's project extension.
pub fn is_screen_studio(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == SCREEN_STUDIO_EXTENSION)
}

/// `folder/<base>.screenforge`, or `folder/<base> 2.screenforge`, … if that is taken.
pub fn unique_package_path(folder: &Path, base: &str) -> PathBuf {
    let mut path = folder.join(format!("{base}.{PACKAGE_EXTENSION}"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{base} {n}.{PACKAGE_EXTENSION}"));
        n += 1;
    }
    path
}

/// Byte progress of an import, shared with the UI; set `cancel` to stop it.
#[derive(Debug, Default)]
pub struct ImportProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

/// Copy a `.screenstudio` package into `folder` as a new `.screenforge` package and return its
/// path. The original is left untouched.
pub fn import_screen_studio(src: &Path, folder: &Path) -> Result<PathBuf> {
    import_screen_studio_with(src, folder, &ImportProgress::default())
}

/// [`import_screen_studio`] reporting bytes copied to `progress`; on cancel nothing is left behind
/// and [`Error::Cancelled`] is returned.
pub fn import_screen_studio_with(src: &Path, folder: &Path, progress: &ImportProgress) -> Result<PathBuf> {
    if !ProjectPackage::is_package(src) {
        return Err(Error::NotAPackage(src.into()));
    }
    std::fs::create_dir_all(folder).map_err(|source| Error::Io { path: folder.into(), source })?;
    progress.total.store(dir_size(src), Ordering::Relaxed);
    let base = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into());
    let tmp = folder.join(format!(".import-{}", crate::ids::new_id()));
    let clone = same_volume(src, folder);
    let dest = copy_dir(src, &tmp, clone, progress).and_then(|_| {
        let dest = unique_package_path(folder, &base);
        std::fs::rename(&tmp, &dest).map_err(|source| Error::Io { path: dest.clone(), source })?;
        Ok(dest)
    });
    if dest.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    dest
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).map(|p| if p.is_dir() { dir_size(&p) } else { p.metadata().map(|m| m.len()).unwrap_or(0) }).sum())
        .unwrap_or(0)
}

/// On one APFS volume `std::fs::copy` clones files: instant and free of extra disk space.
#[cfg(target_os = "macos")]
fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    matches!((a.metadata(), b.metadata()), (Ok(x), Ok(y)) if x.dev() == y.dev())
}

#[cfg(not(target_os = "macos"))]
fn same_volume(_: &Path, _: &Path) -> bool {
    false
}

fn copy_dir(from: &Path, to: &Path, clone: bool, progress: &ImportProgress) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|source| Error::Io { path: to.into(), source })?;
    for entry in std::fs::read_dir(from).map_err(|source| Error::Io { path: from.into(), source })? {
        let entry = entry.map_err(|source| Error::Io { path: from.into(), source })?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if src.is_dir() {
            copy_dir(&src, &dst, clone, progress)?;
        } else if clone {
            let n = std::fs::copy(&src, &dst).map_err(|source| Error::Io { path: dst.clone(), source })?;
            progress.done.fetch_add(n, Ordering::Relaxed);
        } else {
            copy_file(&src, &dst, progress)?;
        }
    }
    Ok(())
}

/// Copy in chunks so a single large recording still shows progress and can be cancelled.
fn copy_file(src: &Path, dst: &Path, progress: &ImportProgress) -> Result<()> {
    use std::io::{Read, Write};
    fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
        move |source| Error::Io { path: path.into(), source }
    }
    let mut r = std::fs::File::open(src).map_err(io(src))?;
    let mut w = std::fs::File::create(dst).map_err(io(dst))?;
    let mut buf = vec![0u8; 4 << 20];
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = r.read(&mut buf).map_err(io(src))?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(io(dst))?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    if let Ok(m) = src.metadata() {
        let _ = std::fs::set_permissions(dst, m.permissions());
    }
    Ok(())
}

fn read_superjson<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let s = std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.into(), source })?;
    superjson::from_str(&s).map_err(|source| Error::Json { path: path.into(), source })
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|source| Error::Io { path: tmp.clone(), source })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::Io { path: path.into(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(v: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects").join(v)
    }

    #[test]
    fn opens_v4_fixture_losslessly() {
        let pkg = ProjectPackage::open(fixture("v4")).unwrap();
        assert!(!pkg.migrated);
        let p = &pkg.project;
        assert_eq!(p.id, "VTxKFiwiin");
        assert_eq!(p.scenes[0].slices.len(), 3);
        assert_eq!(p.scenes[0].slices[1].time_scale, 0.5);
        assert_eq!(p.config.styles.background.system_name.as_deref(), Some("macOS/san-francisco-dark.jpg"));

        // Round trip: re-serialize and compare to the original JSON value.
        let raw = std::fs::read_to_string(fixture("v4").join(PROJECT_FILE)).unwrap();
        let (orig, _) = superjson::to_value_with_meta(&raw).unwrap();
        let mut ours = serde_json::to_value(p).unwrap();
        superjson::normalize_numbers(&mut ours);
        if ours != orig {
            let out = std::env::var("SF_SCRATCH").unwrap_or_else(|_| "/tmp".into());
            std::fs::write(format!("{out}/rt-orig.json"), serde_json::to_string_pretty(&orig).unwrap()).unwrap();
            std::fs::write(format!("{out}/rt-ours.json"), serde_json::to_string_pretty(&ours).unwrap()).unwrap();
        }
        assert_eq!(ours, orig, "v4 project must round-trip byte-for-byte at the value level");
    }

    #[test]
    fn migrates_v3_fixture() {
        let pkg = ProjectPackage::open(fixture("v3")).unwrap();
        assert!(pkg.migrated);
        let p = &pkg.project;
        assert_eq!(p.version, "4.0.0");
        assert_eq!(p.config.styles.background.kind, "system");
        assert_eq!(p.config.styles.background.system_name.as_deref(), Some("macOS/tahoe-light.jpg"));
        assert_eq!(p.config.cursor.set.id, "macos-tahoe");
        assert_eq!(p.config.animations.mouse_movement_spring.unwrap().stiffness, 470.0);
        assert_eq!(p.config.output.padding_ratio01, 0.0);
        assert_eq!(p.config.cursor.size, 24.0);
        assert_eq!(p.scenes[0].slices.len(), 7);
        assert_eq!(p.config.default_layout.kind, "camera-overlay");
        assert!(p.data.contains_key("recordingFlags"));
    }

    #[test]
    fn create_and_reopen() {
        let dir = std::env::temp_dir().join(format!("sf-test-{}.{PACKAGE_EXTENSION}", crate::ids::new_id()));
        let pkg = ProjectPackage::create(&dir, "Hello").unwrap();
        let again = ProjectPackage::open(&dir).unwrap();
        assert_eq!(again.project.id, pkg.project.id);
        assert!(dir.join(META_FILE).exists());
        again.save().unwrap();
        assert!(dir.join(PROJECT_BACKUP_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn imports_screen_studio_package_as_copy() {
        let root = std::env::temp_dir().join(format!("sf-import-{}", crate::ids::new_id()));
        let src = root.join("Demo.screenstudio");
        copy_dir(&fixture("v4"), &src, false, &ImportProgress::default()).unwrap();
        let folder = root.join("projects");

        let a = import_screen_studio(&src, &folder).unwrap();
        assert_eq!(a, folder.join("Demo.screenforge"));
        let b = import_screen_studio(&src, &folder).unwrap();
        assert_eq!(b, folder.join("Demo 2.screenforge"));

        let pkg = ProjectPackage::open(&a).unwrap();
        assert_eq!(pkg.project.id, "VTxKFiwiin");
        pkg.save().unwrap();
        assert!(!src.join(PROJECT_BACKUP_FILE).exists(), "the original must not be written to");
        assert!(is_screen_studio(&src) && !is_screen_studio(&a));
        assert!(import_screen_studio(&root.join("missing.screenstudio"), &folder).is_err());

        // chunked copy reports every byte; a cancelled import leaves nothing behind
        let progress = ImportProgress::default();
        let c = import_screen_studio_with(&src, &root.join("chunked"), &progress).unwrap();
        assert!(ProjectPackage::open(&c).is_ok());
        assert_eq!(progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
        let cancelled = ImportProgress::default();
        cancelled.cancel.store(true, Ordering::Relaxed);
        let empty = root.join("cancelled");
        assert!(matches!(import_screen_studio_with(&src, &empty, &cancelled), Err(Error::Cancelled)));
        assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
