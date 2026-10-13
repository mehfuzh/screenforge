//! Wayland: the ScreenCast portal (xdg-desktop-portal) hands out a PipeWire stream of the
//! monitor the user shares; frames are copied into a [`FrameStore`] and the shared
//! [`Repeater`] feeds ffmpeg, cropped and scaled to the planned region. The cursor is left out
//! of the video and arrives as per-buffer metadata instead, which drives the pointer log
//! (Wayland gives applications no global pointer position otherwise).
//!
//! The portal remembers the shared monitor (restore token in
//! `$XDG_STATE_HOME/screenforge/screencast-token`), so its dialog only appears the first time
//! or when the recording area moves to a different monitor.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use crossbeam_channel::{bounded, unbounded, Sender};
use parking_lot::Mutex;
use pipewire as pw;
use pw::spa;
use pw::spa::pod::{Object, Pod, Property, PropertyFlags, Value};
use sf_project::schema::common::Rect;

use super::display::{target_bitrate, DisplayPlan};
use crate::config::RecordingConfig;
use crate::log::RecLog;
use crate::repeater::{Cmd, FrameStore, RawInput, Repeater};
use crate::{Error, Result};

const COMPONENT: &str = "DisplayRecorder";

/// Receives cursor positions in global points.
pub type PointerSink = Box<dyn Fn(f64, f64) + Send>;

/// True in a Wayland session (where x11grab only sees XWayland windows).
pub fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").map(|t| t.eq_ignore_ascii_case("wayland")).unwrap_or(false) || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn token_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("screenforge/screencast-token"))
}

fn load_token() -> Option<String> {
    let t = std::fs::read_to_string(token_path()?).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn save_token(token: Option<&str>) {
    let Some(p) = token_path() else { return };
    match token {
        Some(t) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, t);
        }
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// A started portal session and the stream it shares.
struct Cast {
    proxy: Screencast,
    session: Session<Screencast>,
    fd: OwnedFd,
    node_id: u32,
    /// Shared monitor in global logical points.
    rect: Rect,
    cursor_metadata: bool,
}

fn portal_err(what: &str, e: ashpd::Error) -> Error {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => Error::Capture("Screen sharing was cancelled.".into()),
        e => Error::Capture(format!("screen-cast portal ({what}): {e}")),
    }
}

async fn open_cast(token: Option<&str>, log: &RecLog) -> Result<Cast> {
    let proxy = Screencast::new().await.map_err(|e| portal_err("connect", e))?;
    let modes = proxy.available_cursor_modes().await.unwrap_or_default();
    // Metadata keeps the cursor out of the video (the editor draws it) and reports its
    // position; Hidden still keeps it out. Embedded would put a second cursor on screen.
    let (mode, cursor_metadata) = if modes.contains(CursorMode::Metadata) {
        (CursorMode::Metadata, true)
    } else if modes.contains(CursorMode::Hidden) {
        (CursorMode::Hidden, false)
    } else {
        (CursorMode::Embedded, false)
    };
    log.notice(COMPONENT, format!("ScreenCast portal v{}: cursorModes={modes:?} using={mode:?} restoreToken={}", proxy.version(), token.is_some()));
    let session = proxy.create_session(Default::default()).await.map_err(|e| portal_err("create session", e))?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(mode)
                .set_sources(enumflags2::BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token),
        )
        .await
        .map_err(|e| portal_err("select sources", e))?
        .response()
        .map_err(|e| portal_err("select sources", e))?;
    let streams = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e| portal_err("start", e))?
        .response()
        .map_err(|e| portal_err("start", e))?;
    save_token(streams.restore_token());
    let stream = streams.streams().first().ok_or_else(|| Error::Capture("the portal shared no stream".into()))?;
    let (x, y) = stream.position().unwrap_or((0, 0));
    let (w, h) = stream.size().ok_or_else(|| Error::Capture("the portal did not report the stream size".into()))?;
    let rect = Rect { x: x as f64, y: y as f64, width: w as f64, height: h as f64 };
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| portal_err("open PipeWire remote", e))?;
    log.notice(COMPONENT, format!("Sharing node {node_id} at {rect:?}"));
    Ok(Cast { proxy, session, fd, node_id, rect, cursor_metadata })
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    const EPS: f64 = 0.5;
    inner.x >= outer.x - EPS
        && inner.y >= outer.y - EPS
        && inner.x + inner.width <= outer.x + outer.width + EPS
        && inner.y + inner.height <= outer.y + outer.height + EPS
}

/// Video format PipeWire settled on.
#[derive(Debug, Clone, Copy)]
struct Negotiated {
    pix_fmt: &'static str,
    width: u32,
    height: u32,
}

fn pix_fmt(f: spa::param::video::VideoFormat) -> Option<&'static str> {
    use spa::param::video::VideoFormat as F;
    Some(match f {
        F::BGRx => "bgr0",
        F::BGRA => "bgra",
        F::RGBx => "rgb0",
        F::RGBA => "rgba",
        F::xRGB => "0rgb",
        F::xBGR => "0bgr",
        F::ARGB => "argb",
        F::ABGR => "abgr",
        _ => return None,
    })
}

fn serialize(obj: Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .expect("pod serialization")
        .0
        .into_inner()
}

fn enum_format_pod(fps: u32) -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::utils::{Fraction, Rectangle};
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::xRGB,
            VideoFormat::xBGR,
            VideoFormat::ARGB,
            VideoFormat::ABGR
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    serialize(obj)
}

/// Mappable buffers only (no DMA-BUF: we copy frames on the CPU), plus room for the cursor
/// metadata with a 64×64 bitmap.
fn buffer_and_meta_pods() -> [Vec<u8>; 2] {
    use spa::sys;
    let types = (1 << sys::SPA_DATA_MemPtr) | (1 << sys::SPA_DATA_MemFd);
    let buffers = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamBuffers,
        id: sys::SPA_PARAM_Buffers,
        properties: vec![Property { key: sys::SPA_PARAM_BUFFERS_dataType, flags: PropertyFlags::empty(), value: Value::Int(types) }],
    };
    let cursor_size = std::mem::size_of::<sys::spa_meta_cursor>() + std::mem::size_of::<sys::spa_meta_bitmap>() + 64 * 64 * 4;
    let meta = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamMeta,
        id: sys::SPA_PARAM_Meta,
        properties: vec![
            Property { key: sys::SPA_PARAM_META_type, flags: PropertyFlags::empty(), value: Value::Id(spa::utils::Id(sys::SPA_META_Cursor)) },
            Property { key: sys::SPA_PARAM_META_size, flags: PropertyFlags::empty(), value: Value::Int(cursor_size as i32) },
        ],
    };
    [serialize(buffers), serialize(meta)]
}

/// State shared between the PipeWire thread and the recorder.
struct Shared {
    store: Arc<FrameStore>,
    negotiated: OnceLock<Negotiated>,
    /// Shared monitor in global points; maps stream pixels to points for the cursor.
    rect: Rect,
    pointer: Mutex<Option<PointerSink>>,
    error: Mutex<Option<String>>,
    log: RecLog,
}

impl Shared {
    fn fail(&self, msg: String) {
        self.log.error(COMPONENT, msg.clone());
        self.error.lock().get_or_insert(msg);
    }
}

/// Copy one mapped buffer into the store as tightly packed rows.
fn take_frame(buffer: &mut pw::buffer::Buffer, n: Negotiated, store: &FrameStore) {
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else { return };
    let (offset, size, stride) = {
        let c = d.chunk();
        (c.offset() as usize, c.size() as usize, c.stride().unsigned_abs() as usize)
    };
    if size == 0 {
        return; // cursor-only update
    }
    let row = n.width as usize * 4;
    let stride = if stride == 0 { row } else { stride };
    let h = n.height as usize;
    let Some(src) = d.data() else { return };
    if stride < row || offset + stride * (h - 1) + row > src.len() {
        return;
    }
    let src = &src[offset..];
    let mut buf = Vec::with_capacity(row * h);
    for y in 0..h {
        buf.extend_from_slice(&src[y * stride..y * stride + row]);
    }
    store.put(buf);
}

/// PipeWire thread: connect to the portal's remote, negotiate, copy frames until told to quit.
fn run_pipewire(fd: OwnedFd, node_id: u32, fps: u32, shared: Arc<Shared>, quit_rx: pw::channel::Receiver<()>) {
    let r = (|| -> std::result::Result<(), pw::Error> {
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_fd_rc(fd, None)?;
        let ml = mainloop.clone();
        let _quit = quit_rx.attach(mainloop.loop_(), move |_| ml.quit());
        let stream = pw::stream::StreamBox::new(
            &core,
            "screenforge-capture",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;
        let s_param = shared.clone();
        let s_proc = shared.clone();
        let s_state = shared.clone();
        let last_ptr = std::cell::Cell::new((f64::NAN, f64::NAN));
        let _listener = stream
            .add_local_listener_with_user_data(spa::param::video::VideoInfoRaw::default())
            .state_changed(move |_, _, old, new| {
                s_state.log.debug(COMPONENT, format!("PipeWire stream {old:?} → {new:?}"));
                if let pw::stream::StreamState::Error(e) = new {
                    s_state.fail(format!("PipeWire stream error: {e}"));
                }
            })
            .param_changed(move |stream, info, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                if info.parse(param).is_err() {
                    return;
                }
                let size = info.size();
                let Some(fmt) = pix_fmt(info.format()) else {
                    s_param.fail(format!("unsupported screen-cast pixel format {:?}", info.format()));
                    return;
                };
                let n = Negotiated { pix_fmt: fmt, width: size.width, height: size.height };
                match s_param.negotiated.get() {
                    None => {
                        let _ = s_param.negotiated.set(n);
                        s_param.log.notice(COMPONENT, format!("Stream format {fmt} {}x{}", size.width, size.height));
                    }
                    Some(first) if (first.width, first.height) != (n.width, n.height) => {
                        s_param.log.warning(COMPONENT, format!("Stream resized to {}x{}; frames of the new size are skipped", n.width, n.height));
                    }
                    Some(_) => {}
                }
                let pods = buffer_and_meta_pods();
                let mut params: Vec<&Pod> = pods.iter().filter_map(|b| Pod::from_bytes(b)).collect();
                if let Err(e) = stream.update_params(&mut params) {
                    s_param.log.warning(COMPONENT, format!("update_params: {e}"));
                }
            })
            .process(move |stream, info| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let Some(&first) = s_proc.negotiated.get() else { return };
                if let Some(c) = buffer.find_meta::<spa::buffer::meta::MetaCursor>() {
                    if c.is_valid() && first.width > 0 {
                        let p = c.position();
                        let r = &s_proc.rect;
                        let (sx, sy) = (first.width as f64 / r.width, first.height as f64 / r.height);
                        let pt = (r.x + p.x as f64 / sx, r.y + p.y as f64 / sy);
                        if pt != last_ptr.get() {
                            last_ptr.set(pt);
                            if let Some(sink) = s_proc.pointer.lock().as_ref() {
                                sink(pt.0, pt.1);
                            }
                        }
                    }
                }
                let size = info.size();
                if (size.width, size.height) == (first.width, first.height) {
                    take_frame(&mut buffer, first, &s_proc.store);
                }
            })
            .register()?;
        let fmt = enum_format_pod(fps);
        let mut params = [Pod::from_bytes(&fmt).expect("format pod")];
        stream.connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )?;
        mainloop.run();
        let _ = stream.disconnect();
        Ok(())
    })();
    if let Err(e) = r {
        shared.fail(format!("PipeWire: {e}"));
    }
}

/// Wayland display recorder (portal + PipeWire).
pub struct PortalRecorder {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    repeater: Option<JoinHandle<()>>,
    pw_quit: Option<pw::channel::Sender<()>>,
    pw_thread: Option<JoinHandle<()>>,
    cast: Option<(Screencast, Session<Screencast>)>,
    stopped: AtomicBool,
    pub cursor_metadata: bool,
}

impl PortalRecorder {
    pub fn start(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog) -> Result<Self> {
        // a remembered monitor that no longer contains the area: ask again
        let token = load_token();
        let mut cast = pollster::block_on(open_cast(token.as_deref(), &log))?;
        if !contains(&cast.rect, &plan.bounds) && token.is_some() {
            log.notice(COMPONENT, "Remembered monitor does not contain the recording area; asking again");
            let _ = pollster::block_on(cast.session.close());
            save_token(None);
            cast = pollster::block_on(open_cast(None, &log))?;
        }
        if !contains(&cast.rect, &plan.bounds) {
            let _ = pollster::block_on(cast.session.close());
            return Err(Error::Capture(
                "The shared screen does not contain the recording area. Start again and pick the screen you are recording in the sharing dialog."
                    .into(),
            ));
        }
        let Cast { proxy, session, fd, node_id, rect, cursor_metadata } = cast;

        let fps = cfg.fps.max(1);
        let shared = Arc::new(Shared {
            store: Arc::new(FrameStore::new(0, 0)),
            negotiated: OnceLock::new(),
            rect,
            pointer: Mutex::new(None),
            error: Mutex::new(None),
            log: log.clone(),
        });
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let pw_shared = shared.clone();
        let pw_thread = std::thread::Builder::new()
            .name("sf-pipewire".into())
            .spawn(move || run_pipewire(fd, node_id, fps, pw_shared, quit_rx))
            .map_err(|source| Error::Io { path: "thread".into(), source })?;

        let mut rec = PortalRecorder {
            shared: shared.clone(),
            tx: unbounded().0,
            repeater: None,
            pw_quit: Some(quit_tx),
            pw_thread: Some(pw_thread),
            cast: Some((proxy, session)),
            stopped: AtomicBool::new(false),
            cursor_metadata,
        };
        // Wait for the first frame so a session never starts without content.
        if !shared.store.wait_first(Duration::from_secs(5)) {
            let why = shared.error.lock().clone().unwrap_or_else(|| "no frames received from the screen-cast stream".into());
            rec.stop();
            return Err(Error::Capture(why));
        }
        let n = *shared.negotiated.get().expect("negotiated before the first frame");

        // region of the shared monitor, in stream pixels
        let (sx, sy) = (n.width as f64 / rect.width, n.height as f64 / rect.height);
        let cx = (((plan.bounds.x - rect.x) * sx).round().max(0.0) as u32) & !1;
        let cy = (((plan.bounds.y - rect.y) * sy).round().max(0.0) as u32) & !1;
        let cw = ((plan.bounds.width * sx).round() as u32).min(n.width - cx) & !1;
        let ch = ((plan.bounds.height * sy).round() as u32).min(n.height - cy) & !1;
        let mut filters = Vec::new();
        if (cx, cy, cw, ch) != (0, 0, n.width, n.height) {
            filters.push(format!("crop={cw}:{ch}:{cx}:{cy}"));
        }
        if (cw, ch) != (plan.width_px, plan.height_px) {
            filters.push(format!("scale={}:{}:flags=bicubic", plan.width_px, plan.height_px));
        }
        let bitrate = target_bitrate(plan.width_px, plan.height_px, cfg.video_quality);
        log.notice(
            COMPONENT,
            format!("Capture started. encoder=libx264 bitrate={bitrate} stream={}x{} filter={filters:?} cursorMetadata={cursor_metadata}", n.width, n.height),
        );
        let (tx, rx) = unbounded();
        let rep = Repeater {
            rx,
            store: shared.store.clone(),
            log,
            component: COMPONENT,
            fps,
            input: RawInput { pix_fmt: n.pix_fmt, width: n.width, height: n.height, filter: (!filters.is_empty()).then(|| filters.join(",")) },
            bitrate,
            encoder: "libx264",
        };
        rec.tx = tx;
        rec.repeater = Some(
            std::thread::Builder::new()
                .name("sf-frame-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?,
        );
        Ok(rec)
    }

    /// Where cursor positions from the stream metadata go (the input recorder on Wayland).
    pub fn set_pointer_sink(&self, sink: PointerSink) {
        *self.shared.pointer.lock() = Some(sink);
    }

    pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
        let (reply, rx) = bounded(1);
        self.tx.send(Cmd::Begin { index, dir: dir.into(), reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
        rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
    }

    pub fn end_session(&self) -> Result<u64> {
        let (reply, rx) = bounded(1);
        self.tx.send(Cmd::End { reply }).map_err(|_| Error::Capture("repeater stopped".into()))?;
        rx.recv().map_err(|_| Error::Capture("repeater stopped".into()))?
    }

    pub fn frames_received(&self) -> u64 {
        self.shared.store.received.load(Ordering::Relaxed)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().clone()
    }

    pub fn stop(&mut self) {
        if self.stopped.swap(true, Ordering::Relaxed) {
            return;
        }
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.repeater.take() {
            let _ = t.join();
        }
        if let Some(q) = self.pw_quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.pw_thread.take() {
            let _ = t.join();
        }
        if let Some((_proxy, session)) = self.cast.take() {
            let _ = pollster::block_on(session.close());
        }
    }
}

impl Drop for PortalRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_inside_shared_monitor() {
        let mon = Rect { x: 1920.0, y: 0.0, width: 2560.0, height: 1440.0 };
        assert!(contains(&mon, &Rect { x: 1920.0, y: 0.0, width: 2560.0, height: 1440.0 }));
        assert!(contains(&mon, &Rect { x: 2000.0, y: 100.0, width: 800.0, height: 600.0 }));
        assert!(!contains(&mon, &Rect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 }));
    }

    #[test]
    fn pods_serialize() {
        assert!(Pod::from_bytes(&enum_format_pod(60)).is_some());
        for p in buffer_and_meta_pods() {
            assert!(Pod::from_bytes(&p).is_some());
        }
    }
}
