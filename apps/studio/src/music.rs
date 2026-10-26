//! Audio → Find music: search Creative Commons tracks on Jamendo through the ScreenForge
//! backend's `jamendo` proxy, or import an audio file from a link. Either way the file is
//! downloaded into the project's `assets/` so it plays and exports offline like a track added
//! with "Add background audio…".
//!
//! A Jamendo track gets a credit file beside it (`assets/<stem>.credit.json`) so the sidebar can
//! show who to credit while the track is in use.
//!
//! Licences: music under a video is an Adaptation in every Creative Commons licence (CC 3.0
//! §1(a), "synching"). NoDerivs (ND) forbids that and ShareAlike (SA) would put the user's video
//! under CC BY-SA, so only CC BY tracks are offered, and CC BY-NC when the user says the video
//! earns nothing. The backend filters by licence; [`usable`] checks again here.
//!
//! Favourite tracks (the heart on a row) and the ones last added to a video are kept for the
//! whole app in `music-library.json` in the app's data folder, so they show in every project.
//!
//! Network work runs on background threads; the sidebar polls [`Browser::poll`] every frame.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Largest audio file we accept (bytes).
const MAX_AUDIO_BYTES: u64 = 150 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artist: String,
    /// Seconds.
    #[serde(default)]
    pub duration: u32,
    /// The MP3 on Jamendo's storage.
    pub download: String,
    /// Album cover, 100 px.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub share_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchReply {
    results: Vec<Track>,
    #[serde(default)]
    total_pages: u32,
}

/// Who to credit for a downloaded track (`assets/<stem>.credit.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credit {
    pub title: String,
    pub artist: String,
    /// The track's page.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub license_url: Option<String>,
}

/// The licence elements of a Creative Commons licence link, lower case: `by`, `by-nc` …
fn cc_kind(license_url: Option<&str>) -> Option<String> {
    let rest = license_url?.split("creativecommons.org/licenses/").nth(1)?;
    rest.split('/').next().filter(|k| !k.is_empty()).map(str::to_ascii_lowercase)
}

/// Whether a track under `license_url` may be set under a video: CC BY, or CC BY-NC when
/// `noncommercial` (the video earns nothing). Never ND, SA, or an unknown licence.
pub fn usable(license_url: Option<&str>, noncommercial: bool) -> bool {
    match cc_kind(license_url).as_deref() {
        Some("by") => true,
        Some("by-nc") => noncommercial,
        _ => false,
    }
}

impl Credit {
    /// `CC BY-NC-SA 3.0` for a Creative Commons licence link.
    pub fn license(&self) -> Option<String> {
        let url = self.license_url.as_deref()?;
        let rest = url.split("/licenses/").nth(1)?;
        let mut parts = rest.split('/').filter(|s| !s.is_empty());
        let kind = parts.next()?.to_uppercase();
        Some(match parts.next() {
            Some(v) => format!("CC {kind} {v}"),
            None => format!("CC {kind}"),
        })
    }

    /// The licence forbids commercial use: the video must earn nothing.
    pub fn is_noncommercial(&self) -> bool {
        cc_kind(self.license_url.as_deref()).is_some_and(|k| k.split('-').any(|e| e == "nc"))
    }

    /// The credit the licence asks for, to paste into a video description: title, artist, where
    /// the track is, and the licence with its link.
    pub fn text(&self) -> String {
        let mut s = format!("Music: \"{}\" by {}", self.title, self.artist);
        s += &format!(" ({})", self.url.as_deref().unwrap_or("jamendo.com"));
        if let Some(l) = self.license() {
            s += &format!(", licensed under {l}");
            if let Some(u) = &self.license_url {
                s += &format!(" ({})", u.replacen("http://", "https://", 1));
            }
        }
        s
    }
}

/// A downloaded track, ready to become the background audio.
pub struct Applied {
    /// File name inside the project's `assets/`.
    pub file_name: String,
}

/// What is downloading: a Jamendo track (by id) or a link.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Track(String),
    Link,
}

/// The tab of Audio → Background audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The bundled tracks and "Add background audio…".
    #[default]
    Library,
    /// Jamendo search results (popular tracks for an empty search).
    Jamendo,
    Favorites,
    Recent,
    /// Import from a link.
    Link,
}

/// How many tracks "Recent" keeps.
const RECENT: usize = 30;

/// Favourite and recently used tracks, saved for the whole app.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    #[serde(default)]
    pub favorites: Vec<Track>,
    /// Newest first.
    #[serde(default)]
    pub recent: Vec<Track>,
}

impl Library {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.data_dir().join("music-library.json"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = write(&path, &bytes);
        }
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|t| t.id == id)
    }

    /// Heart or un-heart `track` (newest favourites first).
    pub fn toggle_favorite(&mut self, track: &Track) {
        if self.is_favorite(&track.id) {
            self.favorites.retain(|t| t.id != track.id);
        } else {
            self.favorites.insert(0, track.clone());
        }
        self.save();
    }

    /// `track` was added to a video: move it to the front of "Recent".
    pub fn note_used(&mut self, track: &Track) {
        self.recent.retain(|t| t.id != track.id);
        self.recent.insert(0, track.clone());
        self.recent.truncate(RECENT);
        self.save();
    }
}

pub struct Browser {
    pub query: String,
    pub instrumental: bool,
    /// Also offer CC BY-NC tracks: the user says the video earns nothing.
    pub noncommercial: bool,
    pub results: Vec<Track>,
    page: u32,
    total_pages: u32,
    pub error: Option<String>,
    /// The first page loads when the section is first shown.
    started: bool,
    search: Option<(u32, Receiver<Result<SearchReply, String>>)>,
    pub source: Source,
    /// Loaded when the section is first shown.
    pub library: Library,
    library_loaded: bool,
    /// The Jamendo track downloading, to note in "Recent" once it is added.
    pending: Option<Track>,
    thumbs: HashMap<String, PathBuf>,
    thumb_rx: Vec<Receiver<(String, PathBuf)>>,
    /// The link typed in "Import from a link".
    pub link: String,
    pub link_error: Option<String>,
    pub applying: Option<Job>,
    apply_rx: Option<Receiver<Result<Applied, String>>>,
    /// Download progress as f32 bits: 0..1, or negative while the size is unknown.
    progress: Arc<AtomicU32>,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            query: String::new(),
            instrumental: true,
            noncommercial: false,
            results: vec![],
            page: 0,
            total_pages: 0,
            error: None,
            started: false,
            search: None,
            source: Source::Library,
            library: Library::default(),
            library_loaded: false,
            pending: None,
            thumbs: HashMap::new(),
            thumb_rx: vec![],
            link: String::new(),
            link_error: None,
            applying: None,
            apply_rx: None,
            progress: Default::default(),
        }
    }
}

impl Browser {
    pub fn is_busy(&self) -> bool {
        self.search.is_some() || self.apply_rx.is_some()
    }

    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    pub fn can_load_more(&self) -> bool {
        self.search.is_none() && self.page > 0 && self.page < self.total_pages
    }

    /// Load the popular tracks the first time the section is shown.
    pub fn ensure_started(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            self.ensure_library();
            let saved: Vec<Track> = self.library.favorites.iter().chain(&self.library.recent).cloned().collect();
            self.fetch_thumbs(ctx, &saved);
            self.search(ctx, 1);
        }
    }

    /// Read the saved favourites and recent tracks once.
    pub fn ensure_library(&mut self) {
        if !self.library_loaded {
            self.library_loaded = true;
            self.library = Library::load();
        }
    }

    /// Search for `self.query` (empty: the month's popular tracks); page 1 replaces the results,
    /// later pages are appended.
    pub fn search(&mut self, ctx: &egui::Context, page: u32) {
        let (tx, rx) = channel();
        let (query, instrumental, noncommercial, ctx) = (self.query.trim().to_string(), self.instrumental, self.noncommercial, ctx.clone());
        std::thread::spawn(move || {
            let reply = search_page(&query, page, instrumental, noncommercial);
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

    /// Fraction of the file being downloaded, `None` while its size is not known yet.
    pub fn progress(&self) -> Option<f32> {
        let v = f32::from_bits(self.progress.load(Ordering::Relaxed));
        (v >= 0.0).then_some(v.min(1.0))
    }

    /// The tracks the current view lists.
    pub fn tracks(&self) -> &[Track] {
        match self.source {
            Source::Favorites => &self.library.favorites,
            Source::Recent => &self.library.recent,
            _ => &self.results,
        }
    }

    /// Download Jamendo `track` into `assets_dir`, with its credit file.
    pub fn apply_track(&mut self, ctx: &egui::Context, track: Track, assets_dir: PathBuf) {
        if self.apply_rx.is_some() {
            return;
        }
        self.pending = Some(track.clone());
        self.start(ctx, Job::Track(track.id.clone()), move |progress, ctx| download_track(&track, &assets_dir, progress, ctx));
        self.error = None;
    }

    /// Download `self.link` into `assets_dir`.
    pub fn import_link(&mut self, ctx: &egui::Context, assets_dir: PathBuf) {
        let link = self.link.trim().to_string();
        self.start(ctx, Job::Link, move |progress, ctx| download_link(&link, &assets_dir, progress, ctx));
        self.link_error = None;
    }

    fn start(&mut self, ctx: &egui::Context, job: Job, work: impl FnOnce(&AtomicU32, &egui::Context) -> Result<Applied, String> + Send + 'static) {
        if self.apply_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.applying = Some(job);
        self.progress.store((-1f32).to_bits(), Ordering::Relaxed);
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(&progress, &ctx));
            ctx.request_repaint();
        });
        self.apply_rx = Some(rx);
    }

    /// Local album cover of a result, once downloaded.
    pub fn thumb(&self, id: &str) -> Option<&Path> {
        self.thumbs.get(id).map(PathBuf::as_path)
    }

    fn fetch_thumbs(&mut self, ctx: &egui::Context, tracks: &[Track]) {
        let Some(dir) = directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("jamendo")) else { return };
        let todo: Vec<(String, String)> = tracks.iter().filter(|t| !self.thumbs.contains_key(&t.id)).filter_map(|t| Some((t.id.clone(), t.image.clone().filter(|u| is_jamendo(u))?))).collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            for (id, url) in todo {
                let path = dir.join(format!("{}.jpg", safe_name(&id)));
                let ok = path.is_file() || fetch(&url, 2 * 1024 * 1024, &AtomicU32::new(0), &ctx).and_then(|(b, _)| write(&path, &b)).is_ok();
                if ok {
                    let _ = tx.send((id, path));
                    ctx.request_repaint();
                }
            }
        });
        self.thumb_rx.push(rx);
    }

    /// Collect finished work; returns a track that finished downloading.
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
        let thumbs = &mut self.thumbs;
        self.thumb_rx.retain(|rx| loop {
            match rx.try_recv() {
                Ok((id, path)) => {
                    thumbs.insert(id, path);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break false,
            }
        });
        let rx = self.apply_rx.as_ref()?;
        let done = rx.try_recv().ok()?;
        self.apply_rx = None;
        let job = self.applying.take();
        let pending = self.pending.take();
        match done {
            Ok(a) => {
                match (&job, pending) {
                    (Some(Job::Link), _) => self.link.clear(),
                    (_, Some(track)) => self.library.note_used(&track),
                    _ => {}
                }
                Some(a)
            }
            Err(e) => {
                match job {
                    Some(Job::Link) => self.link_error = Some(e),
                    _ => self.error = Some(e),
                }
                None
            }
        }
    }
}

/// One page of results for `query`, blocking (the agent runs on its own thread).
/// Only CC BY tracks: the agent cannot know whether the video earns money.
pub fn search_blocking(query: &str, instrumental: bool) -> Result<Vec<Track>, String> {
    search_page(query, 1, instrumental, false).map(|r| r.results)
}

/// Jamendo sends names HTML-escaped ("Axl &amp; Arth").
fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#039;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Page `page` of results, without tracks whose licence does not allow them under the video.
fn search_page(query: &str, page: u32, instrumental: bool, noncommercial: bool) -> Result<SearchReply, String> {
    let v = sf_license::call_backend("jamendo", serde_json::json!({ "query": query.trim(), "page": page, "instrumental": instrumental, "noncommercial": noncommercial }))?;
    let mut reply = serde_json::from_value::<SearchReply>(v).map_err(|e| format!("unexpected reply: {e}"))?;
    reply.results.retain(|t| usable(t.license_url.as_deref(), noncommercial));
    for t in &mut reply.results {
        t.name = unescape(&t.name);
        t.artist = unescape(&t.artist);
    }
    Ok(reply)
}

/// File name of a Jamendo track inside the project's `assets/`. An underscore, not a hyphen:
/// `background_audio_key` reads `-1381641` as a bundler hash and would drop it.
pub fn track_file_name(track: &Track) -> String {
    format!("jamendo_{}.mp3", safe_name(&track.id))
}

/// Fetch a Jamendo track's MP3 into `assets_dir` and write its credit file.
pub fn download_track(track: &Track, assets_dir: &Path, progress: &AtomicU32, ctx: &egui::Context) -> Result<Applied, String> {
    if !is_jamendo(&track.download) {
        return Err("not a Jamendo download".into());
    }
    if !usable(track.license_url.as_deref(), true) {
        return Err("this track's licence does not allow it under a video".into());
    }
    std::fs::create_dir_all(assets_dir).map_err(|e| e.to_string())?;
    let file_name = track_file_name(track);
    let stem = file_name.trim_end_matches(".mp3");
    let dest = assets_dir.join(&file_name);
    if !dest.is_file() {
        let (bytes, _) = fetch(&track.download, MAX_AUDIO_BYTES, progress, ctx)?;
        if sniff(&bytes) != Some("mp3") {
            return Err("Jamendo did not send an MP3".into());
        }
        write(&dest, &bytes)?;
    }
    let credit = Credit { title: track.name.clone(), artist: track.artist.clone(), url: track.share_url.clone(), license_url: track.license_url.clone() };
    let _ = std::fs::write(assets_dir.join(format!("{stem}.credit.json")), serde_json::to_vec_pretty(&credit).unwrap_or_default());
    Ok(Applied { file_name })
}

/// Fetch an audio file from `link` into `assets_dir`, named after the link and given the
/// extension of what actually arrived.
pub fn download_link(link: &str, assets_dir: &Path, progress: &AtomicU32, ctx: &egui::Context) -> Result<Applied, String> {
    let url = normalise_link(link)?;
    let (bytes, disposition) = fetch(&url, MAX_AUDIO_BYTES, progress, ctx)?;
    let Some(ext) = sniff(&bytes) else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_lowercase();
        return Err(if head.contains("<html") || head.contains("<!doctype") {
            "That link opens a web page, not an audio file. Use the page's download link instead.".into()
        } else {
            "That file is not audio ScreenForge can play (MP3, M4A, AAC, WAV, OGG, FLAC or AIFF).".into()
        });
    };
    std::fs::create_dir_all(assets_dir).map_err(|e| e.to_string())?;
    let name = disposition.as_deref().and_then(disposition_file_name).or_else(|| url_file_name(&url)).unwrap_or_default();
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
    let stem = match safe_name(stem) {
        s if s.trim().is_empty() => "linked-audio".to_string(),
        s => s,
    };
    let file_name = free_name(assets_dir, &stem, ext);
    write(&assets_dir.join(&file_name), &bytes)?;
    Ok(Applied { file_name })
}

/// The credit of `file_name` in `assets_dir`, when it came from Jamendo.
pub fn credit(assets_dir: &Path, file_name: &str) -> Option<Credit> {
    let base = file_name.rsplit('/').next().unwrap_or(file_name);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let bytes = std::fs::read(assets_dir.join(format!("{stem}.credit.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn is_jamendo(url: &str) -> bool {
    url.strip_prefix("https://").and_then(|r| r.split(['/', '?']).next()).is_some_and(|host| host == "jamendo.com" || host.ends_with(".jamendo.com"))
}

/// Check a typed link and turn share links that open a preview page into direct downloads.
fn normalise_link(link: &str) -> Result<String, String> {
    let link = link.trim();
    if !(link.starts_with("https://") || link.starts_with("http://")) {
        return Err("Paste a link that starts with https://".into());
    }
    // Dropbox: ?dl=0 shows a preview page, dl=1 sends the file
    if link.contains("dropbox.com/") {
        if link.contains("dl=0") {
            return Ok(link.replacen("dl=0", "dl=1", 1));
        }
        if !link.contains("dl=1") && !link.contains("raw=1") {
            return Ok(format!("{link}{}dl=1", if link.contains('?') { "&" } else { "?" }));
        }
    }
    Ok(link.to_string())
}

/// GET `url` (at most `max` bytes), storing the downloaded fraction in `progress` as it arrives
/// when the server sends the length. Returns the body and the Content-Disposition header.
fn fetch(url: &str, max: u64, progress: &AtomicU32, ctx: &egui::Context) -> Result<(Vec<u8>, Option<String>), String> {
    let resp = ureq::get(url).set("User-Agent", "ScreenForge").timeout(std::time::Duration::from_secs(180)).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("download failed: the server answered {code}"),
        e => format!("download failed: {e}"),
    })?;
    let disposition = resp.header("Content-Disposition").map(str::to_string);
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0);
    if total.is_some_and(|t| t > max) {
        return Err("that file is too large (over 150 MB)".into());
    }
    let mut reader = resp.into_reader().take(max + 1);
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(max) as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(total) = total {
            progress.store((bytes.len() as f32 / total as f32).to_bits(), Ordering::Relaxed);
            ctx.request_repaint();
        }
    }
    if bytes.len() as u64 > max {
        return Err("that file is too large (over 150 MB)".into());
    }
    Ok((bytes, disposition))
}

/// Write `bytes` to `dest` via a temp file.
fn write(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// The audio format of a file from its first bytes, as a file extension.
fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.len() < 12 {
        return None;
    }
    match b {
        [b'I', b'D', b'3', ..] => Some("mp3"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'A', b'V', b'E', ..] => Some("wav"),
        [b'F', b'O', b'R', b'M', _, _, _, _, b'A', b'I', b'F', ..] => Some("aiff"),
        [b'O', b'g', b'g', b'S', ..] => Some("ogg"),
        [b'f', b'L', b'a', b'C', ..] => Some("flac"),
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => Some("m4a"),
        // ADTS AAC: frame sync, layer bits 00
        [0xFF, b1, ..] if b1 & 0xF6 == 0xF0 => Some("aac"),
        // MPEG audio frame sync, layer bits not 00
        [0xFF, b1, ..] if b1 & 0xE0 == 0xE0 && b1 & 0x06 != 0 => Some("mp3"),
        _ => None,
    }
}

/// `filename="…"` (or `filename*=UTF-8''…`) of a Content-Disposition header.
fn disposition_file_name(h: &str) -> Option<String> {
    for part in h.split(';').map(str::trim) {
        if let Some(v) = part.strip_prefix("filename*=") {
            let v = v.rsplit("''").next().unwrap_or(v);
            return Some(percent_decode(v.trim_matches('"')));
        }
    }
    h.split(';').map(str::trim).find_map(|p| p.strip_prefix("filename=")).map(|v| v.trim_matches('"').to_string())
}

/// Last path segment of `url`, decoded.
fn url_file_name(url: &str) -> Option<String> {
    let rest = url.split(['?', '#']).next()?.split_once("://")?.1;
    let path = rest.split_once('/')?.1;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty()).then(|| percent_decode(last))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A file name stem without path separators or control characters, at most 80 characters.
fn safe_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')' | '\'' | ',')).take(80).collect::<String>().trim().to_string()
}

/// `<stem>.<ext>`, or `<stem> 2.<ext>` … when that name is taken in `dir`.
fn free_name(dir: &Path, stem: &str, ext: &str) -> String {
    let mut name = format!("{stem}.{ext}");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{stem} {n}.{ext}");
        n += 1;
    }
    name
}

/// `m:ss` for a duration in seconds.
pub fn fmt_duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_sniffed() {
        let pad = |head: &[u8]| [head, &[0u8; 16]].concat();
        assert_eq!(sniff(&pad(b"ID3\x04")), Some("mp3"));
        assert_eq!(sniff(&pad(&[0xFF, 0xFB, 0x90])), Some("mp3"));
        assert_eq!(sniff(&pad(&[0xFF, 0xF1, 0x50])), Some("aac"));
        assert_eq!(sniff(&pad(b"RIFF\0\0\0\0WAVE")), Some("wav"));
        assert_eq!(sniff(&pad(b"\0\0\0\x20ftypM4A ")), Some("m4a"));
        assert_eq!(sniff(&pad(b"OggS")), Some("ogg"));
        assert_eq!(sniff(&pad(b"fLaC")), Some("flac"));
        assert_eq!(sniff(&pad(b"<!DOCTYPE html>")), None);
    }

    #[test]
    fn links_are_checked_and_named() {
        assert!(normalise_link("ftp://x/a.mp3").is_err());
        assert_eq!(normalise_link(" https://www.dropbox.com/s/x/a.mp3?dl=0 ").unwrap(), "https://www.dropbox.com/s/x/a.mp3?dl=1");
        assert_eq!(normalise_link("https://www.dropbox.com/s/x/a.mp3").unwrap(), "https://www.dropbox.com/s/x/a.mp3?dl=1");
        assert_eq!(url_file_name("https://cdn.example.com/music/My%20Song.mp3?x=1").as_deref(), Some("My Song.mp3"));
        assert_eq!(url_file_name("https://example.com/"), None);
        assert_eq!(disposition_file_name("attachment; filename=\"Calm Piano.mp3\"").as_deref(), Some("Calm Piano.mp3"));
        assert_eq!(disposition_file_name("attachment; filename*=UTF-8''Caf%C3%A9.mp3").as_deref(), Some("Café.mp3"));
        assert_eq!(safe_name("../../etc/passwd"), "etcpasswd");
        assert!(is_jamendo("https://prod-1.storage.jamendo.com/download/track/1/mp32/"));
        assert!(!is_jamendo("https://evil.example/jamendo.com/x.mp3"));
        assert!(!is_jamendo("http://storage.jamendo.com/x"));
    }

    #[test]
    fn credits_read_well() {
        let c = Credit { title: "Lights".into(), artist: "Ann".into(), url: Some("https://www.jamendo.com/track/1".into()), license_url: Some("http://creativecommons.org/licenses/by-nc-sa/3.0/".into()) };
        assert_eq!(c.license().as_deref(), Some("CC BY-NC-SA 3.0"));
        assert!(c.is_noncommercial());
        assert_eq!(c.text(), "Music: \"Lights\" by Ann (https://www.jamendo.com/track/1), licensed under CC BY-NC-SA 3.0 (https://creativecommons.org/licenses/by-nc-sa/3.0/)");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("jamendo_1.credit.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(credit(dir.path(), "jamendo_1.mp3"), Some(c));
        assert_eq!(credit(dir.path(), "other.mp3"), None);
        assert_eq!(free_name(dir.path(), "jamendo_1", "credit.json"), "jamendo_1 2.credit.json");
        assert_eq!(unescape("Axl &amp; Arth &quot;Live&quot;"), "Axl & Arth \"Live\"");
        assert_eq!(fmt_duration(151), "2:31");
    }

    #[test]
    fn only_licences_that_allow_a_video() {
        let cc = |k: &str| Some(format!("http://creativecommons.org/licenses/{k}/3.0/"));
        assert!(usable(cc("by").as_deref(), false));
        assert!(usable(Some("http://creativecommons.org/licenses/by/2.5/se/"), false));
        assert!(!usable(cc("by-nc").as_deref(), false));
        assert!(usable(cc("by-nc").as_deref(), true));
        for k in ["by-nd", "by-sa", "by-nc-nd", "by-nc-sa"] {
            assert!(!usable(cc(k).as_deref(), true), "{k} never");
        }
        assert!(!usable(None, true));
        assert!(!usable(Some("https://example.com/licenses/by/3.0/"), true));
        let by = Credit { title: "T".into(), artist: "A".into(), url: None, license_url: cc("by") };
        assert!(!by.is_noncommercial());
        let nd = Track { id: "1".into(), name: "T".into(), artist: "A".into(), duration: 1, download: "https://prod-1.storage.jamendo.com/download/track/1/mp32/".into(), image: None, license_url: cc("by-nd"), share_url: None };
        let dir = tempfile::tempdir().unwrap();
        assert!(download_track(&nd, dir.path(), &AtomicU32::new(0), &egui::Context::default()).err().unwrap().contains("licence"));
    }

    #[test]
    fn favorites_and_recent() {
        let t = |id: &str| Track { id: id.into(), name: id.into(), artist: "A".into(), duration: 1, download: String::new(), image: None, license_url: None, share_url: None };
        let mut lib = Library::default();
        lib.favorites.insert(0, t("1"));
        assert!(lib.is_favorite("1") && !lib.is_favorite("2"));
        lib.recent = (0..RECENT).map(|i| t(&i.to_string())).collect();
        // what note_used does, without touching the real file
        lib.recent.retain(|x| x.id != "5");
        lib.recent.insert(0, t("5"));
        lib.recent.truncate(RECENT);
        assert_eq!((lib.recent[0].id.as_str(), lib.recent.len()), ("5", RECENT));
        let back: Library = serde_json::from_slice(&serde_json::to_vec(&lib).unwrap()).unwrap();
        assert_eq!(back.favorites[0].id, "1");
    }

    #[test]
    fn search_reply_parses() {
        let v = serde_json::json!({ "results": [{ "id": "7", "name": "Lights", "artist": "Ann", "duration": 151, "download": "https://prod-1.storage.jamendo.com/download/track/7/mp32/", "licenseUrl": null }], "totalPages": 4 });
        let r: SearchReply = serde_json::from_value(v).unwrap();
        assert_eq!(r.results[0].artist, "Ann");
        assert_eq!(r.total_pages, 4);
    }
}
