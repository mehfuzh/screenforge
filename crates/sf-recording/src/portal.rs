//! Wayland: the ScreenCast portal (xdg-desktop-portal) hands out a PipeWire stream of the
//! monitor the user shares; frames are copied into a [`FrameStore`] and the shared
//! [`Repeater`] feeds ffmpeg, cropped and scaled to the planned region. The cursor is drawn
//! into the video: GNOME 50 negotiates cursor metadata but never fills in a position, so the
//! pointer log (and cursor-following zooms) would stay empty and the recording would have no
//! cursor at all. `SF_PORTAL_CURSOR_METADATA=1` asks for metadata instead (the cursor is then
//! left out of the video and its positions drive the pointer log) for portals that send it.
//!
//! The portal remembers the shared monitor (restore token in
//! `$XDG_STATE_HOME/screenforge/screencast-token`), so its dialog only appears the first time
//! or when the recording area moves to a different monitor. Window recordings ask every time
//! ([`pick_window`]): the window is chosen in the dialog and recorded as its own stream.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

async fn open_cast(source: SourceType, token: Option<&str>, log: &RecLog) -> Result<Cast> {
    let proxy = Screencast::new().await.map_err(|e| portal_err("connect", e))?;
    let modes = proxy.available_cursor_modes().await.unwrap_or_default();
    // Embedded: the cursor is part of the video (see the module docs). Metadata keeps it out
    // and reports its position for the editor to draw, when asked for.
    let want_metadata = std::env::var_os("SF_PORTAL_CURSOR_METADATA").is_some_and(|v| v == "1");
    let (mode, cursor_metadata) = if want_metadata && modes.contains(CursorMode::Metadata) {
        (CursorMode::Metadata, true)
    } else {
        (CursorMode::Embedded, false)
    };
    log.notice(COMPONENT, format!("ScreenCast portal v{}: source={source:?} cursorModes={modes:?} using={mode:?} restoreToken={}", proxy.version(), token.is_some()));
    // monitors are remembered; a window is picked afresh for each recording
    let persist = if source == SourceType::Monitor { PersistMode::ExplicitlyRevoked } else { PersistMode::DoNot };
    let session = proxy.create_session(Default::default()).await.map_err(|e| portal_err("create session", e))?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(mode)
                .set_sources(enumflags2::BitFlags::from(source))
                .set_multiple(false)
                .set_persist_mode(persist)
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
    if source == SourceType::Monitor {
        save_token(streams.restore_token());
    }
    let stream = streams.streams().first().ok_or_else(|| Error::Capture("the portal shared no stream".into()))?;
    let (x, y) = stream.position().unwrap_or((0, 0));
    let (w, h) = stream.size().ok_or_else(|| Error::Capture("the portal did not report the stream size".into()))?;
    let rect = Rect { x: x as f64, y: y as f64, width: w as f64, height: h as f64 };
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| portal_err("open PipeWire remote", e))?;
    log.notice(COMPONENT, format!("Sharing node {node_id} at {rect:?}"));
    Ok(Cast { proxy, session, fd, node_id, rect, cursor_metadata })
}

/// Factor from the portal's logical coordinates to X11 (XWayland) coordinates, which the
/// rest of the recorder uses. They are the same unless XWayland runs at native scale
/// (GNOME's `xwayland-native-scaling`), where X11 coordinates are physical pixels. Found by
/// the X11 monitor that is the shared monitor scaled by one factor.
fn x11_factor(logical: &Rect, x11_displays: &[Rect]) -> f64 {
    const EPS: f64 = 2.0;
    if logical.width <= 0.0 {
        return 1.0;
    }
    x11_displays
        .iter()
        .find_map(|d| {
            let k = d.width / logical.width;
            ((d.x - logical.x * k).abs() < EPS && (d.y - logical.y * k).abs() < EPS && (d.height - logical.height * k).abs() < EPS).then_some(k)
        })
        .unwrap_or(1.0)
}

/// The UI scale XWayland gives X11 apps (`Xft.dpi` / 96), 1 when unset.
fn x11_ui_scale() -> f64 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let dpi = (|| {
        let (conn, n) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[n].root;
        let reply = conn.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16).ok()?.reply().ok()?;
        let text = String::from_utf8_lossy(&reply.value).into_owned();
        text.lines().find_map(|l| l.strip_prefix("Xft.dpi:").and_then(|v| v.trim().parse::<f64>().ok()))
    })();
    dpi.map(|d| (d / 96.0).max(1.0)).unwrap_or(1.0)
}

/// A window the user picked in the portal's dialog, streaming and ready to record.
pub struct PickedWindow {
    rec: PortalRecorder,
    scale: f64,
}

impl PickedWindow {
    /// Size of the window in points (from its first frame).
    pub fn size(&self) -> (u32, u32) {
        let r = *self.rec.shared.rect.lock();
        (r.width.round() as u32, r.height.round() as u32)
    }

    /// Pixels per point to record the window at: the UI scale, so a HiDPI window keeps its
    /// detail (the window's own monitor is unknown on Wayland).
    pub fn scale(&self) -> f64 {
        self.scale
    }
}

/// Ask the user to pick a window in the screen-sharing dialog, and start its stream.
pub fn pick_window(fps: u32, log: &RecLog) -> Result<PickedWindow> {
    let cast = pollster::block_on(open_cast(SourceType::Window, None, log))?;
    let scale = x11_ui_scale();
    let want = ((cast.rect.width * scale).round() as u32, (cast.rect.height * scale).round() as u32);
    let rec = PortalRecorder::connect(cast, fps, want, true, log.clone())?;
    // a window stream has no position on screen: everything is relative to the window, whose
    // size is its first frame's (the buffer's crop region) in points
    let (fw, fh) = *rec.shared.frame_size.get().expect("first frame in");
    *rec.shared.rect.lock() = Rect { x: 0.0, y: 0.0, width: fw as f64 / scale, height: fh as f64 / scale };
    Ok(PickedWindow { rec, scale })
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

/// Formats we accept; `want` is the preferred size (the producer may offer a range and then
/// fixates on it).
fn enum_format_pod(fps: u32, want: (u32, u32)) -> Vec<u8> {
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
            Rectangle { width: want.0.clamp(1, 8192), height: want.1.clamp(1, 8192) },
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

/// Mappable buffers only (no DMA-BUF: we copy frames on the CPU), plus the cursor and crop
/// metadata (a window stream's buffer is larger than the window; the crop says where it is).
fn buffer_and_meta_pods() -> [Vec<u8>; 3] {
    use spa::sys;
    let types = (1 << sys::SPA_DATA_MemPtr) | (1 << sys::SPA_DATA_MemFd);
    let buffers = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamBuffers,
        id: sys::SPA_PARAM_Buffers,
        properties: vec![Property { key: sys::SPA_PARAM_BUFFERS_dataType, flags: PropertyFlags::empty(), value: Value::Int(types) }],
    };
    // a range like OBS asks for: the producer picks the bitmap size it sends
    let cursor_size = |side: usize| (std::mem::size_of::<sys::spa_meta_cursor>() + std::mem::size_of::<sys::spa_meta_bitmap>() + side * side * 4) as i32;
    let size_range = spa::utils::Choice(
        spa::utils::ChoiceFlags::empty(),
        spa::utils::ChoiceEnum::Range { default: cursor_size(64), min: cursor_size(1), max: cursor_size(1024) },
    );
    let meta = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamMeta,
        id: sys::SPA_PARAM_Meta,
        properties: vec![
            Property { key: sys::SPA_PARAM_META_type, flags: PropertyFlags::empty(), value: Value::Id(spa::utils::Id(sys::SPA_META_Cursor)) },
            Property { key: sys::SPA_PARAM_META_size, flags: PropertyFlags::empty(), value: Value::Choice(spa::pod::ChoiceValue::Int(size_range)) },
        ],
    };
    let crop = Object {
        type_: sys::SPA_TYPE_OBJECT_ParamMeta,
        id: sys::SPA_PARAM_Meta,
        properties: vec![
            Property { key: sys::SPA_PARAM_META_type, flags: PropertyFlags::empty(), value: Value::Id(spa::utils::Id(sys::SPA_META_VideoCrop)) },
            Property { key: sys::SPA_PARAM_META_size, flags: PropertyFlags::empty(), value: Value::Int(std::mem::size_of::<sys::spa_meta_region>() as i32) },
        ],
    };
    [serialize(buffers), serialize(meta), serialize(crop)]
}

/// State shared between the PipeWire thread and the recorder.
struct Shared {
    store: Arc<FrameStore>,
    negotiated: OnceLock<Negotiated>,
    /// Size of the frames in the store: the first buffer's crop region (the window in a
    /// window stream), else the whole buffer. Later frames are fitted into it.
    frame_size: OnceLock<(u32, u32)>,
    /// Window streams: the opaque part of the first frame's crop (relative to it), which
    /// trims the window's drop shadow off every frame.
    trim: OnceLock<Region>,
    is_window: bool,
    /// Shared monitor (or window) in global points; maps frame pixels to points.
    rect: Mutex<Rect>,
    pointer: Mutex<Option<PointerSink>>,
    error: Mutex<Option<String>>,
    log: RecLog,
    /// Buffers seen, and those carrying a valid cursor position (logged at the end).
    buffers: AtomicU64,
    cursor_buffers: AtomicU64,
}

impl Shared {
    fn fail(&self, msg: String) {
        self.log.error(COMPONENT, msg.clone());
        self.error.lock().get_or_insert(msg);
    }
}

/// Region of a buffer to keep, in buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Region {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// The crop metadata's region clipped to the buffer, or the whole buffer.
fn crop_region(crop: Option<(i32, i32, u32, u32)>, width: u32, height: u32) -> Region {
    let whole = Region { x: 0, y: 0, w: width, h: height };
    let Some((x, y, w, h)) = crop else { return whole };
    let (x, y) = (x.max(0) as u32, y.max(0) as u32);
    if w == 0 || h == 0 || x >= width || y >= height {
        return whole;
    }
    Region { x, y, w: w.min(width - x), h: h.min(height - y) }
}

/// The pixels of one mapped `width`×`height` buffer and its row stride, or `None` for a
/// cursor-only update or a buffer too short for its size.
fn with_pixels<R>(buffer: &mut pw::buffer::Buffer, width: u32, height: u32, f: impl FnOnce(&[u8], usize) -> R) -> Option<R> {
    let d = buffer.datas_mut().first_mut()?;
    let (offset, size, stride) = {
        let c = d.chunk();
        (c.offset() as usize, c.size() as usize, c.stride().unsigned_abs() as usize)
    };
    if size == 0 {
        return None; // cursor-only update
    }
    let src_row = width as usize * 4;
    let stride = if stride == 0 { src_row } else { stride };
    let src_h = height as usize;
    let src = d.data()?;
    if src_h == 0 || stride < src_row || offset + stride * (src_h - 1) + src_row > src.len() {
        return None;
    }
    Some(f(&src[offset..], stride))
}

/// Byte offset of the alpha channel in a 4-byte pixel, for formats that have one.
fn alpha_offset(pix_fmt: &str) -> Option<usize> {
    match pix_fmt {
        "bgra" | "rgba" => Some(3),
        "argb" | "abgr" => Some(0),
        _ => None,
    }
}

/// The part of `region` holding (nearly) opaque pixels, relative to `region`: a window stream
/// includes the client's drop shadow, which is transparent and would show as a dark border.
fn opaque_bounds(src: &[u8], stride: usize, region: Region, alpha: usize) -> Region {
    const OPAQUE: u8 = 200;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..region.h {
        let row = &src[(region.y + y) as usize * stride + region.x as usize * 4..][..region.w as usize * 4];
        let opaque = |x: &u32| row[*x as usize * 4 + alpha] >= OPAQUE;
        if let Some(first) = (0..region.w).find(opaque) {
            let last = (0..region.w).rev().find(opaque).unwrap_or(first);
            x0 = x0.min(first);
            x1 = x1.max(last + 1);
            y0 = y0.min(y);
            y1 = y + 1;
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Region { x: 0, y: 0, w: region.w, h: region.h };
    }
    Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// Copy `region` of one mapped `width`×`height` buffer into the store as tightly packed rows
/// of `out` size. A region of another size (a resized window) is cropped or padded with black
/// at the right and bottom, since the encoder's size is fixed.
fn take_frame(buffer: &mut pw::buffer::Buffer, width: u32, height: u32, region: Region, out: (u32, u32), store: &FrameStore) {
    let Some(buf) = with_pixels(buffer, width, height, |src, stride| copy_region(src, stride, region, out)) else { return };
    store.put(buf);
}

fn copy_region(src: &[u8], stride: usize, region: Region, out: (u32, u32)) -> Vec<u8> {
    let (row, h) = (out.0 as usize * 4, out.1 as usize);
    let (x0, y0) = (region.x as usize * 4, region.y as usize);
    let copy = (region.w as usize * 4).min(row);
    let mut buf = vec![0u8; row * h];
    for y in 0..h.min(region.h as usize) {
        let s = (y0 + y) * stride + x0;
        buf[y * row..y * row + copy].copy_from_slice(&src[s..s + copy]);
    }
    buf
}

/// PipeWire thread: connect to the portal's remote, negotiate, copy frames until told to quit.
fn run_pipewire(fd: OwnedFd, node_id: u32, fps: u32, want: (u32, u32), shared: Arc<Shared>, quit_rx: pw::channel::Receiver<()>) {
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
        let logged_meta = std::cell::Cell::new(false);
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
                        s_param.log.notice(COMPONENT, format!("Stream resized to {}x{} (was {}x{})", n.width, n.height, first.width, first.height));
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
                let Some(&n) = s_proc.negotiated.get() else { return };
                let size = info.size();
                let crop = buffer
                    .find_meta::<spa::buffer::meta::MetaVideoCrop>()
                    .map(|c| c.meta_region())
                    .filter(|r| r.is_valid())
                    .map(|r| (r.position().x, r.position().y, r.size().width, r.size().height));
                let mut region = crop_region(crop, size.width, size.height);
                if let (true, Some(alpha)) = (s_proc.is_window, alpha_offset(n.pix_fmt)) {
                    let trim = match s_proc.trim.get() {
                        Some(t) => Some(*t),
                        None => with_pixels(&mut buffer, size.width, size.height, |src, stride| opaque_bounds(src, stride, region, alpha))
                            .map(|t| *s_proc.trim.get_or_init(|| t)),
                    };
                    if let Some(t) = trim {
                        let (x, y) = (region.x + t.x, region.y + t.y);
                        region = Region { x, y, w: t.w.min(size.width.saturating_sub(x)), h: t.h.min(size.height.saturating_sub(y)) };
                    }
                }
                let out = *s_proc.frame_size.get_or_init(|| {
                    s_proc.log.notice(COMPONENT, format!("Frames {}x{} (buffer {}x{}, crop {crop:?}, trim {:?})", region.w, region.h, size.width, size.height, s_proc.trim.get()));
                    (region.w & !1, region.h & !1)
                });
                let cursor = buffer.find_meta::<spa::buffer::meta::MetaCursor>();
                if !logged_meta.replace(true) {
                    let state = match &cursor {
                        None => "absent".to_string(),
                        Some(c) => format!("id={} valid={}", c.id(), c.is_valid()),
                    };
                    s_proc.log.notice(COMPONENT, format!("Cursor metadata on the first buffer: {state}"));
                }
                s_proc.buffers.fetch_add(1, Ordering::Relaxed);
                if let Some(c) = cursor {
                    if c.is_valid() && out.0 > 0 {
                        s_proc.cursor_buffers.fetch_add(1, Ordering::Relaxed);
                        let p = c.position();
                        let r = *s_proc.rect.lock();
                        let (sx, sy) = (out.0 as f64 / r.width, out.1 as f64 / r.height);
                        let pt = (r.x + (p.x as f64 - region.x as f64) / sx, r.y + (p.y as f64 - region.y as f64) / sy);
                        // only a delivered position counts: the sink is attached after the
                        // stream starts, and the first position must not be lost before that
                        if pt != last_ptr.get() {
                            if let Some(sink) = s_proc.pointer.lock().as_ref() {
                                sink(pt.0, pt.1);
                                last_ptr.set(pt);
                            }
                        }
                    }
                }
                take_frame(&mut buffer, size.width, size.height, region, out, &s_proc.store);
            })
            .register()?;
        let fmt = enum_format_pod(fps, want);
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
        let x11_displays: Vec<Rect> = crate::discover::displays().iter().map(|d| d.bounds_points).collect();
        let open = |token: Option<&str>| -> Result<Cast> {
            let mut cast = pollster::block_on(open_cast(SourceType::Monitor, token, &log))?;
            let k = x11_factor(&cast.rect, &x11_displays);
            if k != 1.0 {
                let r = cast.rect;
                cast.rect = Rect { x: r.x * k, y: r.y * k, width: r.width * k, height: r.height * k };
                log.notice(COMPONENT, format!("XWayland coordinates are {k}x the portal's: shared monitor at {:?}", cast.rect));
            }
            Ok(cast)
        };
        let mut cast = open(token.as_deref())?;
        if !contains(&cast.rect, &plan.bounds) && token.is_some() {
            log.notice(COMPONENT, "Remembered monitor does not contain the recording area; asking again");
            let _ = pollster::block_on(cast.session.close());
            save_token(None);
            cast = open(None)?;
        }
        if !contains(&cast.rect, &plan.bounds) {
            let _ = pollster::block_on(cast.session.close());
            return Err(Error::Capture(
                "The shared screen does not contain the recording area. Start again and pick the screen you are recording in the sharing dialog."
                    .into(),
            ));
        }
        // the shared monitor at the plan's pixel density: full detail on HiDPI
        let want = ((cast.rect.width * plan.scale).round() as u32, (cast.rect.height * plan.scale).round() as u32);
        let mut rec = Self::connect(cast, cfg.fps.max(1), want, false, log.clone())?;
        rec.encode(cfg, &plan, log)?;
        Ok(rec)
    }

    /// Record a window picked with [`pick_window`]; `plan` comes from
    /// [`super::display::window_stream_plan`] for its size.
    pub fn start_window(cfg: &RecordingConfig, plan: DisplayPlan, log: RecLog, window: PickedWindow) -> Result<Self> {
        let mut rec = window.rec;
        rec.encode(cfg, &plan, log)?;
        Ok(rec)
    }

    /// Start the PipeWire stream and wait for its first frame (nothing is encoded yet).
    fn connect(cast: Cast, fps: u32, want: (u32, u32), is_window: bool, log: RecLog) -> Result<Self> {
        let Cast { proxy, session, fd, node_id, rect, cursor_metadata } = cast;
        let shared = Arc::new(Shared {
            store: Arc::new(FrameStore::new(0, 0)),
            negotiated: OnceLock::new(),
            frame_size: OnceLock::new(),
            trim: OnceLock::new(),
            is_window,
            rect: Mutex::new(rect),
            pointer: Mutex::new(None),
            error: Mutex::new(None),
            log: log.clone(),
            buffers: AtomicU64::new(0),
            cursor_buffers: AtomicU64::new(0),
        });
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let pw_shared = shared.clone();
        let pw_thread = std::thread::Builder::new()
            .name("sf-pipewire".into())
            .spawn(move || run_pipewire(fd, node_id, fps, want, pw_shared, quit_rx))
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
        Ok(rec)
    }

    /// Start encoding `plan`'s region of the stream.
    fn encode(&mut self, cfg: &RecordingConfig, plan: &DisplayPlan, log: RecLog) -> Result<()> {
        let fps = cfg.fps.max(1);
        let shared = self.shared.clone();
        let cursor_metadata = self.cursor_metadata;
        let n = *shared.negotiated.get().expect("negotiated before the first frame");
        let (fw, fh) = *shared.frame_size.get().expect("first frame in");
        let rect = *shared.rect.lock();

        // region of the shared monitor (or window), in frame pixels
        let (sx, sy) = (fw as f64 / rect.width, fh as f64 / rect.height);
        let cx = (((plan.bounds.x - rect.x) * sx).round().max(0.0) as u32).min(fw.saturating_sub(2)) & !1;
        let cy = (((plan.bounds.y - rect.y) * sy).round().max(0.0) as u32).min(fh.saturating_sub(2)) & !1;
        let cw = ((plan.bounds.width * sx).round() as u32).min(fw - cx).max(2) & !1;
        let ch = ((plan.bounds.height * sy).round() as u32).min(fh - cy).max(2) & !1;
        let mut filters = Vec::new();
        if (cx, cy, cw, ch) != (0, 0, fw, fh) {
            filters.push(format!("crop={cw}:{ch}:{cx}:{cy}"));
        }
        if (cw, ch) != (plan.width_px, plan.height_px) {
            filters.push(format!("scale={}:{}:flags=bicubic", plan.width_px, plan.height_px));
        }
        let bitrate = target_bitrate(plan.width_px, plan.height_px, cfg.video_quality);
        log.notice(
            COMPONENT,
            format!("Capture started. encoder=libx264 bitrate={bitrate} stream={}x{} frames={fw}x{fh} filter={filters:?} cursorMetadata={cursor_metadata}", n.width, n.height),
        );
        let (tx, rx) = unbounded();
        let rep = Repeater {
            rx,
            store: shared.store.clone(),
            log,
            component: COMPONENT,
            fps,
            input: RawInput { pix_fmt: n.pix_fmt, width: fw, height: fh, filter: (!filters.is_empty()).then(|| filters.join(",")) },
            bitrate,
            encoder: "libx264",
            channel: crate::display::DISPLAY_CHANNEL.into(),
        };
        self.tx = tx;
        self.repeater = Some(
            std::thread::Builder::new()
                .name("sf-frame-repeater".into())
                .spawn(move || rep.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?,
        );
        Ok(())
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
        let (all, cursor) = (self.shared.buffers.load(Ordering::Relaxed), self.shared.cursor_buffers.load(Ordering::Relaxed));
        self.shared.log.notice(COMPONENT, format!("Stream ended. buffers={all} withCursorPosition={cursor}"));
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
    fn xwayland_native_scaling_factor() {
        // DP-1 2560x1440 and eDP-1 1920x1080 right of it, both at scale 2, X11 in pixels
        let x11 = [Rect { x: 0.0, y: 0.0, width: 5120.0, height: 2880.0 }, Rect { x: 5120.0, y: 720.0, width: 3840.0, height: 2160.0 }];
        assert_eq!(x11_factor(&Rect { x: 2560.0, y: 360.0, width: 1920.0, height: 1080.0 }, &x11), 2.0);
        assert_eq!(x11_factor(&Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1440.0 }, &x11), 2.0);
        // same coordinates (no native scaling)
        assert_eq!(x11_factor(&Rect { x: 0.0, y: 0.0, width: 5120.0, height: 2880.0 }, &x11), 1.0);
        // unknown layout
        assert_eq!(x11_factor(&Rect { x: 7.0, y: 0.0, width: 100.0, height: 100.0 }, &x11), 1.0);
    }

    #[test]
    fn opaque_bounds_trims_the_shadow() {
        // 10x6 BGRA: transparent, with an opaque 6x3 window at (2, 1) and a faint shadow
        let (w, h) = (10usize, 6usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let a = if (2..8).contains(&x) && (1..4).contains(&y) { 255 } else if y == 4 { 60 } else { 0 };
                px[(y * w + x) * 4 + 3] = a;
            }
        }
        let all = Region { x: 0, y: 0, w: w as u32, h: h as u32 };
        assert_eq!(opaque_bounds(&px, w * 4, all, 3), Region { x: 2, y: 1, w: 6, h: 3 });
        // relative to a region that starts inside the buffer
        assert_eq!(opaque_bounds(&px, w * 4, Region { x: 1, y: 1, w: 8, h: 5 }, 3), Region { x: 1, y: 0, w: 6, h: 3 });
        // nothing opaque: keep everything
        assert_eq!(opaque_bounds(&vec![0u8; w * h * 4], w * 4, all, 3), all);
    }

    #[test]
    fn crop_region_clips_to_buffer() {
        let whole = Region { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(crop_region(None, 1920, 1080), whole);
        // a window inside a larger buffer with shadow margins
        assert_eq!(crop_region(Some((26, 23, 1200, 800)), 1300, 900), Region { x: 26, y: 23, w: 1200, h: 800 });
        // overhanging regions are clipped; empty or outside ones mean the whole buffer
        assert_eq!(crop_region(Some((100, 50, 1900, 1100)), 1920, 1080), Region { x: 100, y: 50, w: 1820, h: 1030 });
        assert_eq!(crop_region(Some((0, 0, 0, 0)), 1920, 1080), whole);
        assert_eq!(crop_region(Some((2000, 0, 10, 10)), 1920, 1080), whole);
    }

    #[test]
    fn pods_serialize() {
        assert!(Pod::from_bytes(&enum_format_pod(60, (3840, 2160))).is_some());
        for p in buffer_and_meta_pods() {
            assert!(Pod::from_bytes(&p).is_some());
        }
    }
}
