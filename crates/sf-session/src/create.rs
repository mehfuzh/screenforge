//! Turning a finished recording directory into a `.screenforge` package, like Screen Studio's
//! post-recording step: move the recording into `<pkg>/recording`, write `project.json` with
//! one full-length slice (and automatic zooms), `meta.json` and `recording-markers.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sf_engine::cursor::CursorTrack;
use sf_project::recording::{InputLog, RecordingMetadata};
use sf_project::v4::{Config, Project, Slice, Zoom};
use sf_project::{package, ProjectPackage};

/// Bundled wallpaper used for new recordings (`assets/backgrounds/…`).
pub const DEFAULT_WALLPAPER: &str = "Neon/dracula.jpg";

pub struct CreateOptions {
    pub name: String,
    pub projects_folder: PathBuf,
    pub default_config: Option<Config>,
    pub create_automatic_zooms: bool,
    /// Source-time ms of markers added while recording.
    pub markers_ms: Vec<f64>,
    /// An iPhone or iPad screen recording: start in a device frame (a phone for tall or wide
    /// phone-shaped recordings, else a tablet), on the wallpaper with room around it.
    pub device_mockup: bool,
}

/// Default projects folder: `~/ScreenForge Projects`.
pub fn default_projects_folder() -> PathBuf {
    let home = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()).or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."));
    home.join("ScreenForge Projects")
}

/// The `.screenforge` package to open for `path`. A `.screenstudio` package is imported first:
/// one in our projects folder was written by an older ScreenForge and is renamed in place; any
/// other (Screen Studio's own) is copied into the projects folder, leaving the original untouched;
/// `progress` follows that copy.
pub fn package_to_open(path: &Path, progress: &package::ImportProgress) -> Result<PathBuf> {
    if !package::is_screen_studio(path) {
        return Ok(path.to_path_buf());
    }
    let folder = default_projects_folder();
    if path.parent() == Some(folder.as_path()) && ProjectPackage::is_package(path) {
        let base = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into());
        let dest = package::unique_package_path(&folder, &base);
        std::fs::rename(path, &dest).with_context(|| format!("renaming {} to .{}", path.display(), sf_project::PACKAGE_EXTENSION))?;
        return Ok(dest);
    }
    package::import_screen_studio_with(path, &folder, progress).with_context(|| format!("importing {}", path.display()))
}

/// Sanitise a project file name like Screen Studio (`?/\*"'` ` .<>|` stripped, max 100 chars).
pub fn sanitize_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !"?/\\*\"'`.<>|".contains(*c)).collect();
    let s = s.trim().to_string();
    let s = if s.is_empty() { "Recording".to_string() } else { s };
    s.chars().take(100).collect()
}

/// Create the package and return it opened.
pub fn create_project_from_recording(recording_dir: &Path, opts: CreateOptions) -> Result<ProjectPackage> {
    let meta = RecordingMetadata::load(recording_dir).context("reading recording metadata")?;
    std::fs::create_dir_all(&opts.projects_folder).with_context(|| format!("creating {}", opts.projects_folder.display()))?;
    let base = sanitize_name(&opts.name);
    let pkg_path = package::unique_package_path(&opts.projects_folder, &base);
    for sub in [package::DATA_DIR, "QuickLook", "assets"] {
        std::fs::create_dir_all(pkg_path.join(sub))?;
    }
    let dest = pkg_path.join(package::RECORDING_DIR);
    move_dir(recording_dir, &dest).context("moving the recording into the package")?;

    // project
    let mut project = Project::new(&base);
    let has_preset = opts.default_config.is_some();
    if let Some(cfg) = opts.default_config {
        project.config = cfg;
        project.config.crop = Default::default();
    }
    let duration = meta.duration_ms();
    let display = meta.display();
    let is_area = display.and_then(|d| d.crop_rect()).is_some();
    project.config.styles.screen_border_radius = if is_area { 16.0 } else { 0.0 };
    // a full display fills the video edge to edge; area/window recordings sit on the wallpaper
    if !is_area && !has_preset {
        project.config.output.padding_ratio01 = 0.0;
    }
    // start on one of this Mac's own wallpapers unless a preset picked something else
    let bg = project.config.styles.background.system_name.as_deref();
    if matches!(bg, None | Some("macOS/san-francisco-dark.jpg") | Some(DEFAULT_WALLPAPER)) {
        let name = crate::assets::system_wallpapers::default_name().unwrap_or_else(|| DEFAULT_WALLPAPER.into());
        project.config.styles.background.system_name = Some(name);
    }
    if opts.device_mockup {
        if let Some(b) = display.and_then(|d| d.sessions.first()).and_then(|s| s.bounds).or_else(|| display.and_then(|d| d.crop_rect())) {
            let long = b.width.max(b.height) / b.width.min(b.height).max(1.0);
            let kind = if long >= 1.6 { sf_engine::device::Kind::Phone } else { sf_engine::device::Kind::Tablet };
            project.config.device.enable_mockup = true;
            project.config.device.frame_key = Some(sf_engine::device::Frame { kind, finish: sf_engine::device::Finish::Black }.key());
        }
        project.config.output.padding_ratio01 = 0.08;
        project.config.styles.screen_border_radius = 0.0;
    }
    let scene = &mut project.scenes[0];
    scene.slices = vec![Slice::new(0.0, duration)];
    if opts.create_automatic_zooms {
        if let (Some(input), Some(disp)) = (meta.input(), display) {
            if let (Some(s), Some(ds)) = (input.sessions.first(), disp.sessions.first()) {
                if let Ok(log) = InputLog::load(&dest, s) {
                    let bounds = ds.bounds.or_else(|| disp.crop_rect()).unwrap_or(sf_project::schema::common::Rect::UNIT);
                    let track = CursorTrack::build(&log.mouse_moves, &log.mouse_clicks, &log.key_strokes, bounds, s.process_time_start_ms, project.config.cursor.remove_shake_treshold);
                    scene.zooms = sf_engine::autozoom::generate_with(&track, duration, Zoom::DEFAULT_LEVEL, project.config.zooms.zoom_on_typing());
                }
            }
        }
    }
    let markers: Vec<serde_json::Value> = opts
        .markers_ms
        .iter()
        .map(|ms| {
            let unix = meta.sessions.first().map(|s| s.unix_start_ms).unwrap_or(0.0) + ms;
            serde_json::json!({ "id": sf_project::ids::new_id(), "date": chrono_iso(unix) })
        })
        .collect();
    let pkg = ProjectPackage { path: pkg_path, meta: package::PackageMeta::new_now(), project, markers, recording: Some(meta), migrated: false };
    pkg.save()?;
    Ok(pkg)
}

fn chrono_iso(unix_ms: f64) -> String {
    let secs = (unix_ms / 1000.0).floor() as i64;
    let nanos = ((unix_ms / 1000.0 - secs as f64) * 1e9) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).unwrap_or_default()
}

fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // cross-device: copy then remove
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            move_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    std::fs::remove_dir_all(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a project from the fixture recording, optionally without its crop rect (a full display).
    fn create(full_display: bool) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path();
        let rec = tmp.join("recording");
        std::fs::create_dir_all(&rec).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recording");
        for e in std::fs::read_dir(&fixture).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), rec.join(e.file_name())).unwrap();
        }
        if full_display {
            let path = rec.join(sf_project::recording::METADATA_FILE);
            let mut meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for r in meta["recorders"].as_array_mut().unwrap() {
                if let Some(cfg) = r["configuration"].as_object_mut() {
                    cfg.remove("cropRect");
                }
            }
            std::fs::write(&path, meta.to_string()).unwrap();
        }
        let opts = CreateOptions { name: "Test".into(), projects_folder: tmp.join("projects"), default_config: None, create_automatic_zooms: false, markers_ms: Vec::new(), device_mockup: false };
        create_project_from_recording(&rec, opts).unwrap().project.config
    }

    #[test]
    fn full_display_fills_the_video() {
        let cfg = create(true);
        assert_eq!(cfg.output.padding_ratio01, 0.0);
        assert_eq!(cfg.styles.screen_border_radius, 0.0);
    }

    #[test]
    fn area_keeps_the_wallpaper_padding() {
        let cfg = create(false);
        assert_eq!(cfg.output.padding_ratio01, 0.1);
        assert_eq!(cfg.styles.screen_border_radius, 16.0);
    }
}
