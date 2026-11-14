//! Cloud share links (Pro): a project rendered to MP4 on a background thread, uploaded straight
//! to the share storage (Cloudflare R2, through URLs the `share` backend function signs) and
//! answered with a `screenforge.studio/s/<id>` link. Export with "Make a share link too" uploads
//! its file. A project has one link: sharing it again replaces the video behind the same URL.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use sf_media::VideoCodec;

/// How the shared video is rendered. Always H.264 MP4, the one format every browser plays.
#[derive(Clone, Copy, Debug)]
pub struct ShareSettings {
    pub fps: f64,
    pub max_height: u32,
    /// 0..1, as `sf_export::ExportOptions::quality`
    pub quality: f64,
    pub include_audio: bool,
}

/// What a share uploads: the project rendered for it, or a file Export just wrote (an H.264
/// MP4, so it is not rendered twice).
#[derive(Clone, Debug)]
pub enum Source {
    Render(ShareSettings),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Rendering(f32),
    Uploading(f32),
}

enum Msg {
    Stage(Stage),
    Done(Result<String, String>),
}

/// A share in progress (or just finished) for one project.
pub struct ShareJob {
    pub project: PathBuf,
    pub title: String,
    pub stage: Stage,
    /// The link, or why there is none. Set once the job is over.
    pub result: Option<Result<String, String>>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

impl ShareJob {
    /// Render `project` (or take the exported file) and upload it on a background thread.
    /// `repaint` wakes the UI as the job moves on.
    pub fn start(project: PathBuf, title: String, source: Source, repaint: egui::Context) -> ShareJob {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (path, name, stop) = (project.clone(), title.clone(), cancel.clone());
        let first = if matches!(source, Source::File(_)) { Stage::Uploading(0.0) } else { Stage::Rendering(0.0) };
        std::thread::Builder::new()
            .name("share-link".into())
            .spawn(move || {
                let send = |m: Msg| {
                    let _ = tx.send(m);
                    repaint.request_repaint();
                };
                let r = run(&path, &name, source, &stop, &|s| send(Msg::Stage(s)));
                send(Msg::Done(r));
            })
            .expect("spawn share job");
        ShareJob { project, title, stage: first, result: None, rx, cancel }
    }

    /// Take the thread's news. True once, when the job has just finished.
    pub fn poll(&mut self) -> bool {
        let mut finished = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Stage(s) => self.stage = s,
                Msg::Done(r) => {
                    self.result = Some(r);
                    finished = true;
                }
            }
        }
        finished
    }

    pub fn is_running(&self) -> bool {
        self.result.is_none()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Progress over the whole job: rendering is the first 60 %, uploading the rest.
    pub fn fraction(&self) -> f32 {
        match self.stage {
            Stage::Rendering(f) => 0.6 * f,
            Stage::Uploading(f) => 0.6 + 0.4 * f,
        }
    }

    /// What the job is doing now, shown under its progress bar.
    pub fn label(&self) -> String {
        match self.stage {
            Stage::Rendering(_) => "Rendering the video…".to_string(),
            Stage::Uploading(_) => "Uploading…".to_string(),
        }
    }
}

const CANCELLED: &str = "cancelled";

/// One `share` request. Its 404 means the link is gone (not that the service is missing).
fn call(req: serde_json::Value) -> Result<serde_json::Value, String> {
    sf_license::call_backend_status("share", req).map_err(|(code, e)| if code == 404 && e == "not found" { "That link no longer exists.".into() } else { e })
}

/// Whether this computer's account may make share links (Pro, or staff on any license), as
/// the server last said, and the license generation it was asked about.
static ALLOWED: std::sync::Mutex<Option<(u64, bool)>> = std::sync::Mutex::new(None);
static ASKING: AtomicBool = AtomicBool::new(false);

/// May this computer make share links? A Pro key says yes offline; otherwise the server's last
/// answer (staff). When the license changed since (a renewal re-saves the key), the last answer
/// stands while the server is asked again in the background, so Export does not flip to the
/// Pro offer and back; the answer is kept on disk for the next launch too.
pub fn allowed() -> bool {
    if sf_license::saved_status().is_pro() {
        return true;
    }
    let generation = sf_license::license_generation();
    let mut last = ALLOWED.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none() {
        // u64::MAX: from disk, never the current generation, so it is asked again once
        *last = allowed_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| (u64::MAX, s.trim() == "yes"));
    }
    match *last {
        Some((g, ok)) if g == generation => ok,
        stale => {
            drop(last);
            ask_allowed(generation);
            stale.is_some_and(|(_, ok)| ok)
        }
    }
}

fn allowed_file() -> Option<PathBuf> {
    setting_file().map(|f| f.with_file_name("share-allowed"))
}

fn remember_allowed(generation: u64, ok: bool) {
    *ALLOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, ok));
    if let Some(f) = allowed_file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, if ok { "yes" } else { "no" });
    }
}

fn ask_allowed(generation: u64) {
    if ASKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let answer = sf_license::owner_auth().map(|mut req| {
            req["action"] = "allowed".into();
            sf_license::call_backend_status("share", req).ok().and_then(|v| v["allowed"].as_bool())
        });
        match answer {
            // not signed in: no links
            None => remember_allowed(generation, false),
            Some(Some(ok)) => remember_allowed(generation, ok),
            // offline or a server error: asked again next time
            Some(None) => {}
        }
        ASKING.store(false, Ordering::SeqCst);
    });
}

fn run(project: &Path, title: &str, source: Source, cancel: &AtomicBool, stage: &dyn Fn(Stage)) -> Result<String, String> {
    let auth = sf_license::owner_auth().ok_or("Sign in to ScreenForge Pro on this computer to share links (Export → Sign in).")?;
    let s = match source {
        Source::File(video) => {
            // the exported file stays where the user saved it
            let info = sf_media::probe::probe(&video).map_err(|e| format!("Could not read the exported video: {e:#}"))?;
            let v = info.video();
            let dims = (v.and_then(|v| v.width).unwrap_or(0), v.and_then(|v| v.height).unwrap_or(0), info.duration_ms().unwrap_or(0.0).max(0.0) as u64);
            return upload(project, &video, title, dims, auth, cancel, &|f| stage(Stage::Uploading(f)));
        }
        Source::Render(s) => s,
    };
    let dir = directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.cache_dir().join("shares")).ok_or("no cache folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let video = dir.join(format!("{stamp}.mp4"));
    let result = (|| {
        let (width, height, duration_ms) = render(project, &video, s, cancel, stage)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        stage(Stage::Uploading(0.0));
        upload(project, &video, title, (width, height, duration_ms), auth, cancel, &|f| stage(Stage::Uploading(f)))
    })();
    let _ = std::fs::remove_file(&video);
    result
}

/// Render `project` to `out` (H.264 MP4). Returns the video's width, height and length in ms.
fn render(project: &Path, out: &Path, s: ShareSettings, cancel: &AtomicBool, stage: &dyn Fn(Stage)) -> Result<(u32, u32, u64), String> {
    let go = || -> anyhow::Result<(u32, u32, u64)> {
        let gpu = sf_render::Gpu::headless()?;
        let mut session = sf_session::Session::open(project, gpu.clone())?;
        let mut comp = sf_render::Compositor::new(gpu);
        let size = session.output_size(s.max_height);
        let duration = session.duration_ms().max(0.0) as u64;
        let opts = sf_export::ExportOptions { output: out.to_path_buf(), fps: s.fps, max_height: s.max_height, codec: VideoCodec::H264, quality: s.quality, include_audio: s.include_audio, range_ms: None };
        sf_export::export_cancellable(&mut session, &mut comp, &opts, cancel, |p| stage(Stage::Rendering(p.frame as f32 / p.total_frames.max(1) as f32)))?;
        Ok((size.width, size.height, duration))
    };
    go().map_err(|e| {
        let e = format!("{e:#}");
        if e.contains("cancelled") {
            CANCELLED.to_string()
        } else {
            format!("Rendering the video failed: {e}")
        }
    })
}

/// The project's QuickLook preview as a JPEG, for the link's poster and unfurl image. Rendered
/// first when the project has none yet (a recording that just stopped may still be writing it).
fn poster_jpeg(project: &Path) -> Option<Vec<u8>> {
    let png = project.join(sf_project::package::QUICKLOOK_PREVIEW);
    if !png.is_file() {
        let _ = sf_session::preview::write_quicklook_preview(project);
    }
    let img = image::open(&png).ok()?.to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).ok()?;
    Some(out)
}

fn upload(project: &Path, video: &Path, title: &str, (width, height, duration_ms): (u32, u32, u64), auth: serde_json::Value, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<String, String> {
    let bytes = std::fs::metadata(video).map_err(|e| format!("The rendered video is missing: {e}"))?.len();
    let poster = poster_jpeg(project);
    let mut req = auth.clone();
    req["action"] = "create".into();
    // the project's one link (the server reuses it)
    let project_id = sf_project::ProjectPackage::open(project).ok().map(|pkg| pkg.project.id);
    if let Some(id) = &project_id {
        req["project"] = id.clone().into();
    }
    req["title"] = title.into();
    req["bytes"] = bytes.into();
    req["width"] = width.into();
    req["height"] = height.into();
    req["duration_ms"] = duration_ms.into();
    if let Some(p) = &poster {
        req["poster"] = p.len().into();
    }
    let created = call(req)?;
    let id = created["id"].as_str().ok_or("The share server sent no link id.")?.to_string();
    let video_url = created["upload"]["video"].as_str().ok_or("The share server sent no upload address.")?;
    let agent = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(15)).timeout_read(std::time::Duration::from_secs(120)).timeout_write(std::time::Duration::from_secs(120)).build();
    // the poster is a nicety: a failed one leaves the link without it
    if let (Some(url), Some(jpg)) = (created["upload"]["poster"].as_str(), &poster) {
        let _ = agent.put(url).set("Content-Type", "image/jpeg").send_bytes(jpg);
    }
    let file = std::fs::File::open(video).map_err(|e| format!("Could not read the rendered video: {e}"))?;
    let reader = Counting { inner: file, sent: 0, total: bytes, cancel, progress };
    match agent.put(video_url).set("Content-Type", "video/mp4").set("Content-Length", &bytes.to_string()).send(reader) {
        Ok(_) => {}
        Err(_) if cancel.load(Ordering::Relaxed) => return Err(CANCELLED.into()),
        Err(ureq::Error::Status(code, _)) => return Err(format!("The upload was refused ({code}). Try sharing again.")),
        Err(e) => return Err(format!("The upload stopped ({}). Check your connection and try again.", e.kind())),
    }
    let mut req = auth;
    req["action"] = "complete".into();
    req["id"] = id.into();
    let done = call(req)?;
    let url = done["url"].as_str().map(String::from).ok_or("The share server sent no link.")?;
    if let Some(id) = &project_id {
        remember_uploaded(id, true);
    }
    Ok(url)
}

/// Projects this computer knows have a video behind their link, so Export says so as it opens
/// instead of after asking the server (which corrects it in the background). Kept on disk.
static UPLOADED: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);

fn uploaded_file() -> Option<PathBuf> {
    setting_file().map(|f| f.with_file_name("shared-projects"))
}

/// Whether this project's link has a video, as last known on this computer.
pub fn known_uploaded(project: &str) -> bool {
    let mut set = UPLOADED.lock().unwrap_or_else(|e| e.into_inner());
    set.get_or_insert_with(|| uploaded_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()).contains(project)
}

pub fn remember_uploaded(project: &str, uploaded: bool) {
    if known_uploaded(project) == uploaded {
        return;
    }
    let mut guard = UPLOADED.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(Default::default);
    if uploaded {
        set.insert(project.to_string());
    } else {
        set.remove(project);
    }
    if let Some(f) = uploaded_file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let mut lines: Vec<&str> = set.iter().map(String::as_str).collect();
        lines.sort();
        let _ = std::fs::write(f, lines.join("\n"));
    }
}

/// The video file as the upload body, reporting progress and stopping when cancelled.
struct Counting<'a> {
    inner: std::fs::File,
    sent: u64,
    total: u64,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(f32),
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(CANCELLED));
        }
        let n = self.inner.read(buf)?;
        let before = self.sent * 100 / self.total.max(1);
        self.sent += n as u64;
        // a repaint per whole percent is plenty
        if self.sent * 100 / self.total.max(1) != before {
            (self.progress)(self.sent as f32 / self.total.max(1) as f32);
        }
        Ok(n)
    }
}

/// A link of this account, as `share` lists them.
#[derive(Clone, Debug)]
pub struct Link {
    pub url: String,
    /// The project (file id) the link belongs to: one link per project.
    pub project: Option<String>,
}

/// This account's links, newest first.
pub fn list() -> Result<Vec<Link>, String> {
    let mut req = sf_license::owner_auth().ok_or("Sign in to see your share links.")?;
    req["action"] = "list".into();
    let r = call(req)?;
    Ok(r["links"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(Link {
                        url: l["url"].as_str()?.into(),
                        project: l["project"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The page a link id opens.
pub const PAGE_URL: &str = "https://screenforge.studio/s/";

/// A project's link id, the same every time: 12 base62 characters of
/// SHA-256("screenforge-share/" + project id). The app shows a project's link before anything
/// is uploaded; the server gives a new link this id (`linkIdFor` in the share function), and
/// links made before keep theirs. Unguessable as long as the project id is (10 random base62
/// characters, kept in the project file).
pub fn link_id(project: &str) -> String {
    use sha2::Digest;
    const ABC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let d = sha2::Sha256::digest(format!("screenforge-share/{project}"));
    d[..12].iter().map(|b| ABC[*b as usize % 62] as char).collect()
}

pub fn link_url(project: &str) -> String {
    format!("{PAGE_URL}{}", link_id(project))
}

/// The link of the project with this id, if it has one.
pub fn project_link(project: &str) -> Result<Option<String>, String> {
    Ok(list()?.into_iter().find(|l| l.project.as_deref() == Some(project)).map(|l| l.url))
}

/// Where "Make a share link too" is remembered (`allowed_file` sits next to it).
fn setting_file() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.config_dir().join("share-with-export"))
}

/// Make a share link of every file Export writes (Pro). Off unless turned on.
pub fn with_export_enabled() -> bool {
    setting_file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|s| s.trim() == "on")
}

pub fn set_with_export(on: bool) {
    if let Some(f) = setting_file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, if on { "on" } else { "off" });
    }
}

/// Is the job's error just a cancel (nothing to tell the user)?
pub fn is_cancel(e: &str) -> bool {
    e == CANCELLED
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    fn file_with(bytes: usize) -> (tempfile::TempDir, std::fs::File) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.mp4");
        std::fs::File::create(&path).unwrap().write_all(&vec![7u8; bytes]).unwrap();
        (dir, std::fs::File::open(&path).unwrap())
    }

    #[test]
    fn upload_body_reports_progress_to_the_end() {
        let (_dir, file) = file_with(10_000);
        let seen = RefCell::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let report = |f: f32| seen.borrow_mut().push(f);
        let mut r = Counting { inner: file, sent: 0, total: 10_000, cancel: &cancel, progress: &report };
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 10_000);
        let seen = seen.borrow();
        assert_eq!(*seen.last().unwrap(), 1.0);
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn upload_body_stops_when_cancelled() {
        let (_dir, file) = file_with(100);
        let cancel = AtomicBool::new(true);
        let mut r = Counting { inner: file, sent: 0, total: 100, cancel: &cancel, progress: &|_| {} };
        let err = r.read(&mut [0u8; 64]).unwrap_err();
        assert!(is_cancel(&err.to_string()));
    }

    #[test]
    fn job_progress_splits_render_and_upload() {
        let (_tx, rx) = channel();
        let mut job = ShareJob { project: PathBuf::new(), title: String::new(), stage: Stage::Rendering(0.5), result: None, rx, cancel: Arc::new(AtomicBool::new(false)) };
        assert!((job.fraction() - 0.3).abs() < 1e-6);
        job.stage = Stage::Uploading(1.0);
        assert!((job.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn link_id_is_the_servers() {
        // the same values as `linkIdFor` in backend/supabase/functions/share/index.ts
        assert_eq!(link_id("iv5Pww9n2l"), "v3BwvryVLJ5B");
        assert_eq!(link_id("abc"), "LyVS9w6Jh3S9");
        assert_eq!(link_url("abc"), "https://screenforge.studio/s/LyVS9w6Jh3S9");
    }

    /// Against the live backend: the account's links, with the project each belongs to.
    /// `cargo test -p studio list_shows_projects -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn list_shows_projects() {
        for l in list().unwrap() {
            println!("{} {:?}", l.url, l.project);
        }
    }

    /// Against the live backend with this computer's license: one project, one link.
    /// `cargo test -p studio share_again_keeps_the_link -- --ignored`
    #[test]
    #[ignore]
    fn share_again_keeps_the_link() {
        let auth = sf_license::owner_auth().expect("signed in");
        let project = format!("e2e-{}", std::process::id());
        let put = |bytes: &[u8]| {
            let mut req = auth.clone();
            req["action"] = "create".into();
            req["project"] = project.clone().into();
            req["title"] = "e2e replace".into();
            req["bytes"] = bytes.len().into();
            let c = call(req).unwrap();
            ureq::put(c["upload"]["video"].as_str().unwrap()).set("Content-Type", "video/mp4").send_bytes(bytes).unwrap();
            let mut req = auth.clone();
            req["action"] = "complete".into();
            req["id"] = c["id"].clone();
            call(req).unwrap();
            c["id"].as_str().unwrap().to_string()
        };
        let first = put(&[1u8; 1000]);
        let get = |id: &str| call(serde_json::json!({ "action": "get", "id": id })).unwrap();
        let v1 = get(&first)["video"].as_str().unwrap().to_string();
        let second = put(&[2u8; 2000]);
        let v2 = get(&second)["video"].as_str().unwrap().to_string();
        let served = ureq::get(&v2).call().ok().and_then(|r| r.header("content-length").map(str::to_string));
        let mut req = auth.clone();
        req["action"] = "delete".into();
        req["id"] = first.clone().into();
        call(req).unwrap();
        assert_eq!(first, second, "same link");
        assert_eq!(first, link_id(&project), "the project's derived id");
        assert_ne!(v1, v2, "new video address");
        assert_eq!(served.as_deref(), Some("2000"), "the new video is served");
    }
}
