//! `.screenforgepreset` packages: a saved project configuration (`.screenstudiopreset` ones are read too).
//!
//! ```text
//! Name.screenforgepreset/
//!   meta.json   superjson {version:"4.0.0", app:{version}, projectConfig: Config (minus crop), name?}
//!   assets/     copy of the project's assets/ (background image, LUT …)
//! ```

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::package::WRITER_VERSION;
use crate::schema::v4::Config;
use crate::{superjson, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetMeta {
    pub version: String,
    pub app: AppInfo,
    pub project_config: Config,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct Preset {
    pub path: PathBuf,
    pub meta: PresetMeta,
}

impl Preset {
    /// Preset id/display name = folder basename without extension.
    pub fn name(&self) -> String {
        self.meta.name.clone().unwrap_or_else(|| self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let meta_path = path.join("meta.json");
        let raw = std::fs::read_to_string(&meta_path).map_err(|source| Error::Io { path: meta_path.clone(), source })?;
        let (mut json, _) = superjson::to_value_with_meta(&raw).map_err(|source| Error::Json { path: meta_path.clone(), source })?;
        // v3 presets: no `app` key, flat config
        if json.get("app").is_none() {
            let cfg = json.get("projectConfig").cloned().unwrap_or(Value::Object(Map::new()));
            let cfg_v4: Config = if cfg.get("audio").is_some() {
                serde_json::from_value(cfg).unwrap_or_default()
            } else {
                let fake = serde_json::json!({"id":"x","name":"x","createdAt":"2020-01-01T00:00:00.000Z","updatedAt":"2020-01-01T00:00:00.000Z","lastSavedAt":"2020-01-01T00:00:00.000Z","config":cfg,"scenes":[]});
                crate::schema::migrate::load_any(fake).map(|p| p.config).unwrap_or_default()
            };
            let version = json.get("version").and_then(Value::as_str).unwrap_or("unknown").to_string();
            json = serde_json::json!({"name": json.get("name").cloned().unwrap_or(Value::Null), "version": "4.0.0", "app": {"version": version}, "projectConfig": cfg_v4});
        }
        let mut meta: PresetMeta = serde_json::from_value(json).map_err(|source| Error::Json { path: meta_path, source })?;
        meta.project_config.crop = Default::default();
        meta.project_config.camera.crop01 = None;
        Ok(Preset { path, meta })
    }

    /// Create a preset package from a project config (crop is never part of a preset).
    pub fn create(path: impl AsRef<Path>, name: &str, config: &Config, assets_dir: Option<&Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(path.join("assets")).map_err(|source| Error::Io { path: path.clone(), source })?;
        let mut cfg = config.clone();
        cfg.crop = Default::default();
        cfg.camera.crop01 = None;
        let meta = PresetMeta { version: "4.0.0".into(), app: AppInfo { version: WRITER_VERSION.into() }, project_config: cfg, name: Some(name.into()), extra: Map::new() };
        let s = superjson::to_string_pretty(&meta, &[])?;
        std::fs::write(path.join("meta.json"), s).map_err(|source| Error::Io { path: path.clone(), source })?;
        if let Some(src) = assets_dir {
            if let Ok(rd) = std::fs::read_dir(src) {
                for e in rd.flatten() {
                    let _ = std::fs::copy(e.path(), path.join("assets").join(e.file_name()));
                }
            }
        }
        let _ = Utc::now();
        Ok(Preset { path, meta })
    }

    /// Apply to a project: config is replaced section by section; `crop` is kept.
    pub fn apply(&self, project: &mut crate::Project, project_dir: &Path) -> Result<()> {
        let crop = project.config.crop.clone();
        project.config = self.meta.project_config.clone();
        project.config.crop = crop;
        let assets = self.path.join("assets");
        if assets.is_dir() {
            let dst = project_dir.join("assets");
            std::fs::create_dir_all(&dst).map_err(|source| Error::Io { path: dst.clone(), source })?;
            for e in std::fs::read_dir(&assets).map_err(|source| Error::Io { path: assets.clone(), source })?.flatten() {
                let _ = std::fs::copy(e.path(), dst.join(e.file_name()));
            }
        }
        Ok(())
    }

    /// Package path for a preset called `name` inside `folder` (`<name>.screenforgepreset`).
    pub fn path_for(folder: &Path, name: &str) -> PathBuf {
        folder.join(format!("{}.{}", sanitize_preset_name(name), crate::PRESET_EXTENSION))
    }

    /// Whether a preset called `name` already exists in `folder` (either extension).
    pub fn exists(folder: &Path, name: &str) -> bool {
        let stem = sanitize_preset_name(name);
        Self::list(folder).iter().any(|p| p.name().eq_ignore_ascii_case(&stem) || p.path.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem)))
    }

    /// Rename: the display name in `meta.json` and the package folder (`<name>.<ext>`, the
    /// extension is kept). Fails when another preset already has that name.
    pub fn rename(&mut self, new_name: &str) -> Result<()> {
        let stem = sanitize_preset_name(new_name);
        if stem.is_empty() {
            return Err(Error::Io { path: self.path.clone(), source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty preset name") });
        }
        let ext = self.path.extension().and_then(|e| e.to_str()).unwrap_or(crate::PRESET_EXTENSION).to_string();
        let new_path = self.path.with_file_name(format!("{stem}.{ext}"));
        if new_path != self.path && new_path.exists() {
            return Err(Error::Io { path: new_path, source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "a preset with this name already exists") });
        }
        self.meta.name = Some(stem.clone());
        let text = superjson::to_string_pretty(&self.meta, &[])?;
        std::fs::write(self.path.join("meta.json"), text).map_err(|source| Error::Io { path: self.path.join("meta.json"), source })?;
        if new_path != self.path {
            std::fs::rename(&self.path, &new_path).map_err(|source| Error::Io { path: self.path.clone(), source })?;
            self.path = new_path;
        }
        Ok(())
    }

    /// Remove the preset package from disk.
    pub fn delete(self) -> Result<()> {
        std::fs::remove_dir_all(&self.path).map_err(|source| Error::Io { path: self.path.clone(), source })
    }

    /// Default presets folder (`~/Documents/Screen Studio Presets`).
    pub fn default_folder() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Documents").join("ScreenForge Presets"))
    }

    pub fn list(folder: &Path) -> Vec<Preset> {
        let mut out = vec![];
        if let Ok(rd) = std::fs::read_dir(folder) {
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|x| x.to_str());
                if ext == Some(crate::PRESET_EXTENSION) || ext == Some(crate::SCREEN_STUDIO_PRESET_EXTENSION) {
                    if let Ok(pr) = Preset::open(&p) {
                        out.push(pr);
                    }
                }
            }
        }
        out.sort_by_key(|p| p.name().to_lowercase());
        out
    }
}

/// A preset name as a file-system friendly stem: no path separators or quote characters,
/// trimmed, at most 100 characters.
pub fn sanitize_preset_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !"?/\\*\"'`<>|:".contains(*c)).collect();
    s.trim().chars().take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_and_delete() {
        let folder = std::env::temp_dir().join(format!("sf-presets-{}", crate::ids::new_id()));
        std::fs::create_dir_all(&folder).unwrap();
        let cfg = Config::default();
        let mut p = Preset::create(Preset::path_for(&folder, "First"), "First", &cfg, None).unwrap();
        assert!(Preset::exists(&folder, "first"));
        p.rename("Second: take/2").unwrap();
        assert_eq!(p.name(), "Second take2");
        assert!(p.path.ends_with(format!("Second take2.{}", crate::PRESET_EXTENSION)));
        assert!(!Preset::exists(&folder, "First") && Preset::exists(&folder, "Second take2"));
        // renaming onto an existing preset is refused
        let mut other = Preset::create(Preset::path_for(&folder, "Other"), "Other", &cfg, None).unwrap();
        assert!(other.rename("Second take2").is_err());
        assert!(other.rename("").is_err());
        let reopened = Preset::open(&p.path).unwrap();
        assert_eq!(reopened.name(), "Second take2");
        p.delete().unwrap();
        assert_eq!(Preset::list(&folder).len(), 1);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn create_open_apply() {
        let dir = std::env::temp_dir().join(format!("sf-preset-{}.{}", crate::ids::new_id(), crate::PRESET_EXTENSION));
        let mut cfg = Config::default();
        cfg.styles.screen_border_radius = 42.0;
        cfg.crop.rect01.width = 0.5;
        let p = Preset::create(&dir, "Test", &cfg, None).unwrap();
        assert_eq!(p.meta.project_config.crop.rect01.width, 1.0, "crop must not be stored");
        let again = Preset::open(&dir).unwrap();
        assert_eq!(again.meta.project_config.styles.screen_border_radius, 42.0);
        let mut project = crate::Project::new("x");
        project.config.crop.rect01.width = 0.25;
        again.apply(&mut project, &std::env::temp_dir()).unwrap();
        assert_eq!(project.config.styles.screen_border_radius, 42.0);
        assert_eq!(project.config.crop.rect01.width, 0.25);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
