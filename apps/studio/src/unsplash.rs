//! Background → Unsplash: search photos through the ScreenForge backend's `unsplash` proxy
//! (the access key stays on the server), show thumbnails, and download the chosen photo into
//! the project's `assets/` so it renders and exports offline like any image background.
//!
//! Network work runs on background threads; the sidebar polls [`Browser::poll`] every frame.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use serde::Deserialize;

/// Largest photo we accept (bytes).
const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;
/// Referral parameters Unsplash asks for on every link back to it.
const UTM: &str = "utm_source=screenforge&utm_medium=referral";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    pub id: String,
    pub thumb: String,
    pub full: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub author_url: Option<String>,
    #[serde(default)]
    pub photo_url: Option<String>,
    pub download_location: String,
    /// What the photo shows (Unsplash's alt text), when the server sends it.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchReply {
    results: Vec<Photo>,
    #[serde(default)]
    total_pages: u32,
}

/// A downloaded photo, ready to become the background.
pub struct Applied {
    pub photo: Photo,
    /// File name inside the project's `assets/`.
    pub file_name: String,
}

#[derive(Default)]
pub struct Browser {
    pub query: String,
    pub results: Vec<Photo>,
    page: u32,
    total_pages: u32,
    pub error: Option<String>,
    /// The first page loads when the Unsplash tab is first shown.
    started: bool,
    search: Option<(u32, Receiver<Result<SearchReply, String>>)>,
    thumbs: HashMap<String, PathBuf>,
    thumb_rx: Option<Receiver<(String, PathBuf)>>,
    /// Photo id being downloaded.
    pub applying: Option<String>,
    apply_rx: Option<Receiver<Result<Applied, String>>>,
    /// Download progress of `applying` as f32 bits: 0..1, or negative while the size is unknown.
    progress: Arc<AtomicU32>,
}

impl Browser {
    pub fn is_busy(&self) -> bool {
        self.search.is_some() || self.apply_rx.is_some()
    }

    pub fn can_load_more(&self) -> bool {
        self.search.is_none() && self.page > 0 && self.page < self.total_pages
    }

    /// Load the wallpaper topic the first time the tab is shown.
    pub fn ensure_started(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            self.search(ctx, 1);
        }
    }

    /// Search for `self.query` (empty: Unsplash's wallpapers); page 1 replaces the results,
    /// later pages are appended.
    pub fn search(&mut self, ctx: &egui::Context, page: u32) {
        let (tx, rx) = channel();
        let (query, ctx) = (self.query.trim().to_string(), ctx.clone());
        std::thread::spawn(move || {
            let reply = sf_license::call_backend("unsplash", serde_json::json!({ "query": query, "page": page })).and_then(|v| serde_json::from_value::<SearchReply>(v).map_err(|e| format!("unexpected reply: {e}")));
            let _ = tx.send(reply);
            ctx.request_repaint();
        });
        self.search = Some((page, rx));
        self.error = None;
    }

    pub fn load_more(&mut self, ctx: &egui::Context) {
        if self.can_load_more() {
            self.search(ctx, self.page + 1);
        }
    }

    /// Local thumbnail of a result, once downloaded.
    pub fn thumb(&self, id: &str) -> Option<&Path> {
        self.thumbs.get(id).map(PathBuf::as_path)
    }

    /// Fraction of the photo being applied that has downloaded, `None` while its size is not
    /// known yet.
    pub fn progress(&self) -> Option<f32> {
        let v = f32::from_bits(self.progress.load(Ordering::Relaxed));
        (v >= 0.0).then_some(v.min(1.0))
    }

    /// Download `photo` into `assets_dir` (registering the download with Unsplash first).
    pub fn apply(&mut self, ctx: &egui::Context, photo: Photo, assets_dir: PathBuf) {
        if self.apply_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.applying = Some(photo.id.clone());
        self.progress.store((-1f32).to_bits(), Ordering::Relaxed);
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(download_photo(photo, &assets_dir, &progress, &ctx));
            ctx.request_repaint();
        });
        self.apply_rx = Some(rx);
        self.error = None;
    }

    /// Collect finished work; returns a photo that finished downloading.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Applied> {
        if let Some((page, rx)) = &self.search {
            if let Ok(reply) = rx.try_recv() {
                let page = *page;
                self.search = None;
                match reply {
                    Ok(r) => {
                        if page == 1 {
                            self.results.clear();
                        }
                        self.page = page;
                        self.total_pages = r.total_pages;
                        self.fetch_thumbs(ctx, &r.results);
                        self.results.extend(r.results);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(rx) = &self.thumb_rx {
            while let Ok((id, path)) = rx.try_recv() {
                self.thumbs.insert(id, path);
            }
        }
        let rx = self.apply_rx.as_ref()?;
        let done = rx.try_recv().ok()?;
        self.apply_rx = None;
        self.applying = None;
        match done {
            Ok(a) => Some(a),
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    fn fetch_thumbs(&mut self, ctx: &egui::Context, photos: &[Photo]) {
        let Some(dir) = cache_dir() else { return };
        let (tx, rx) = channel();
        let todo: Vec<(String, String)> = photos.iter().filter(|p| !self.thumbs.contains_key(&p.id)).map(|p| (p.id.clone(), p.thumb.clone())).collect();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            for (id, url) in todo {
                let path = dir.join(format!("{}.jpg", safe_id(&id)));
                if path.is_file() || fetch(&url, &path, 4 * 1024 * 1024, None).is_ok() {
                    let _ = tx.send((id, path));
                    ctx.request_repaint();
                }
            }
        });
        // replaces an older batch's channel: its thumbnails still land on disk for next time
        self.thumb_rx = Some(rx);
    }
}

/// One page of results for `query`, blocking (the agent runs on its own thread).
pub fn search_blocking(query: &str) -> Result<Vec<Photo>, String> {
    let v = sf_license::call_backend("unsplash", serde_json::json!({ "query": query.trim(), "page": 1 }))?;
    serde_json::from_value::<SearchReply>(v).map(|r| r.results).map_err(|e| format!("unexpected reply: {e}"))
}

/// Register the download with Unsplash, then fetch the image into `assets_dir`.
pub fn download_photo(photo: Photo, assets_dir: &Path, progress: &AtomicU32, ctx: &egui::Context) -> Result<Applied, String> {
    let v = sf_license::call_backend("unsplash", serde_json::json!({ "download": photo.download_location }))?;
    // the download endpoint hands back the original; ask the CDN for a 4K-wide JPEG instead
    let url = v.get("url").and_then(|u| u.as_str()).map(|u| format!("{u}{}w=3840&fm=jpg&q=85", if u.contains('?') { "&" } else { "?" })).unwrap_or_else(|| photo.full.clone());
    std::fs::create_dir_all(assets_dir).map_err(|e| e.to_string())?;
    let file_name = format!("unsplash-{}.jpg", safe_id(&photo.id));
    fetch(&url, &assets_dir.join(&file_name), MAX_IMAGE_BYTES, Some((progress, ctx)))?;
    Ok(Applied { photo, file_name })
}

/// GET an image from Unsplash's CDN into `dest` (written via a temp file, at most `max` bytes).
/// With `progress`, the downloaded fraction is stored there as it arrives (when the server
/// sends the length) and the UI is asked to repaint.
fn fetch(url: &str, dest: &Path, max: u64, progress: Option<(&AtomicU32, &egui::Context)>) -> Result<(), String> {
    if !url.starts_with("https://images.unsplash.com/") {
        return Err("not an Unsplash image".into());
    }
    let resp = ureq::get(url).timeout(std::time::Duration::from_secs(60)).call().map_err(|e| format!("download failed: {e}"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0);
    let mut reader = resp.into_reader().take(max + 1);
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(max) as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let (Some((p, ctx)), Some(total)) = (progress, total) {
            p.store((bytes.len() as f32 / total as f32).to_bits(), Ordering::Relaxed);
            ctx.request_repaint();
        }
    }
    if bytes.len() as u64 > max {
        return Err("image too large".into());
    }
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("unsplash"))
}

/// Photo ids are `[A-Za-z0-9_-]`; anything else is dropped before it reaches a file name.
fn safe_id(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
}

/// `url` with Unsplash's referral parameters.
pub fn referral(url: &str) -> String {
    format!("{url}{}{UTM}", if url.contains('?') { "&" } else { "?" })
}

/// The project-file record of an Unsplash background (`config.styles.background.unsplash`).
pub fn background_value(a: &Applied) -> serde_json::Value {
    serde_json::json!({
        "image": a.file_name,
        "id": a.photo.id,
        "author": a.photo.author,
        "authorUrl": a.photo.author_url,
        "photoUrl": a.photo.photo_url,
        "color": a.photo.color,
    })
}

/// The credit to paste into a video description for an Unsplash background (`value` as from
/// [`background_value`]): the photographer and the photo, each with its link.
pub fn credit(value: &serde_json::Value) -> String {
    let get = |k: &str| value.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let mut s = format!("Photo by {}", get("author").unwrap_or("an Unsplash photographer"));
    if let Some(url) = get("authorUrl") {
        s += &format!(" ({})", referral(url));
    }
    s += &format!(" on Unsplash ({})", referral(get("photoUrl").unwrap_or("https://unsplash.com/")));
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn credit_names_the_photographer() {
        let v = serde_json::json!({ "author": "Ann", "authorUrl": "https://unsplash.com/@ann", "photoUrl": "https://unsplash.com/photos/abc" });
        assert_eq!(super::credit(&v), "Photo by Ann (https://unsplash.com/@ann?utm_source=screenforge&utm_medium=referral) on Unsplash (https://unsplash.com/photos/abc?utm_source=screenforge&utm_medium=referral)");
        assert_eq!(super::credit(&serde_json::json!({})), "Photo by an Unsplash photographer on Unsplash (https://unsplash.com/?utm_source=screenforge&utm_medium=referral)");
    }

    #[test]
    fn ids_and_links_are_sanitised() {
        assert_eq!(super::safe_id("a/../b_C-9"), "ab_C-9");
        assert_eq!(super::referral("https://unsplash.com/@x"), "https://unsplash.com/@x?utm_source=screenforge&utm_medium=referral");
        assert_eq!(super::referral("https://unsplash.com/?a=1"), "https://unsplash.com/?a=1&utm_source=screenforge&utm_medium=referral");
        assert!(super::fetch("https://evil.example/x.jpg", std::path::Path::new("/tmp/never"), 10, None).is_err());
    }

    #[test]
    fn search_reply_parses() {
        let v = serde_json::json!({ "results": [{ "id": "abc", "thumb": "https://images.unsplash.com/t", "full": "https://images.unsplash.com/f", "author": "Ann", "downloadLocation": "https://api.unsplash.com/photos/abc/download" }], "totalPages": 3 });
        let r: super::SearchReply = serde_json::from_value(v).unwrap();
        assert_eq!(r.results[0].author.as_deref(), Some("Ann"));
        assert_eq!(r.total_pages, 3);
    }
}
