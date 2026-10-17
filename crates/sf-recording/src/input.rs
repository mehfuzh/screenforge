//! Input recorder: `keystrokes-S.json`, `mouseclicks-S.json`, `mousemoves-S.json`.
//!
//! A listen-only, session-level `CGEventTap` runs on its own thread's `CFRunLoop` and
//! forwards raw events to a writer thread that appends one JSON object per line, exactly
//! like polyrecorder (`[\n{…},\n{…}\n]`). Coordinates are global screen points (top-left
//! origin, y down); times are `processTimeMs` (from the event's mach timestamp) and
//! `unixTimeMs`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result};

/// One line of an event file. Field order is the sorted-key order polyrecorder writes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventLine {
    pub active_modifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<&'static str>,
    /// ScreenForge extension on key events: text caret position (global points) right after
    /// the key, when the focused app exposes it through Accessibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caret_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caret_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_a_repeat: Option<bool>,
    /// Virtual key code of key events (macOS), so shortcut labels need not rely on `character`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_code: Option<u32>,
    pub process_time_ms: f64,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub unix_time_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

/// Incremental writer of one `[...]` JSON array file, one object per line.
pub struct EventFile {
    path: PathBuf,
    w: BufWriter<File>,
    count: usize,
}

impl EventFile {
    pub fn create(path: &Path) -> Result<Self> {
        let file = File::create(path).map_err(|source| Error::Io { path: path.into(), source })?;
        let mut w = BufWriter::new(file);
        w.write_all(b"[").map_err(|source| Error::Io { path: path.into(), source })?;
        Ok(EventFile { path: path.into(), w, count: 0 })
    }

    pub fn push(&mut self, line: &EventLine) -> Result<()> {
        let json = serde_json::to_string(line).map_err(Error::Json)?;
        let sep: &[u8] = if self.count == 0 { b"\n" } else { b",\n" };
        self.w
            .write_all(sep)
            .and_then(|_| self.w.write_all(json.as_bytes()))
            .map_err(|source| Error::Io { path: self.path.clone(), source })?;
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.w.write_all(b"\n]").and_then(|_| self.w.flush()).map_err(|source| Error::Io { path: self.path.clone(), source })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use imp::InputRecorder;

/// Event capture: a listen-only `CGEventTap` on macOS, the X11 RECORD extension (`rdev`) on
/// Linux. Both feed the same writer, so the event files are identical.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    #[cfg(target_os = "macos")]
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        CallbackResult, EventField,
    };
    #[cfg(target_os = "macos")]
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
    #[cfg(target_os = "macos")]
    use foreign_types::ForeignType;

    use super::{EventFile, EventLine};
    use crate::clock;
    use crate::log::RecLog;
    use crate::metadata::input_filenames;
    use crate::{Error, Result};

    const COMPONENT: &str = "InputRecorder";

    #[cfg(target_os = "macos")]
    extern "C" {
        fn CGEventGetTimestamp(event: core_graphics::sys::CGEventRef) -> u64;
        fn CGEventKeyboardGetUnicodeString(
            event: core_graphics::sys::CGEventRef,
            max_len: libc::c_ulong,
            actual_len: *mut libc::c_ulong,
            out: *mut u16,
        );
    }

    #[derive(Debug)]
    struct RawEvent {
        kind: &'static str,
        button: Option<&'static str>,
        x: Option<f64>,
        y: Option<f64>,
        character: Option<String>,
        is_repeat: Option<bool>,
        key_code: Option<u32>,
        modifiers: Vec<&'static str>,
        process_time_ms: f64,
        unix_time_ms: f64,
    }

    enum Cmd {
        Event(RawEvent),
        CursorChanged(String),
        Begin { index: usize, dir: PathBuf, reply: Sender<Result<()>> },
        End { reply: Sender<Result<()>> },
        Stop,
    }

    #[cfg(target_os = "macos")]
    fn modifiers(flags: CGEventFlags) -> Vec<&'static str> {
        let mut v = Vec::new();
        if flags.contains(CGEventFlags::CGEventFlagShift) {
            v.push("shift");
        }
        if flags.contains(CGEventFlags::CGEventFlagControl) {
            v.push("control");
        }
        if flags.contains(CGEventFlags::CGEventFlagAlternate) {
            v.push("option");
        }
        if flags.contains(CGEventFlags::CGEventFlagCommand) {
            v.push("command");
        }
        v
    }

    #[cfg(target_os = "macos")]
    fn key_characters(event: &CGEvent) -> String {
        let mut buf = [0u16; 8];
        let mut len: libc::c_ulong = 0;
        unsafe { CGEventKeyboardGetUnicodeString(event.as_ptr(), buf.len() as libc::c_ulong, &mut len, buf.as_mut_ptr()) };
        String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
    }

    #[cfg(target_os = "macos")]
    fn raw_event(etype: CGEventType, event: &CGEvent) -> Option<RawEvent> {
        use CGEventType::*;
        let (kind, button, mouse, key) = match etype {
            LeftMouseDown => ("mouseDown", Some("left"), true, false),
            LeftMouseUp => ("mouseUp", Some("left"), true, false),
            RightMouseDown => ("mouseDown", Some("right"), true, false),
            RightMouseUp => ("mouseUp", Some("right"), true, false),
            OtherMouseDown => ("mouseDown", Some("other"), true, false),
            OtherMouseUp => ("mouseUp", Some("other"), true, false),
            MouseMoved => ("mouseMoved", None, true, false),
            LeftMouseDragged => ("mouseDragged", Some("left"), true, false),
            RightMouseDragged => ("mouseDragged", Some("right"), true, false),
            OtherMouseDragged => ("mouseDragged", Some("other"), true, false),
            KeyDown => ("keyDown", None, false, true),
            KeyUp => ("keyUp", None, false, true),
            _ => return None,
        };
        let ts = unsafe { CGEventGetTimestamp(event.as_ptr()) };
        let process_time_ms = if ts > 0 { clock::process_time_ms_from_event_timestamp(ts) } else { clock::process_time_ms() };
        let (x, y) = if mouse {
            let p = event.location();
            (Some(p.x), Some(p.y))
        } else {
            (None, None)
        };
        let (character, is_repeat, key_code) = if key {
            (
                Some(key_characters(event)),
                Some(event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0),
                Some(event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32),
            )
        } else {
            (None, None, None)
        };
        Some(RawEvent {
            kind,
            button,
            x,
            y,
            character,
            is_repeat,
            key_code,
            modifiers: modifiers(event.get_flags()),
            process_time_ms,
            unix_time_ms: clock::unix_time_ms(),
        })
    }

    /// Current mouse location in global points (top-left origin), for synthesized moves.
    #[cfg(target_os = "linux")]
    fn mouse_location() -> Option<(f64, f64)> {
        None
    }

    /// Current mouse location in global points (top-left origin), for synthesized moves.
    #[cfg(target_os = "macos")]
    fn mouse_location() -> Option<(f64, f64)> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
        let e = CGEvent::new(source).ok()?;
        let p = e.location();
        Some((p.x, p.y))
    }

    struct Session {
        keys: EventFile,
        clicks: EventFile,
        moves: EventFile,
    }

    struct Writer {
        rx: Receiver<Cmd>,
        log: RecLog,
        session: Option<Session>,
        cursor_id: String,
        last_pos: Option<(f64, f64)>,
        held_button: Option<&'static str>,
        capture_keystrokes: bool,
    }

    impl Writer {
        fn run(mut self) {
            while let Ok(cmd) = self.rx.recv() {
                match cmd {
                    Cmd::Event(ev) => self.on_event(ev),
                    Cmd::CursorChanged(id) => self.on_cursor(id),
                    Cmd::Begin { index, dir, reply } => {
                        let r = self.begin(index, &dir);
                        let _ = reply.send(r);
                    }
                    Cmd::End { reply } => {
                        let r = self.end();
                        let _ = reply.send(r);
                    }
                    Cmd::Stop => break,
                }
            }
            if let Err(e) = self.end() {
                self.log.warning(COMPONENT, format!("Could not close event files: {e}"));
            }
        }

        fn begin(&mut self, index: usize, dir: &Path) -> Result<()> {
            self.end()?;
            let (k, c, m) = input_filenames(index);
            self.session = Some(Session {
                keys: EventFile::create(&dir.join(k))?,
                clicks: EventFile::create(&dir.join(c))?,
                moves: EventFile::create(&dir.join(m))?,
            });
            Ok(())
        }

        fn end(&mut self) -> Result<()> {
            if let Some(s) = self.session.take() {
                s.keys.finish()?;
                s.clicks.finish()?;
                s.moves.finish()?;
            }
            Ok(())
        }

        fn write(&mut self, line: EventLine) {
            let Some(s) = self.session.as_mut() else { return };
            let file = match line.kind {
                "keyDown" | "keyUp" => &mut s.keys,
                "mouseDown" | "mouseUp" => &mut s.clicks,
                _ => &mut s.moves,
            };
            if let Err(e) = file.push(&line) {
                self.log.warning(COMPONENT, format!("Could not write event: {e}"));
            }
        }

        fn on_event(&mut self, ev: RawEvent) {
            if let (Some(x), Some(y)) = (ev.x, ev.y) {
                self.last_pos = Some((x, y));
            }
            match ev.kind {
                "mouseDown" => self.held_button = ev.button,
                "mouseUp" => self.held_button = None,
                "keyDown" | "keyUp" if !self.capture_keystrokes => return,
                _ => {}
            }
            let is_mouse = ev.x.is_some();
            // on key-up the character is in place, so the caret sits where typing continues
            let caret = if ev.kind == "keyUp" && self.session.is_some() { crate::caret::caret_position() } else { None };
            self.write(EventLine {
                active_modifiers: ev.modifiers,
                button: ev.button,
                caret_x: caret.map(|c| c.0),
                caret_y: caret.map(|c| c.1),
                character: ev.character,
                cursor_id: is_mouse.then(|| self.cursor_id.clone()),
                is_a_repeat: ev.is_repeat,
                key_code: ev.key_code,
                process_time_ms: ev.process_time_ms,
                kind: ev.kind,
                unix_time_ms: ev.unix_time_ms,
                x: ev.x,
                y: ev.y,
            });
        }

        /// polyrecorder "generates mouse move events for cursor changes between mouse moves".
        fn on_cursor(&mut self, id: String) {
            self.cursor_id = id;
            if self.session.is_none() {
                return;
            }
            let Some((x, y)) = self.last_pos.or_else(mouse_location) else { return };
            let (process_time_ms, unix_time_ms) = clock::now_pair();
            let (kind, button) = match self.held_button {
                Some(b) => ("mouseDragged", Some(b)),
                None => ("mouseMoved", None),
            };
            self.write(EventLine {
                active_modifiers: Vec::new(),
                button,
                caret_x: None,
                caret_y: None,
                character: None,
                cursor_id: Some(self.cursor_id.clone()),
                is_a_repeat: None,
                key_code: None,
                process_time_ms,
                kind,
                unix_time_ms,
                x: Some(x),
                y: Some(y),
            });
        }
    }

    /// Event tap + JSON writer.
    pub struct InputRecorder {
        tx: Sender<Cmd>,
        stop: Arc<AtomicBool>,
        tap_thread: Option<JoinHandle<()>>,
        writer_thread: Option<JoinHandle<()>>,
    }

    impl InputRecorder {
        /// Create the event tap (fails with [`Error::PermissionDenied`] without Input
        /// Monitoring) and start the writer. No file is written until [`begin_session`].
        ///
        /// [`begin_session`]: InputRecorder::begin_session
        pub fn start(log: RecLog, capture_keystrokes: bool) -> Result<Self> {
            let (tx, rx) = unbounded::<Cmd>();
            let stop = Arc::new(AtomicBool::new(false));

            // Wayland gives no global input: the X11 tap would only see XWayland windows, so
            // there is none, and pointer positions come from the screen-cast stream instead
            // (see `pointer_sink`). Clicks and keys are not recorded there.
            #[cfg(target_os = "linux")]
            let use_tap = !crate::portal::is_wayland();
            #[cfg(not(target_os = "linux"))]
            let use_tap = true;
            let tap_thread = if use_tap {
                // Create the tap on its own thread (the tap is tied to that thread's run loop)
                // and report success/failure back before returning.
                let (ready_tx, ready_rx) = bounded::<std::result::Result<(), ()>>(1);
                let tap_tx = tx.clone();
                let tap_stop = stop.clone();
                let tap_thread = std::thread::Builder::new()
                    .name("sf-input-tap".into())
                    .spawn(move || tap_loop(tap_tx, tap_stop, capture_keystrokes, ready_tx))
                    .map_err(|source| Error::Io { path: "thread".into(), source })?;
                match ready_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(())) => {}
                    _ => {
                        let _ = tap_thread.join();
                        return Err(Error::PermissionDenied("inputMonitoring"));
                    }
                }
                Some(tap_thread)
            } else {
                log.notice(COMPONENT, "Wayland session: no global input tap; clicks and keys are not recorded; the cursor is part of the video (pointer positions only with SF_PORTAL_CURSOR_METADATA=1).");
                None
            };

            let writer = Writer {
                rx,
                log: log.clone(),
                session: None,
                cursor_id: "arrow".into(),
                last_pos: None,
                held_button: None,
                capture_keystrokes,
            };
            let writer_thread = std::thread::Builder::new()
                .name("sf-input-writer".into())
                .spawn(move || writer.run())
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            log.notice(COMPONENT, format!("Event tap created. captureKeyStrokes={capture_keystrokes}"));
            Ok(InputRecorder { tx, stop, tap_thread, writer_thread: Some(writer_thread) })
        }

        /// Called by the cursor recorder when the cursor image changes.
        pub fn cursor_changed(&self, id: &str) {
            let _ = self.tx.send(Cmd::CursorChanged(id.to_string()));
        }

        /// A handle that can be passed to [`crate::cursor::CursorRecorder::start`].
        pub fn cursor_change_sink(&self) -> Box<dyn Fn(&str) + Send> {
            let tx = self.tx.clone();
            Box::new(move |id: &str| {
                let _ = tx.send(Cmd::CursorChanged(id.to_string()));
            })
        }

        /// Feed pointer positions (global points) from another source, e.g. the cursor
        /// metadata of a Wayland screen-cast stream.
        pub fn pointer_sink(&self) -> Box<dyn Fn(f64, f64) + Send> {
            let tx = self.tx.clone();
            Box::new(move |x: f64, y: f64| {
                let _ = tx.send(Cmd::Event(RawEvent {
                    kind: "mouseMoved",
                    button: None,
                    x: Some(x),
                    y: Some(y),
                    character: None,
                    is_repeat: None,
                    key_code: None,
                    modifiers: Vec::new(),
                    process_time_ms: clock::process_time_ms(),
                    unix_time_ms: clock::unix_time_ms(),
                }));
            })
        }

        /// Open the three event files for session `index` in `dir`.
        pub fn begin_session(&self, index: usize, dir: &Path) -> Result<()> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::Begin { index, dir: dir.into(), reply }).map_err(|_| Error::Capture("input writer stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("input writer stopped".into()))?
        }

        /// Close the current session's files.
        pub fn end_session(&self) -> Result<()> {
            let (reply, rx) = bounded(1);
            self.tx.send(Cmd::End { reply }).map_err(|_| Error::Capture("input writer stopped".into()))?;
            rx.recv().map_err(|_| Error::Capture("input writer stopped".into()))?
        }

        /// Remove the tap and close files.
        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.tx.send(Cmd::Stop);
            if let Some(t) = self.tap_thread.take() {
                let _ = t.join();
            }
            if let Some(t) = self.writer_thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for InputRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// X11 input through the RECORD extension. `rdev::listen` cannot be stopped, so the
    /// listener thread outlives the recording and simply drops events once `stop` is set.
    #[cfg(target_os = "linux")]
    fn tap_loop(tx: Sender<Cmd>, stop: Arc<AtomicBool>, keys: bool, ready: Sender<std::result::Result<(), ()>>) {
        use rdev::{Button, EventType, Key};
        let (err_tx, err_rx) = bounded::<()>(1);
        let listener = std::thread::Builder::new().name("sf-input-x11".into()).spawn(move || {
            let mut pos: Option<(f64, f64)> = None;
            let mut held: Option<&'static str> = None;
            let (mut shift, mut ctrl, mut alt, mut meta) = (false, false, false, false);
            let r = rdev::listen(move |ev| {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let mods = |shift: bool, ctrl: bool, alt: bool, meta: bool| {
                    let mut v = Vec::new();
                    if shift { v.push("shift") }
                    if ctrl { v.push("control") }
                    if alt { v.push("option") }
                    if meta { v.push("command") }
                    v
                };
                let button_name = |b: &Button| match b {
                    Button::Left => "left",
                    Button::Right => "right",
                    _ => "other",
                };
                let (kind, button, xy, character, repeat) = match &ev.event_type {
                    EventType::MouseMove { x, y } => {
                        pos = Some((*x, *y));
                        (if held.is_some() { "mouseDragged" } else { "mouseMoved" }, held, pos, None, None)
                    }
                    EventType::ButtonPress(b) => {
                        held = Some(button_name(b));
                        ("mouseDown", held, pos, None, None)
                    }
                    EventType::ButtonRelease(b) => {
                        held = None;
                        ("mouseUp", Some(button_name(b)), pos, None, None)
                    }
                    EventType::KeyPress(k) | EventType::KeyRelease(k) => {
                        let down = matches!(ev.event_type, EventType::KeyPress(_));
                        match k {
                            Key::ShiftLeft | Key::ShiftRight => shift = down,
                            Key::ControlLeft | Key::ControlRight => ctrl = down,
                            Key::Alt | Key::AltGr => alt = down,
                            Key::MetaLeft | Key::MetaRight => meta = down,
                            _ => {}
                        }
                        if !keys {
                            return;
                        }
                        (if down { "keyDown" } else { "keyUp" }, None, None, Some(ev.name.clone().unwrap_or_default()), Some(false))
                    }
                    EventType::Wheel { .. } => return,
                };
                if (kind.starts_with("mouse")) && xy.is_none() {
                    return;
                }
                let raw = RawEvent {
                    kind,
                    button,
                    x: xy.map(|p| p.0),
                    y: xy.map(|p| p.1),
                    character,
                    is_repeat: repeat,
                    key_code: None,
                    modifiers: mods(shift, ctrl, alt, meta),
                    process_time_ms: clock::process_time_ms(),
                    unix_time_ms: clock::unix_time_ms(),
                };
                let _ = tx.send(Cmd::Event(raw));
            });
            if r.is_err() {
                let _ = err_tx.send(());
            }
        });
        if listener.is_err() {
            let _ = ready.send(Err(()));
            return;
        }
        // listen() returns straight away when X11/RECORD is unavailable; otherwise it blocks
        match err_rx.recv_timeout(Duration::from_millis(400)) {
            Ok(()) => {
                let _ = ready.send(Err(()));
            }
            Err(_) => {
                let _ = ready.send(Ok(()));
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn tap_loop(tx: Sender<Cmd>, stop: Arc<AtomicBool>, keys: bool, ready: Sender<std::result::Result<(), ()>>) {
        use CGEventType::*;
        let mut types = vec![
            LeftMouseDown,
            LeftMouseUp,
            RightMouseDown,
            RightMouseUp,
            MouseMoved,
            LeftMouseDragged,
            RightMouseDragged,
            OtherMouseDown,
            OtherMouseUp,
            OtherMouseDragged,
        ];
        if keys {
            types.push(KeyDown);
            types.push(KeyUp);
        }
        let reenable = Arc::new(AtomicBool::new(false));
        let cb_reenable = reenable.clone();
        let tap = CGEventTap::new(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            types,
            move |_proxy, etype, event| {
                match etype {
                    TapDisabledByTimeout | TapDisabledByUserInput => cb_reenable.store(true, Ordering::Relaxed),
                    _ => {
                        if let Some(ev) = raw_event(etype, event) {
                            let _ = tx.send(Cmd::Event(ev));
                        }
                    }
                }
                CallbackResult::Keep
            },
        );
        let tap = match tap {
            Ok(t) => t,
            Err(()) => {
                let _ = ready.send(Err(()));
                return;
            }
        };
        let Ok(source) = tap.mach_port().create_runloop_source(0) else {
            let _ = ready.send(Err(()));
            return;
        };
        let rl = CFRunLoop::get_current();
        rl.add_source(&source, unsafe { kCFRunLoopDefaultMode });
        tap.enable();
        let _ = ready.send(Ok(()));
        while !stop.load(Ordering::Relaxed) {
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(100), false);
            if reenable.swap(false, Ordering::Relaxed) {
                tap.enable();
            }
        }
        drop(tap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_file_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mousemoves-0.json");
        let mut f = EventFile::create(&path).unwrap();
        f.push(&EventLine {
            active_modifiers: vec!["shift"],
            button: None,
            caret_x: None,
            caret_y: None,
            character: None,
            cursor_id: Some("arrow".into()),
            is_a_repeat: None,
            key_code: None,
            process_time_ms: 1681.5,
            kind: "mouseMoved",
            unix_time_ms: 1791399209826.581,
            x: Some(1945.68359375),
            y: Some(649.703125),
        })
        .unwrap();
        f.push(&EventLine {
            active_modifiers: vec![],
            button: Some("left"),
            caret_x: None,
            caret_y: None,
            character: None,
            cursor_id: Some("arrow".into()),
            is_a_repeat: None,
            key_code: None,
            process_time_ms: 1700.0,
            kind: "mouseDragged",
            unix_time_ms: 1791399209900.0,
            x: Some(1.0),
            y: Some(2.0),
        })
        .unwrap();
        f.finish().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "[\n{\"activeModifiers\":[\"shift\"],\"cursorId\":\"arrow\",\"processTimeMs\":1681.5,\"type\":\"mouseMoved\",\"unixTimeMs\":1791399209826.581,\"x\":1945.68359375,\"y\":649.703125},\n{\"activeModifiers\":[],\"button\":\"left\",\"cursorId\":\"arrow\",\"processTimeMs\":1700.0,\"type\":\"mouseDragged\",\"unixTimeMs\":1791399209900.0,\"x\":1.0,\"y\":2.0}\n]"
        );
        let events = sf_project::recording::load_events(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].button.as_deref(), Some("left"));
    }

    #[test]
    fn empty_event_file_is_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keystrokes-0.json");
        EventFile::create(&path).unwrap().finish().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[\n]");
        assert!(sf_project::recording::load_events(&path).unwrap().is_empty());
    }
}
