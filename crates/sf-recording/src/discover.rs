//! Enumerate displays, windows, microphones and cameras (the equivalent of
//! `polyrecorder discover`).

use sf_project::schema::common::Rect;

/// A connected display.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// `CGDirectDisplayID`.
    pub id: u32,
    /// Global screen points, origin top-left of the main display, y down.
    pub bounds_points: Rect,
    /// Backing scale (pixels per point), e.g. 2.0 for Retina.
    pub scale: f64,
    pub refresh_rate: f64,
    pub is_main: bool,
    pub is_builtin: bool,
    pub name: String,
}

/// An on-screen window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// `CGWindowID`.
    pub id: u32,
    pub title: String,
    pub app_name: String,
    pub bundle_id: String,
    pub pid: i32,
    /// False for windows on another Space, minimised or fully covered.
    pub is_on_screen: bool,
    /// Global screen points, top-left origin.
    pub bounds: Rect,
}

/// An audio input device.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    /// CoreAudio device UID (what Screen Studio stores as `deviceId`).
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// A camera. Camera recording is not implemented yet; the list is informational.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Everything the picker UI needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Discovery {
    pub displays: Vec<DisplayInfo>,
    pub windows: Vec<WindowInfo>,
    pub microphones: Vec<AudioDevice>,
    pub cameras: Vec<CameraInfo>,
}

impl Discovery {
    pub fn main_display(&self) -> Option<&DisplayInfo> {
        self.displays.iter().find(|d| d.is_main).or_else(|| self.displays.first())
    }
    pub fn display(&self, id: u32) -> Option<&DisplayInfo> {
        self.displays.iter().find(|d| d.id == id)
    }
    pub fn window(&self, id: u32) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.id == id)
    }
}

/// Enumerate devices. Never fails: categories that cannot be listed (e.g. windows without
/// Screen Recording permission) come back empty.
pub fn discover() -> Discovery {
    Discovery {
        displays: displays(),
        windows: windows(),
        microphones: microphones(),
        cameras: Vec::new(),
    }
}

/// Displays only (cheap; no TCC involvement).
#[cfg(target_os = "macos")]
pub fn displays() -> Vec<DisplayInfo> {
    use core_graphics::display::CGDisplay;
    let ids = CGDisplay::active_displays().unwrap_or_default();
    let names = screen_names();
    ids.into_iter()
        .map(|id| {
            let d = CGDisplay::new(id);
            let b = d.bounds();
            let bounds_points = Rect { x: b.origin.x, y: b.origin.y, width: b.size.width, height: b.size.height };
            let mode = d.display_mode();
            // CGDisplayPixelsWide reports points on Retina displays; the mode knows the real pixel size.
            let scale = mode
                .as_ref()
                .filter(|m| m.width() > 0)
                .map(|m| m.pixel_width() as f64 / m.width() as f64)
                .filter(|s| *s >= 1.0)
                .unwrap_or(1.0);
            let refresh_rate = mode.as_ref().map(|m| m.refresh_rate()).filter(|r| *r > 0.0).unwrap_or(60.0);
            let is_builtin = d.is_builtin();
            let name = names
                .iter()
                .find(|(nid, _)| *nid == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| if is_builtin { "Built-in Display".into() } else { format!("Display {id}") });
            DisplayInfo { id, bounds_points, scale, refresh_rate, is_main: d.is_main(), is_builtin, name }
        })
        .collect()
}

/// `(display id, localized name)` from `NSScreen`; only available on the main thread.
#[cfg(target_os = "macos")]
fn screen_names() -> Vec<(u32, String)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    use objc2_foundation::{NSNumber, NSString};
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let desc = screen.deviceDescription();
            let key = NSString::from_str("NSScreenNumber");
            let num = desc.objectForKey(&key)?;
            let num = num.downcast_ref::<NSNumber>()?;
            Some((num.unsignedIntValue(), screen.localizedName().to_string()))
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn windows() -> Vec<WindowInfo> {
    windows_with(true)
}

/// Windows of regular apps (layer 0). With `on_screen_only = false`, windows on other Spaces
/// or hidden behind others are included too (`is_on_screen` tells which).
#[cfg(target_os = "macos")]
pub fn windows_with(on_screen_only: bool) -> Vec<WindowInfo> {
    use screencapturekit::shareable_content::SCShareableContent;
    let Ok(content) = SCShareableContent::create().with_on_screen_windows_only(on_screen_only).get() else {
        return Vec::new();
    };
    let Some(snap) = content.snapshot() else { return Vec::new() };
    snap.windows
        .iter()
        .filter(|w| (w.is_on_screen || !on_screen_only) && w.window_layer == 0)
        .filter(|w| w.frame.size.width >= 2.0 && w.frame.size.height >= 2.0)
        .map(|w| {
            let app = w.owning_app_index.and_then(|i| snap.applications.get(i));
            WindowInfo {
                id: w.window_id,
                title: w.title.clone().unwrap_or_default(),
                app_name: app.map(|a| a.application_name.clone()).unwrap_or_default(),
                bundle_id: app.map(|a| a.bundle_identifier.clone()).unwrap_or_default(),
                pid: app.map(|a| a.process_id).unwrap_or(0),
                is_on_screen: w.is_on_screen,
                bounds: Rect {
                    x: w.frame.origin.x,
                    y: w.frame.origin.y,
                    width: w.frame.size.width,
                    height: w.frame.size.height,
                },
            }
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn microphones() -> Vec<AudioDevice> {
    screencapturekit::audio_devices::AudioInputDevice::list()
        .into_iter()
        .map(|d| AudioDevice { id: d.id, name: d.name, is_default: d.is_default })
        .collect()
}

#[cfg(target_os = "linux")]
pub use linux::{displays, microphones, windows, windows_with};

/// Linux (X11): monitors from RandR, windows from the EWMH client list, microphones from cpal.
#[cfg(target_os = "linux")]
mod linux {
    use super::{AudioDevice, DisplayInfo, WindowInfo};
    use sf_project::schema::common::Rect;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    pub fn displays() -> Vec<DisplayInfo> {
        let Ok((conn, screen_num)) = x11rb::connect(None) else { return Vec::new() };
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let monitors = conn.randr_get_monitors(root, true).ok().and_then(|c| c.reply().ok());
        let mut out: Vec<DisplayInfo> = match monitors {
            Some(m) if !m.monitors.is_empty() => m
                .monitors
                .iter()
                .enumerate()
                .map(|(i, mon)| {
                    let name = conn.get_atom_name(mon.name).ok().and_then(|c| c.reply().ok()).map(|r| String::from_utf8_lossy(&r.name).into_owned()).unwrap_or_else(|| format!("Monitor {}", i + 1));
                    DisplayInfo {
                        id: i as u32 + 1,
                        bounds_points: Rect { x: mon.x as f64, y: mon.y as f64, width: mon.width as f64, height: mon.height as f64 },
                        scale: 1.0,
                        refresh_rate: 60.0,
                        is_main: mon.primary,
                        is_builtin: name.starts_with("eDP") || name.starts_with("LVDS"),
                        name,
                    }
                })
                .collect(),
            _ => vec![DisplayInfo {
                id: 1,
                bounds_points: Rect { x: 0.0, y: 0.0, width: screen.width_in_pixels as f64, height: screen.height_in_pixels as f64 },
                scale: 1.0,
                refresh_rate: 60.0,
                is_main: true,
                is_builtin: false,
                name: "Screen".into(),
            }],
        };
        if !out.iter().any(|d| d.is_main) {
            if let Some(d) = out.first_mut() {
                d.is_main = true;
            }
        }
        out
    }

    fn atom(conn: &impl Connection, name: &str) -> Option<u32> {
        conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom)
    }

    fn text_prop(conn: &impl Connection, win: u32, prop: u32, ty: u32) -> Option<String> {
        let r = conn.get_property(false, win, prop, ty, 0, 1024).ok()?.reply().ok()?;
        (!r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).trim_end_matches('\0').to_string())
    }

    pub fn windows_with(_on_screen_only: bool) -> Vec<WindowInfo> {
        let Ok((conn, screen_num)) = x11rb::connect(None) else { return Vec::new() };
        let root = conn.setup().roots[screen_num].root;
        let (Some(list), Some(net_name), Some(utf8), Some(pid_atom)) = (atom(&conn, "_NET_CLIENT_LIST_STACKING"), atom(&conn, "_NET_WM_NAME"), atom(&conn, "UTF8_STRING"), atom(&conn, "_NET_WM_PID")) else { return Vec::new() };
        let Some(reply) = conn.get_property(false, root, list, AtomEnum::WINDOW, 0, 4096).ok().and_then(|c| c.reply().ok()) else { return Vec::new() };
        let Some(ids) = reply.value32() else { return Vec::new() };
        let ids: Vec<u32> = ids.collect();
        let mut out = Vec::new();
        // stacking order is bottom → top; the picker wants front to back
        for win in ids.into_iter().rev() {
            let Some(geo) = conn.get_geometry(win).ok().and_then(|c| c.reply().ok()) else { continue };
            let Some(tr) = conn.translate_coordinates(win, root, 0, 0).ok().and_then(|c| c.reply().ok()) else { continue };
            let title = text_prop(&conn, win, net_name, utf8).or_else(|| text_prop(&conn, win, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into())).unwrap_or_default();
            let class = text_prop(&conn, win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into()).unwrap_or_default();
            // WM_CLASS is "instance\0Class"
            let app_name = class.split('\0').filter(|s| !s.is_empty()).last().unwrap_or("").to_string();
            let pid = conn.get_property(false, win, pid_atom, AtomEnum::CARDINAL, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0) as i32;
            let attrs = conn.get_window_attributes(win).ok().and_then(|c| c.reply().ok());
            let mapped = attrs.map(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE).unwrap_or(true);
            if geo.width < 40 || geo.height < 40 || app_name.eq_ignore_ascii_case("screenforge") {
                continue;
            }
            out.push(WindowInfo {
                id: win,
                title,
                bundle_id: app_name.to_lowercase(),
                app_name,
                pid,
                is_on_screen: mapped,
                bounds: Rect { x: tr.dst_x as f64, y: tr.dst_y as f64, width: geo.width as f64, height: geo.height as f64 },
            });
        }
        out
    }

    pub fn windows() -> Vec<WindowInfo> {
        windows_with(true)
    }

    pub fn microphones() -> Vec<AudioDevice> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let default = host.default_input_device().and_then(|d| d.description().ok().map(|d| d.name().to_string()));
        let Ok(devices) = host.input_devices() else { return Vec::new() };
        devices
            .filter_map(|d| d.description().ok().map(|d| d.name().to_string()))
            .map(|name| AudioDevice { is_default: Some(&name) == default.as_ref(), id: name.clone(), name })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn displays() -> Vec<DisplayInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn windows_with(_on_screen_only: bool) -> Vec<WindowInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn windows() -> Vec<WindowInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn microphones() -> Vec<AudioDevice> {
    Vec::new()
}
