//! Turning a finished recording directory into a `.screenstudio` package, like Screen Studio's
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
}

/// Default projects folder: `~/Screen Studio Projects` (same as Screen Studio).
pub fn default_projects_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("ScreenForge Projects")
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
    let mut pkg_path = opts.projects_folder.join(format!("{base}.{}", sf_project::PACKAGE_EXTENSION));
    let mut n = 2;
    while pkg_path.exists() {
        pkg_path = opts.projects_folder.join(format!("{base} {n}.{}", sf_project::PACKAGE_EXTENSION));
        n += 1;
    }
    for sub in [package::DATA_DIR, "QuickLook", "assets"] {
        std::fs::create_dir_all(pkg_path.join(sub))?;
    }
    let dest = pkg_path.join(package::RECORDING_DIR);
    move_dir(recording_dir, &dest).context("moving the recording into the package")?;

    // project
    let mut project = Project::new(&base);
    if let Some(cfg) = opts.default_config {
        project.config = cfg;
        project.config.crop = Default::default();
    }
    let duration = meta.duration_ms();
    let display = meta.display();
    let is_area = display.and_then(|d| d.crop_rect()).is_some();
    project.config.styles.screen_border_radius = if is_area { 16.0 } else { 0.0 };
    // start on one of this Mac's own wallpapers unless a preset picked something else
    let bg = project.config.styles.background.system_name.as_deref();
    if matches!(bg, None | Some("macOS/san-francisco-dark.jpg") | Some(DEFAULT_WALLPAPER)) {
        let name = crate::assets::system_wallpapers::default_name().unwrap_or_else(|| DEFAULT_WALLPAPER.into());
        project.config.styles.background.system_name = Some(name);
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
