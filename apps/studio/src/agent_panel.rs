//! Agent page of the sidebar: the user asks in plain words and Claude edits the open project.
//!
//! A turn runs on a thread with a copy of the project (`sf_agent::run_turn`). Each round of
//! edits it sends back is applied to the editor straight away, without undo entries, so the
//! timeline updates while the agent works; when the turn ends the whole of it becomes one undo
//! step. Editing by hand during a turn stops the agent and keeps the hand edit.
//!
//! Requests go through ScreenForge's backend, which holds the Claude key; a local build with
//! `ANTHROPIC_API_KEY` in its environment or `.env` calls Claude directly instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use egui::{vec2, CornerRadius, RichText, Stroke};
use serde_json::{json, Value};
use sf_agent::{Action, Conversation, Event, Host, Route, Settings, Usage, Workspace, MODELS};
use sf_project::Project;

use crate::editor::{fmt_ms, Editor};
use crate::theme;
use crate::widgets;

const SUGGESTIONS: [&str; 5] = ["Auto edit: tighten the whole video", "Remove the long pauses and filler words", "Speed up the parts where nothing happens to 2×", "Find a calm Unsplash photo of mountains for the background", "Use a dark gradient background with more padding"];

fn settings_path() -> Option<std::path::PathBuf> {
    sf_license::key_file().map(|p| p.with_file_name("agent.json"))
}

/// A developer's local key (`.env` or the environment), in debug builds only: released builds
/// always go through the ScreenForge backend, where the agent is part of Pro.
fn local_key() -> Option<String> {
    if cfg!(debug_assertions) {
        sf_agent::client::local_key()
    } else {
        None
    }
}

/// A local key when there is one (debug builds), else the ScreenForge backend.
fn route() -> Result<Route, String> {
    if let Some(api_key) = local_key() {
        return Ok(Route::Direct { api_key });
    }
    let device = sf_license::device_id().ok_or("ScreenForge cannot identify this computer, so the agent is not available.")?;
    Ok(Route::Hosted { url: format!("{}/functions/v1/agent", sf_license::SUPABASE_URL), anon_key: sf_license::SUPABASE_ANON_KEY.into(), device })
}

/// The editor's side of the agent: catalogs gathered on the UI thread when a turn starts, and
/// what needs files or the network (presets, Unsplash, imports), run on the agent's thread.
struct StudioHost {
    options: Value,
    presets: Vec<(String, PathBuf)>,
    project_dir: PathBuf,
    /// The screen video (source time = its time) and its size.
    media: PathBuf,
    media_size: (u32, u32),
    /// Results of the last Unsplash search, for `download_photo`.
    photos: Mutex<Vec<crate::unsplash::Photo>>,
    ctx: egui::Context,
}

impl StudioHost {
    fn new(ed: &Editor, ctx: &egui::Context) -> Self {
        let assets = &ed.session.assets;
        let presets: Vec<(String, PathBuf)> = sf_project::Preset::default_folder().map(|f| sf_project::Preset::list(&f)).unwrap_or_default().into_iter().map(|p| (p.name(), p.path.clone())).collect();
        let mut looks = assets.list_luts();
        if let Ok(files) = std::fs::read_dir(ed.session.pkg.path.join("assets")) {
            looks.extend(files.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".cube")));
        }
        let options = json!({
            "wallpapers": assets.list_backgrounds(),
            "music": assets.list_background_audio().iter().map(|n| json!({ "track": n, "label": sf_session::assets::background_audio_label(n) })).collect::<Vec<_>>(),
            "cursorSets": sf_session::cursor_sets::SETS.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
            "cameraLooks": looks,
            "presets": presets.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "recordedAudio": crate::audio::AudioPlayer::available_tracks(&ed.session),
            "hasCamera": !ed.session.source.camera.is_empty(),
            "hasTranscript": !ed.session.captions.is_empty(),
        });
        let media_size = (ed.session.source.native_width.max(1), ed.session.source.native_height.max(1));
        StudioHost { options, presets, project_dir: ed.session.pkg.path.clone(), media: ed.session.source.media_path.clone(), media_size, photos: Mutex::new(vec![]), ctx: ctx.clone() }
    }
}

/// Motion sections per screen video, measured once (decoding is the slow part).
static MOTION: Mutex<Vec<(PathBuf, Vec<(f64, f64, sf_agent::motion::Motion)>)>> = Mutex::new(Vec::new());

/// Frames the motion scan reads per second, and their width (wide enough that a typed
/// character changes a few pixels).
const MOTION_FPS: f64 = 2.0;
const MOTION_W: u32 = 640;

fn ffmpeg() -> Result<std::process::Command, String> {
    let path = sf_recording::ffmpeg::ffmpeg_path().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(path);
    sf_recording::ffmpeg::hide_console(&mut cmd);
    Ok(cmd)
}

impl Host for StudioHost {
    fn screen_motion(&self) -> Result<Vec<(f64, f64, sf_agent::motion::Motion)>, String> {
        if let Some((_, s)) = MOTION.lock().map_err(|_| "busy")?.iter().find(|(p, _)| *p == self.media) {
            return Ok(s.clone());
        }
        let h = ((MOTION_W as f64 * self.media_size.1 as f64 / self.media_size.0 as f64 / 2.0).round() as u32 * 2).max(2);
        let mut child = ffmpeg()?
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(&self.media)
            .args(["-vf", &format!("fps={MOTION_FPS},scale={MOTION_W}:{h},format=gray"), "-f", "rawvideo", "-"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not read the video: {e}"))?;
        // one frame at a time: a long video never sits in memory
        let mut stdout = child.stdout.take().ok_or("could not read the video")?;
        let size = (MOTION_W * h) as usize;
        let frames = std::iter::from_fn(|| {
            use std::io::Read;
            let mut f = vec![0u8; size];
            stdout.read_exact(&mut f).ok().map(|_| f)
        });
        let sections = sf_agent::motion::sections(frames, MOTION_W as usize, MOTION_FPS);
        let _ = child.wait();
        if sections.is_empty() {
            return Err("could not read the video".into());
        }
        MOTION.lock().map_err(|_| "busy")?.push((self.media.clone(), sections.clone()));
        Ok(sections)
    }

    fn screen_frames(&self, source_ms: &[f64]) -> Result<Vec<Vec<u8>>, String> {
        source_ms
            .iter()
            .map(|t| {
                let out = ffmpeg()?
                    .args(["-v", "error", "-nostdin", "-ss", &format!("{:.3}", t / 1000.0), "-i"])
                    .arg(&self.media)
                    .args(["-frames:v", "1", "-vf", "scale=768:-2", "-q:v", "5", "-f", "image2", "-c:v", "mjpeg", "pipe:1"])
                    .output()
                    .map_err(|e| format!("could not read the video: {e}"))?;
                if out.stdout.is_empty() {
                    return Err(format!("no frame at {:.1} s", t / 1000.0));
                }
                Ok(out.stdout)
            })
            .collect()
    }

    fn options(&self) -> Value {
        self.options.clone()
    }

    fn apply_preset(&self, name: &str, project: &mut Project) -> Result<(), String> {
        let (_, path) = self.presets.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).ok_or_else(|| format!("no preset named {name}"))?;
        let preset = sf_project::Preset::open(path).map_err(|e| e.to_string())?;
        preset.apply(project, &self.project_dir).map_err(|e| e.to_string())
    }

    fn search_photos(&self, query: &str) -> Result<Value, String> {
        let photos = crate::unsplash::search_blocking(query)?;
        let list = photos.iter().map(|p| json!({ "id": p.id, "description": p.description, "author": p.author, "color": p.color })).collect();
        *self.photos.lock().map_err(|_| "busy")? = photos;
        Ok(Value::Array(list))
    }

    fn download_photo(&self, id: &str) -> Result<Value, String> {
        let photo = self.photos.lock().map_err(|_| "busy")?.iter().find(|p| p.id == id).cloned().ok_or("search_photos first: that id is not in the last results")?;
        let applied = crate::unsplash::download_photo(photo, &self.project_dir.join("assets"), &AtomicU32::new(0), &self.ctx)?;
        Ok(crate::unsplash::background_value(&applied))
    }

    fn import_file(&self, path: &str) -> Result<String, String> {
        let src = PathBuf::from(path.trim());
        let src = match src.strip_prefix("~") {
            Ok(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or(src.clone()),
            Err(_) => src,
        };
        if !src.is_file() {
            return Err(format!("no file at {}", src.display()));
        }
        let name = src.file_name().ok_or("not a file")?.to_string_lossy().into_owned();
        let dest = self.project_dir.join("assets").join(&name);
        std::fs::create_dir_all(dest.parent().expect("assets dir")).map_err(|e| e.to_string())?;
        std::fs::copy(&src, &dest).map_err(|e| format!("could not copy it into the project: {e}"))?;
        Ok(name)
    }
}

enum Entry {
    User(String),
    Assistant(String),
    Step { text: String, failed: bool },
    Error(String),
    Note(String),
}

enum Msg {
    Event(Event),
    Done(Conversation),
}

struct Job {
    rx: mpsc::Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// The project when the turn started: the undo step goes back to it.
    before: Project,
    /// The project as the agent last left it; anything else means it was edited by hand.
    applied: Project,
    /// Edited by hand during the turn: the agent was stopped and its later edits are dropped.
    overridden: bool,
}

#[derive(Default)]
pub struct AgentPanel {
    /// `None` until the panel is first used (read from disk then).
    settings: Option<Settings>,
    /// Calls Claude with a local key instead of the backend (worked out on first use).
    local: Option<bool>,
    conversation: Conversation,
    log: Vec<Entry>,
    input: String,
    job: Option<Job>,
    /// Tokens and cost of this conversation.
    usage: Usage,
    /// Focus the message field on the next frame (⌘L).
    pub focus_input: bool,
    /// Height of the message card last frame, so the log above it leaves room.
    composer_h: f32,
    /// Credit balance and what can be bought (the backend's `credits` summary); hosted only.
    credits: Option<Value>,
    credits_rx: Option<mpsc::Receiver<Result<Value, String>>>,
    /// Email for checkout when the computer has no license email saved.
    email: String,
    /// A checkout being opened: its URL, or why it failed.
    checkout_rx: Option<mpsc::Receiver<Result<String, String>>>,
}

/// Same edit state, ignoring `data` (the autosave writes the playhead position there).
fn same_edit(a: &Project, b: &Project) -> bool {
    a.name == b.name && a.config == b.config && a.scenes == b.scenes
}

/// Playhead and selection, so "here" and "this clip" mean something to the agent.
fn editor_context(ed: &Editor) -> String {
    let tl = &ed.session.timeline;
    let mut s = format!("Playhead at {} ({} ms) of {}.", fmt_ms(ed.playhead_ms), ed.playhead_ms.round(), fmt_ms(ed.duration_ms()));
    let selected = if let Some(span) = ed.selected_slice.and_then(|i| tl.spans().get(i)) {
        format!("clip {} ({} – {})", span.index, fmt_ms(span.output_start_ms), fmt_ms(span.output_end_ms))
    } else if let Some(id) = &ed.selected_zoom {
        format!("zoom {id}")
    } else if let Some(id) = &ed.selected_mask {
        format!("mask {id}")
    } else if let Some(id) = &ed.selected_voice_over {
        format!("voice-over {id}")
    } else if let Some(id) = ed.selected_audio.as_ref().or(ed.selected_music.as_ref()) {
        format!("audio clip {id}")
    } else {
        "nothing".into()
    };
    s.push_str(&format!(" Selected: {selected}."));
    s
}

/// Short balance for the header chip ("820 credits").
fn credits_label(n: f64) -> String {
    let n = n.max(0.0).floor() as u64;
    match n {
        1 => "1 credit".into(),
        _ if n >= 10_000 => format!("{}K credits", n / 1000),
        _ => format!("{n} credits"),
    }
}

impl AgentPanel {
    /// Ask the backend for this computer's balance (hosted only).
    fn fetch_credits(&mut self) {
        if self.local == Some(true) || self.credits_rx.is_some() {
            return;
        }
        let Some(device) = sf_license::device_id() else { return };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(sf_license::call_backend("credits", json!({ "device": device })));
        });
        self.credits_rx = Some(rx);
    }

    /// Open Stripe Checkout for `plan` (`pro`, `topup_10` …) in the browser.
    fn buy(&mut self, plan: &str) {
        let email = sf_license::load_email().unwrap_or_else(|| self.email.trim().to_string());
        if !email.contains('@') {
            self.log.push(Entry::Error("Enter your email in the credits menu first.".into()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let plan = plan.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(sf_license::checkout_url(&email, &plan));
        });
        self.checkout_rx = Some(rx);
    }

    /// Credits and checkout replies that arrived since the last frame.
    fn poll_billing(&mut self) {
        if let Some(rx) = &self.credits_rx {
            match rx.try_recv() {
                Ok(Ok(v)) => {
                    self.credits = Some(v);
                    self.credits_rx = None;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => self.credits_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.checkout_rx {
            match rx.try_recv() {
                Ok(Ok(url)) => {
                    crate::os::open(&url);
                    self.log.push(Entry::Note("Opened checkout in your browser. Credits arrive here a moment after you pay.".into()));
                    self.checkout_rx = None;
                }
                Ok(Err(e)) => {
                    self.log.push(Entry::Error(e));
                    self.checkout_rx = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.checkout_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    /// Plan, balance, Pro and the top-up packs.
    fn credits_menu(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        let Some(c) = self.credits.clone() else { return };
        let num = |k: &str| c[k].as_f64().unwrap_or(0.0);
        let licensed = c["account"].is_string();
        let allowed = c["allowed"].as_bool() == Some(true);
        ui.set_width(260.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let status = if c["pro"].as_bool() == Some(true) {
            format!("Pro · {} this month · {} top-up", credits_label(num("monthly")), credits_label(num("topup")))
        } else if licensed {
            format!("{} left", credits_label(num("credits")))
        } else {
            format!("Free trial · {} left", credits_label(num("trial")))
        };
        ui.label(RichText::new(status).size(12.5).color(pal.text_dim));
        ui.label(RichText::new("1 credit is about one cent of Claude usage.").size(11.5).color(pal.text_faint));
        ui.separator();
        if sf_license::load_email().is_none() {
            ui.label(RichText::new("Email for checkout").size(12.5).color(pal.text_dim));
            widgets::text_field(ui, &mut self.email, "you@example.com");
        }
        if c["pro"].as_bool() != Some(true) {
            let monthly = num("monthlyCredits") as u64;
            if model_row(ui, "Upgrade to Pro · $20/month", &format!("{monthly} credits a month, and export"), false).clicked() {
                self.buy("pro");
                ui.close();
            }
        }
        if allowed && licensed {
            for p in c["packs"].as_array().cloned().unwrap_or_default() {
                let (plan, credits, usd) = (p["plan"].as_str().unwrap_or(""), p["credits"].as_u64().unwrap_or(0), p["usd"].as_u64().unwrap_or(0));
                if model_row(ui, &format!("{credits} credits · ${usd}"), "Top-up, never expires", false).clicked() {
                    self.buy(plan);
                    ui.close();
                }
            }
        } else {
            ui.label(RichText::new("Credit packs top up Pro.").size(11.5).color(pal.text_faint));
        }
    }

    /// The agent is part of Pro: shown instead of the chat on a computer without it.
    fn locked_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        let monthly = self.credits.as_ref().and_then(|c| c["monthlyCredits"].as_u64()).unwrap_or(1000);
        ui.add_space(16.0);
        egui::Frame::NONE.fill(pal.card_2).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("The AI agent is part of Pro").size(16.0).color(pal.text));
            ui.add_space(6.0);
            for line in ["Ask for edits in plain words, or Auto edit the whole video".to_string(), format!("{monthly} agent credits every month, top-ups anytime"), "Unlimited export".to_string()] {
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(vec2(10.0, 16.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 2.5, pal.accent_hover);
                    ui.add(egui::Label::new(RichText::new(line).size(13.0).color(pal.text_dim)).wrap());
                });
            }
            ui.add_space(10.0);
            if sf_license::load_email().is_none() {
                ui.label(RichText::new("Email for checkout").size(12.5).color(pal.text_dim));
                widgets::text_field(ui, &mut self.email, "you@example.com");
                ui.add_space(6.0);
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::accent_button(ui, "Upgrade to Pro · $20/month").clicked() {
                    self.buy("pro");
                }
                if widgets::secondary_button(ui, "Check again").on_hover_text("After subscribing and activating Pro on this computer").clicked() {
                    self.credits = None;
                    self.fetch_credits();
                }
            });
            ui.add_space(8.0);
            widgets::hint(ui, "Already on Pro? Open Export, enter the email you subscribed with and press Activate.");
        });
        // checkout notes and errors from the buttons above
        for e in self.log.iter().rev().take(1) {
            match e {
                Entry::Error(t) => {
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(t).size(13.0).color(pal.danger)).wrap());
                }
                Entry::Note(t) => {
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(t).size(13.0).color(pal.text_dim)).wrap());
                }
                _ => {}
            }
        }
    }

    fn settings(&mut self) -> &mut Settings {
        self.settings.get_or_insert_with(|| settings_path().map(|p| Settings::load(&p)).unwrap_or_default())
    }

    fn save_settings(&mut self) -> Result<(), String> {
        let s = self.settings().clone();
        let path = settings_path().ok_or("no home folder")?;
        s.save(&path).map_err(|e| e.to_string())
    }

    /// Start a turn with `text` (from the field or a suggestion).
    fn send(&mut self, ed: &mut Editor, ctx: &egui::Context, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || self.job.is_some() {
            return;
        }
        let settings = self.settings().clone();
        let route = match route() {
            Ok(r) => r,
            Err(e) => {
                self.log.push(Entry::Error(e));
                return;
            }
        };
        self.input.clear();
        self.log.push(Entry::User(text.clone()));
        let workspace = Workspace {
            project: ed.session.pkg.project.clone(),
            words: ed.session.captions.lines.iter().flat_map(|l| l.words.iter().cloned()).collect(),
            auto_zooms: ed.session.auto_zooms(),
            activity: {
                let t = &ed.session.cursor_track;
                let clicks: Vec<f64> = t.clicks.iter().filter(|c| c.down).map(|c| c.time_ms).collect();
                let moves: Vec<f64> = t.samples.iter().map(|s| s.time_ms).collect();
                (!t.key_downs.is_empty() || !clicks.is_empty() || !moves.is_empty()).then(|| sf_agent::activity::Activity::from_events(&t.key_downs, &clicks, &moves, ed.session.source.duration_ms))
            },
            seek_to: None,
            content_px: crate::masks::content_px(ed),
            audio_tracks: crate::audio::AudioPlayer::available_tracks(&ed.session),
            host: Some(Arc::new(StudioHost::new(ed, ctx))),
            actions: vec![],
        };
        let context = editor_context(ed);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (stop, conversation, ctx) = (cancel.clone(), self.conversation.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut ws = workspace;
            let conv = sf_agent::run_turn(&route, &settings, conversation, &mut ws, &text, &context, &stop, |e| {
                let _ = tx.send(Msg::Event(e));
                ctx.request_repaint();
            });
            let _ = tx.send(Msg::Done(conv));
            ctx.request_repaint();
        });
        let before = ed.session.pkg.project.clone();
        self.job = Some(Job { rx, cancel, applied: before.clone(), before, overridden: false });
    }

    /// Apply what the running turn sent since the last frame. Called every frame, whichever
    /// sidebar page is open.
    pub fn poll(&mut self, ed: &mut Editor) {
        let Some(job) = self.job.as_mut() else { return };
        let mut done = None;
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Event(Event::Changed(p))) => {
                    if job.overridden {
                        continue;
                    }
                    if !same_edit(&ed.session.pkg.project, &job.applied) {
                        job.overridden = true;
                        job.cancel.store(true, Ordering::Relaxed);
                        self.log.push(Entry::Note("Stopped: the project was edited by hand. Your edit is kept.".into()));
                        continue;
                    }
                    ed.apply_agent_project(*p);
                    job.applied = ed.session.pkg.project.clone();
                }
                Ok(Msg::Event(Event::Seek(t))) => ed.seek(t),
                Ok(Msg::Event(Event::Model(m))) => self.log.push(Entry::Step { text: format!("Auto: {}", sf_agent::client::label(m).trim_start_matches("Claude ")), failed: false }),
                Ok(Msg::Event(Event::Action(Action::OpenExport))) => ed.export.open = true,
                Ok(Msg::Event(Event::Action(Action::Transcribe { language }))) => {
                    if !ed.transcribe.is_running() {
                        if let Some(l) = language {
                            ed.transcribe.language = l;
                        }
                        ed.save_if_dirty();
                        ed.transcribe.start(ed.path.clone());
                    }
                }
                Ok(Msg::Event(Event::Text(t))) => self.log.push(Entry::Assistant(t)),
                Ok(Msg::Event(Event::Step { summary, failed })) => self.log.push(Entry::Step { text: summary, failed }),
                Ok(Msg::Event(Event::Usage(u))) => self.usage.add(u),
                Ok(Msg::Event(Event::Credits(c))) => self.credits = Some(c),
                Ok(Msg::Event(Event::Error(e))) => self.log.push(Entry::Error(e)),
                Ok(Msg::Done(conv)) => {
                    done = Some(Some(conv));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    done = Some(None);
                    break;
                }
            }
        }
        let Some(conv) = done else { return };
        let job = self.job.take().expect("job");
        self.fetch_credits();
        match conv {
            Some(c) => self.conversation = c,
            None => self.log.push(Entry::Error("The agent stopped unexpectedly.".into())),
        }
        let now = &ed.session.pkg.project;
        if !job.overridden && !same_edit(&job.before, now) {
            let (b, n) = (&job.before.scenes[0], &now.scenes[0]);
            let audio_changed = job.before.config.audio != now.config.audio || b.voice_overs != n.voice_overs || b.extra != n.extra;
            ed.commit_live_edit(job.before);
            if audio_changed {
                ed.reload_audio();
            }
            ed.set_status("Agent edits applied · ⌘Z undoes them");
        }
    }

    pub fn ui(&mut self, ed: &mut Editor, ui: &mut egui::Ui) {
        let pal = theme::p();
        self.poll_billing();
        if self.credits.is_none() && self.local == Some(false) {
            self.fetch_credits();
        }
        if self.credits_rx.is_some() || self.checkout_rx.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
            widgets::paint_icon(ui.painter(), mark, widgets::Icon::Agent, pal.text);
            ui.label(RichText::new("Agent").size(16.0).color(pal.text));
            let local = *self.local.get_or_insert_with(|| local_key().is_some());
            if local {
                ui.label(RichText::new("local key").size(11.0).color(pal.text_faint)).on_hover_text("ANTHROPIC_API_KEY from the environment or .env: requests go straight to Claude, not through the ScreenForge server.");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_reset = !self.log.is_empty() && self.job.is_none();
                if ui.add_enabled_ui(can_reset, |ui| widgets::icon_button(ui, widgets::Icon::Restart, 16.0, "Clear chat")).inner.clicked() {
                    self.conversation = Conversation::default();
                    self.log.clear();
                    self.usage = Usage::default();
                }
                if let Some(n) = self.credits.as_ref().filter(|c| c["allowed"].as_bool() == Some(true)).and_then(|c| c["credits"].as_f64()).filter(|_| !local) {
                    let colour = if n <= 0.0 { pal.danger } else { pal.text_dim };
                    let chip = ui.add(egui::Button::new(RichText::new(credits_label(n)).size(12.0).color(colour)).fill(pal.card_2).corner_radius(CornerRadius::same(widgets::BUTTON_RADIUS))).on_hover_text("Agent credits: plan, Pro and top-ups");
                    let frame = egui::Frame::menu(ui.style()).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8));
                    egui::Popup::menu(&chip).frame(frame).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| self.credits_menu(ui));
                }
            });
        });

        // hosted and not on Pro: no chat, the upgrade card instead
        if self.local == Some(false) && self.credits.as_ref().is_some_and(|c| c["allowed"].as_bool() == Some(false)) {
            self.locked_ui(ui);
            return;
        }

        // the composer's height from the last frame (it grows with the text)
        let composer_h = if self.composer_h > 0.0 { self.composer_h } else { 110.0 } + 8.0;
        let log_h = (ui.available_height() - composer_h).max(80.0);
        let mut suggestion = None;
        egui::ScrollArea::vertical().id_salt("agent_log").max_height(log_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            if self.log.is_empty() {
                widgets::hint(ui, "Ask for an edit in your own words. The agent reads the timeline and the transcript, makes the change, and you see it on the timeline as it works. ⌘Z undoes a whole answer.");
                ui.add_space(4.0);
                for s in SUGGESTIONS {
                    let b = egui::Button::new(RichText::new(s).size(13.0).color(pal.text_dim)).fill(pal.card_2).corner_radius(CornerRadius::same(widgets::BUTTON_RADIUS)).wrap();
                    if ui.add(b).clicked() {
                        suggestion = Some(s.to_string());
                    }
                }
            }
            for e in &self.log {
                match e {
                    Entry::User(t) => {
                        egui::Frame::NONE.fill(pal.card_3).corner_radius(CornerRadius::same(widgets::BUTTON_RADIUS)).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(RichText::new(t).size(14.0).color(pal.text)).wrap());
                        });
                    }
                    Entry::Assistant(t) => {
                        ui.add(egui::Label::new(RichText::new(t).size(14.0).color(pal.text)).wrap());
                    }
                    Entry::Step { text, failed } => {
                        ui.horizontal_wrapped(|ui| {
                            let colour = if *failed { pal.danger } else { pal.text_faint };
                            let (rect, _) = ui.allocate_exact_size(vec2(10.0, 14.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 2.5, if *failed { pal.danger } else { pal.accent_hover });
                            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(colour)).wrap());
                        });
                    }
                    Entry::Error(t) => {
                        ui.add(egui::Label::new(RichText::new(t).size(13.0).color(pal.danger)).wrap());
                    }
                    Entry::Note(t) => {
                        ui.add(egui::Label::new(RichText::new(t).size(13.0).color(pal.text_dim)).wrap());
                    }
                }
            }
            let out = self.job.is_none() && self.credits.as_ref().and_then(|c| c["credits"].as_f64()).is_some_and(|n| n <= 0.0);
            if out && self.local != Some(true) {
                let licensed = self.credits.as_ref().is_some_and(|c| c["account"].is_string() && c["allowed"].as_bool() == Some(true));
                let pro = self.credits.as_ref().is_some_and(|c| c["pro"].as_bool() == Some(true));
                ui.horizontal_wrapped(|ui| {
                    if !pro && widgets::accent_button(ui, "Upgrade to Pro · $20/month").clicked() {
                        self.buy("pro");
                    }
                    if licensed && widgets::secondary_button(ui, "500 credits · $10").clicked() {
                        self.buy("topup_10");
                    }
                });
            }
            if let Some(job) = &self.job {
                ui.horizontal(|ui| {
                    working_dots(ui);
                    if job.cancel.load(Ordering::Relaxed) {
                        ui.label(RichText::new("Stopping after this step").size(12.5).color(pal.text_faint));
                    }
                });
            }
        });
        if let Some(s) = suggestion {
            self.send(ed, ui.ctx(), s);
        }

        // ── composer: one card with the message and a footer (model, send), as in Cursor ──
        ui.add_space(8.0);
        let top = ui.cursor().top();
        let id = egui::Id::new("agent_input");
        let focused = ui.memory(|m| m.has_focus(id));
        // Enter sends, Shift+Enter starts a new line
        let mut send = focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let hint = if self.job.is_some() { "The agent is working… (you can type the next request)" } else { "Ask for an edit, e.g. cut the first 5 seconds" };
        egui::Frame::NONE
            .fill(pal.card_2)
            .stroke(Stroke::new(1.0, if focused { pal.text_faint } else { pal.border }))
            .corner_radius(CornerRadius::same(theme::RADIUS_SM))
            .inner_margin(egui::Margin { left: 12, right: 8, top: 10, bottom: 8 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // grows with the text, then scrolls
                egui::ScrollArea::vertical().id_salt("agent_input_scroll").max_height(180.0).show(ui, |ui| {
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .id(id)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .font(egui::FontId::proportional(14.0))
                            .text_color(pal.text)
                            .hint_text(RichText::new(hint).color(pal.text_faint)),
                    );
                    if self.focus_input {
                        resp.request_focus();
                        self.focus_input = false;
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    self.model_menu(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &self.job {
                        Some(job) => {
                            if square_button(ui, widgets::Icon::Stop, true, "Stop (the step in progress finishes first)").clicked() {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        }
                        None => {
                            let ok = !self.input.trim().is_empty();
                            if square_button(ui, widgets::Icon::ArrowUp, ok, "Send (Enter) · Shift+Enter for a new line").clicked() && ok {
                                send = true;
                            }
                        }
                    });
                });
            });
        if self.usage.input_tokens + self.usage.output_tokens > 0 {
            let tokens = self.usage.input_tokens + self.usage.output_tokens + self.usage.cache_read_tokens + self.usage.cache_write_tokens;
            let tip = format!("{} input, {} output, {} cached tokens, at list price", self.usage.input_tokens + self.usage.cache_write_tokens, self.usage.output_tokens, self.usage.cache_read_tokens);
            // a small caption under the card, right-aligned like the composer's send button
            ui.add_space(5.0);
            // one line tall: a bare right-to-left layout would take all the height left
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{} tokens · {} this chat", compact(tokens), dollars(self.usage.cost_usd))).size(11.0).color(pal.text_faint)).on_hover_text(tip);
            });
            ui.add_space(2.0);
        }
        self.composer_h = ui.cursor().top() - top;
        if send {
            let text = self.input.clone();
            self.send(ed, ui.ctx(), text);
        }
    }

    /// "Auto ⌄" chip in the composer's footer (secondary fill, one step lighter than the
    /// composer card) with the models in a menu.
    fn model_menu(&mut self, ui: &mut egui::Ui) {
        let pal = theme::p();
        let current = self.settings().choice().to_string();
        let label = if current == sf_agent::client::AUTO { "Auto".to_string() } else { sf_agent::client::label(&current).trim_start_matches("Claude ").to_string() };
        let font = egui::FontId::proportional(12.5);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.painter().layout_no_wrap(label.clone(), font.clone(), pal.text).size().x + 30.0, 28.0), egui::Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Model");
        let fg = if resp.hovered() || resp.is_pointer_button_down_on() { pal.text } else { pal.text_dim };
        let galley = ui.painter().layout_no_wrap(label, font, fg);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(widgets::BUTTON_RADIUS), pal.card_3);
        painter.galley(egui::pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0), galley, fg);
        widgets::paint_icon(painter, egui::Rect::from_center_size(egui::pos2(rect.right() - 11.0, rect.center().y), vec2(8.0, 8.0)), widgets::Icon::ChevronDown, fg);
        let mut chosen = None;
        let frame = egui::Frame::menu(ui.style()).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(4));
        egui::Popup::menu(&resp).frame(frame).show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let rows = [(sf_agent::client::AUTO, "Auto", "Sonnet, or Opus for big edits")].into_iter().chain(MODELS.iter().map(|m| {
                let hint = if m.id.contains("haiku") { "Quickest and cheapest" } else { "Fast, for everyday edits" };
                (m.id, m.label.trim_start_matches("Claude "), hint)
            }));
            for (id, name, hint) in rows {
                if model_row(ui, name, hint, id == current).clicked() {
                    chosen = Some(id);
                    ui.close();
                }
            }
        });
        if let Some(id) = chosen {
            self.settings().model = id.into();
            if let Err(e) = self.save_settings() {
                self.log.push(Entry::Error(format!("Could not save the model choice: {e}")));
            }
        }
    }
}

/// `38587` → `38.6K`, `1250000` → `1.3M`.
fn compact(n: u64) -> String {
    let (v, unit) = match n {
        0..=999 => return n.to_string(),
        1_000..=999_999 => (n as f64 / 1e3, "K"),
        _ => (n as f64 / 1e6, "M"),
    };
    let s = format!("{v:.1}");
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

/// `0.0412` → `$0.04`; less than a cent → `<$0.01`.
fn dollars(usd: f64) -> String {
    if usd < 0.005 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

/// Three dots that pulse one after another while the agent works.
fn working_dots(ui: &mut egui::Ui) {
    let pal = theme::p();
    let (rect, _) = ui.allocate_exact_size(vec2(30.0, 16.0), egui::Sense::hover());
    let t = ui.input(|i| i.time);
    for k in 0..3 {
        // each dot brightens and lifts a little, a third of a cycle after the one before
        let phase = ((t * 4.0 - k as f64 * 0.9).sin() * 0.5 + 0.5) as f32;
        let centre = egui::pos2(rect.left() + 5.0 + k as f32 * 9.0, rect.center().y - phase * 2.0);
        ui.painter().circle_filled(centre, 2.8, pal.text_faint.lerp_to_gamma(pal.text, phase));
    }
    ui.ctx().request_repaint();
}

/// A row of the model menu, like the presets menu's rows: name and a dim hint, a quiet fill on
/// hover, and a check on the chosen one.
fn model_row(ui: &mut egui::Ui, name: &str, hint: &str, selected: bool) -> egui::Response {
    const W: f32 = 236.0;
    let pal = theme::p();
    let (rect, resp) = ui.allocate_exact_size(vec2(W, 40.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(widgets::BUTTON_RADIUS), pal.card_2);
    }
    let x = rect.left() + 10.0;
    painter.text(egui::pos2(x, rect.top() + 12.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), pal.text);
    painter.text(egui::pos2(x, rect.top() + 28.0), egui::Align2::LEFT_CENTER, hint, egui::FontId::proportional(11.5), pal.text_faint);
    if selected {
        widgets::paint_icon(painter, egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(12.0, 12.0)), widgets::Icon::Check, pal.accent_hover);
    }
    resp
}

/// Square icon button in the composer: accent for Send (when there is text) and Stop, quiet
/// when there is nothing to send.
fn square_button(ui: &mut egui::Ui, icon: widgets::Icon, primary: bool, tip: &str) -> egui::Response {
    let pal = theme::p();
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
    let (fill, fg) = match (primary, resp.hovered()) {
        (true, true) => (pal.accent_hover, pal.clip_text),
        (true, false) => (pal.accent, pal.clip_text),
        (false, true) => (pal.card_3, pal.text),
        (false, false) => (pal.card_3, pal.text_dim),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(widgets::BUTTON_RADIUS), fill);
    // the stop square fills less of its box than the arrow: draw it larger so both read the same
    let size = if icon == widgets::Icon::Stop { 18.0 } else { 14.0 };
    widgets::paint_icon(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(size, size)), icon, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::{compact, dollars};

    #[test]
    fn usage_reads_short() {
        assert_eq!(compact(812), "812");
        assert_eq!(compact(38_587), "38.6K");
        assert_eq!(compact(2_000), "2K");
        assert_eq!(compact(1_270_000), "1.3M");
        assert_eq!(dollars(0.0412), "$0.04");
        assert_eq!(dollars(0.001), "<$0.01");
    }
}
