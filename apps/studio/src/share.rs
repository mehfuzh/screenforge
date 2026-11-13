//! Cloud share links (Pro): a project rendered to MP4 on a background thread, uploaded straight
//! to the share storage (Cloudflare R2, through URLs the `share` backend function signs) and
//! answered with a `screenforge.studio/s/<id>` link. When a recording stops, Pro computers do
//! this by themselves (see [`on_stop_enabled`]); Export → Get share link does it on demand.

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

impl ShareSettings {
    /// What a recording is shared at when it stops: 1080p, 30 fps, the Social Media preset.
    pub const ON_STOP: ShareSettings = ShareSettings { fps: 30.0, max_height: 1080, quality: 0.75, include_audio: true };
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
    /// Render `project` and upload it on a background thread. `repaint` wakes the UI as the
    /// job moves on.
    pub fn start(project: PathBuf, title: String, settings: ShareSettings, repaint: egui::Context) -> ShareJob {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (path, name, stop) = (project.clone(), title.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("share-link".into())
            .spawn(move || {
                let send = |m: Msg| {
                    let _ = tx.send(m);
                    repaint.request_repaint();
                };
                let r = run(&path, &name, settings, &stop, &|s| send(Msg::Stage(s)));
                send(Msg::Done(r));
            })
            .expect("spawn share job");
        ShareJob { project, title, stage: Stage::Rendering(0.0), result: None, rx, cancel }
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

    /// "Rendering 40%" / "Uploading 80%", for status lines.
    pub fn label(&self) -> String {
        match self.stage {
            Stage::Rendering(f) => format!("Rendering share link {:.0}%", f * 100.0),
            Stage::Uploading(f) => format!("Uploading share link {:.0}%", f * 100.0),
        }
    }
}

const CANCELLED: &str = "cancelled";

/// One `share` request. Its 404 means the link is gone (not that the service is missing).
fn call(req: serde_json::Value) -> Result<serde_json::Value, String> {
    sf_license::call_backend_status("share", req).map_err(|(code, e)| if code == 404 && e == "not found" { "That link no longer exists.".into() } else { e })
}

/// Whether this computer's account may make share links (Pro, or staff on any license), as
/// the server last said, for the license generation it was asked about.
static ALLOWED: std::sync::Mutex<Option<(u64, bool)>> = std::sync::Mutex::new(None);
static ASKING: AtomicBool = AtomicBool::new(false);

/// May this computer make share links? A Pro key says yes offline; otherwise the server's last
/// answer for the current license (staff), asked for in the background when there is none yet.
pub fn allowed() -> bool {
    if sf_license::saved_status().is_pro() {
        return true;
    }
    let generation = sf_license::license_generation();
    match *ALLOWED.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((g, ok)) if g == generation => return ok,
        _ => {}
    }
    ask_allowed(generation);
    false
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
            None => *ALLOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, false)),
            Some(Some(ok)) => *ALLOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, ok)),
            // offline or a server error: asked again next time
            Some(None) => {}
        }
        ASKING.store(false, Ordering::SeqCst);
    });
}

fn run(project: &Path, title: &str, s: ShareSettings, cancel: &AtomicBool, stage: &dyn Fn(Stage)) -> Result<String, String> {
    let auth = sf_license::owner_auth().ok_or("Sign in to ScreenForge Pro on this computer to share links (Export → Sign in).")?;
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
    done["url"].as_str().map(String::from).ok_or_else(|| "The share server sent no link.".into())
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
    pub id: String,
    pub url: String,
    pub title: String,
    pub views: u64,
    pub created: String,
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
                        id: l["id"].as_str()?.into(),
                        url: l["url"].as_str()?.into(),
                        title: l["title"].as_str().unwrap_or("").into(),
                        views: l["views"].as_u64().unwrap_or(0),
                        created: l["created_at"].as_str().unwrap_or("").get(..10).unwrap_or("").into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Delete a link and its video for good.
pub fn delete(id: &str) -> Result<(), String> {
    let mut req = sf_license::owner_auth().ok_or("Sign in to delete share links.")?;
    req["action"] = "delete".into();
    req["id"] = id.into();
    match sf_license::call_backend_status("share", req) {
        // already gone: what the user wanted
        Ok(_) | Err((404, _)) => Ok(()),
        Err((_, e)) => Err(e),
    }
}

fn setting_file() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "screenforge", "ScreenForge").map(|d| d.config_dir().join("share-on-stop"))
}

/// Make a share link by itself whenever a recording stops (Pro). On unless turned off.
pub fn on_stop_enabled() -> bool {
    setting_file().and_then(|f| std::fs::read_to_string(f).ok()).is_none_or(|s| s.trim() != "off")
}

pub fn set_on_stop(on: bool) {
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
}
