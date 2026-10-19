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

/// A camera.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    /// The device name on macOS (what ffmpeg's AVFoundation input knows it by) and Windows
    /// (DirectShow), the device node (`/dev/video0`) on Linux.
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
        cameras: cameras(),
    }
}

/// Cameras as ffmpeg's AVFoundation input lists them (`-list_devices`), so every entry is
/// one the recorder can open. Screen capture pseudo-devices are left out.
#[cfg(target_os = "macos")]
pub fn cameras() -> Vec<CameraInfo> {
    avfoundation_video_devices()
        .into_iter()
        .enumerate()
        .map(|(i, (_, name))| CameraInfo { id: name.clone(), name, is_default: i == 0 })
        .collect()
}

/// `(ffmpeg index, name)` of the AVFoundation video devices, excluding screens.
#[cfg(target_os = "macos")]
fn avfoundation_video_devices() -> Vec<(usize, String)> {
    let Ok(mut cmd) = crate::ffmpeg::ffmpeg_cmd() else { return Vec::new() };
    // -loglevel info: the device list is printed at info level
    let Ok(out) = cmd.args(["-loglevel", "info", "-f", "avfoundation", "-list_devices", "true", "-i", ""]).output() else { return Vec::new() };
    parse_avfoundation_devices(&String::from_utf8_lossy(&out.stderr))
}

/// Parse `ffmpeg -f avfoundation -list_devices true` output (video section only).
pub fn parse_avfoundation_devices(text: &str) -> Vec<(usize, String)> {
    let mut in_video = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.contains("AVFoundation video devices") {
            in_video = true;
            continue;
        }
        if line.contains("AVFoundation audio devices") {
            break;
        }
        if !in_video {
            continue;
        }
        // "[AVFoundation indev @ 0x…] [1] Studio Display Camera"
        let Some(rest) = line.split_once("] [").map(|(_, r)| r) else { continue };
        let Some((idx, name)) = rest.split_once("] ") else { continue };
        let (Ok(idx), name) = (idx.parse::<usize>(), name.trim()) else { continue };
        if name.is_empty() || name.starts_with("Capture screen") {
            continue;
        }
        out.push((idx, name.to_string()));
    }
    out
}

/// The ffmpeg input that opens camera `id`: its current AVFoundation index (device names may
/// contain `:`, which the `video:audio` input syntax cannot take).
#[cfg(target_os = "macos")]
pub fn camera_input(id: &str) -> Option<String> {
    avfoundation_video_devices().into_iter().find(|(_, name)| name == id).map(|(i, _)| i.to_string())
}

/// Video capture devices from sysfs. Drivers register extra nodes (metadata) for the same
/// camera with a non-zero `index`; only index 0 delivers frames.
#[cfg(target_os = "linux")]
pub fn cameras() -> Vec<CameraInfo> {
    let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else { return Vec::new() };
    let mut out: Vec<CameraInfo> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("video"))
        .filter(|e| std::fs::read_to_string(e.path().join("index")).map(|s| s.trim() == "0").unwrap_or(true))
        .map(|e| {
            let node = e.file_name().to_string_lossy().into_owned();
            let name = std::fs::read_to_string(e.path().join("name")).map(|s| s.trim().to_string()).unwrap_or_else(|_| node.clone());
            CameraInfo { id: format!("/dev/{node}"), name, is_default: false }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(c) = out.first_mut() {
        c.is_default = true;
    }
    out
}

#[cfg(target_os = "linux")]
pub fn camera_input(id: &str) -> Option<String> {
    std::path::Path::new(id).exists().then(|| id.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn cameras() -> Vec<CameraInfo> {
    Vec::new()
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

    /// Microphones from cpal's ALSA host, reduced to what a person can choose between: ALSA
    /// lists every alias of a card (`hw:`, `plughw:`, `dsnoop:`, `sysdefault:` for each of
    /// its capture devices), all under the card's name, so one laptop card shows up ten times.
    /// See [`pick_alsa_inputs`].
    pub fn microphones() -> Vec<AudioDevice> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let Ok(devices) = host.input_devices() else { return Vec::new() };
        let entries: Vec<(String, String)> = devices
            .filter_map(|d| {
                let pcm = d.id().ok()?.id().to_string();
                let desc = d.description().ok().map(|d| d.name().to_string()).unwrap_or_default();
                Some((pcm, desc))
            })
            .collect();
        super::pick_alsa_inputs(&entries)
    }
}

/// Choose the ALSA PCMs worth offering as microphones. `entries` are `(pcm name, first line of
/// the ALSA description)` as cpal lists them.
///
/// With a sound server (`default` routing to PipeWire or PulseAudio, or a `pipewire`/`pulse`
/// PCM), that one entry is the whole list: the server owns the hardware and the person picks
/// the microphone in the system sound settings, while raw card PCMs would mostly fail to open
/// with "device busy". Without a server, one `plughw:` PCM per capture device is offered
/// (software conversions, so any rate works) and every other alias is dropped.
#[cfg(any(target_os = "linux", test))]
pub fn pick_alsa_inputs(entries: &[(String, String)]) -> Vec<AudioDevice> {
    let find = |pcm: &str| entries.iter().find(|(p, _)| p == pcm);
    let server_name = |desc: &str| {
        let d = desc.to_ascii_lowercase();
        if d.contains("pipewire") {
            Some("PipeWire")
        } else if d.contains("pulse") {
            Some("PulseAudio")
        } else {
            None
        }
    };
    let server = find("default")
        .and_then(|(_, desc)| server_name(desc).map(|s| ("default", s)))
        .or_else(|| find("pipewire").map(|_| ("pipewire", "PipeWire")))
        .or_else(|| find("pulse").map(|_| ("pulse", "PulseAudio")));
    if let Some((pcm, server)) = server {
        return vec![AudioDevice { id: pcm.to_string(), name: format!("System default ({server})"), is_default: true }];
    }
    let cards: Vec<(&str, &str, &str)> = entries
        .iter()
        .filter_map(|(pcm, desc)| {
            let rest = pcm.strip_prefix("plughw:CARD=")?;
            let (card, dev) = rest.split_once(",DEV=")?;
            Some((card, dev, desc.as_str()))
        })
        .collect();
    let mut out: Vec<AudioDevice> = cards
        .iter()
        .map(|(card, dev, desc)| {
            let base = desc.trim().trim_end_matches(',').trim();
            let base = if base.is_empty() { *card } else { base };
            let siblings = cards.iter().filter(|(c, _, _)| c == card).count();
            let name = if siblings > 1 { format!("{base} (device {dev})") } else { base.to_string() };
            AudioDevice { id: format!("plughw:CARD={card},DEV={dev}"), name, is_default: false }
        })
        .collect();
    if let Some(first) = out.first_mut() {
        first.is_default = true;
    }
    out
}

#[cfg(target_os = "windows")]
pub use windows_impl::{camera_input, cameras, displays, microphones, windows, windows_with};

/// Windows: monitors from `EnumDisplayMonitors` (points = pixels / DPI scale), top-level
/// windows from `EnumWindows`, microphones from cpal (WASAPI), cameras from ffmpeg's
/// DirectShow device list.
#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{AudioDevice, CameraInfo, DisplayInfo, WindowInfo};
    use crate::win;

    pub fn displays() -> Vec<DisplayInfo> {
        let mut out: Vec<DisplayInfo> = win::monitors()
            .into_iter()
            .map(|m| DisplayInfo {
                id: m.id,
                bounds_points: m.rect_points(),
                scale: m.scale,
                refresh_rate: m.refresh_rate,
                is_main: m.is_primary,
                // the built-in panel of a laptop has no better marker than being the first one
                is_builtin: false,
                name: if m.is_primary { format!("Display {} (primary)", m.id) } else { format!("Display {}", m.id) },
            })
            .collect();
        if !out.iter().any(|d| d.is_main) {
            if let Some(d) = out.first_mut() {
                d.is_main = true;
            }
        }
        out
    }

    pub fn windows_with(on_screen_only: bool) -> Vec<WindowInfo> {
        let me = std::process::id();
        win::windows()
            .into_iter()
            .filter(|w| w.pid != me && (!on_screen_only || !w.minimized))
            .map(|w| WindowInfo {
                id: win::id_from_hwnd(windows::Win32::Foundation::HWND(w.hwnd as *mut std::ffi::c_void)),
                bounds: w.rect_points(),
                is_on_screen: !w.minimized,
                pid: w.pid as i32,
                bundle_id: w.app.to_lowercase(),
                title: w.title,
                app_name: w.app,
            })
            .collect()
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

    /// Cameras as ffmpeg's DirectShow input lists them.
    pub fn cameras() -> Vec<CameraInfo> {
        let Ok(mut cmd) = crate::ffmpeg::ffmpeg_cmd() else { return Vec::new() };
        let Ok(out) = cmd.args(["-loglevel", "info", "-f", "dshow", "-list_devices", "true", "-i", "dummy"]).output() else { return Vec::new() };
        super::parse_dshow_devices(&String::from_utf8_lossy(&out.stderr))
            .into_iter()
            .enumerate()
            .map(|(i, name)| CameraInfo { id: name.clone(), name, is_default: i == 0 })
            .collect()
    }

    /// The DirectShow input for camera `id`.
    pub fn camera_input(id: &str) -> Option<String> {
        Some(format!("video={id}"))
    }
}

/// Parse `ffmpeg -f dshow -list_devices true` output: the names of the video devices.
pub fn parse_dshow_devices(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        // `[dshow @ 000001] "Integrated Camera" (video)`; older builds list sections instead
        let Some(rest) = line.split_once("] ").map(|(_, r)| r.trim()) else { continue };
        if !rest.starts_with('"') {
            continue;
        }
        let Some((name, kind)) = rest[1..].split_once('"') else { continue };
        if kind.trim() == "(video)" && !name.is_empty() && !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn displays() -> Vec<DisplayInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn windows_with(_on_screen_only: bool) -> Vec<WindowInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn windows() -> Vec<WindowInfo> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn microphones() -> Vec<AudioDevice> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alsa_inputs_collapse_to_the_sound_server() {
        let e = |p: &str, d: &str| (p.to_string(), d.to_string());
        let entries = vec![
            e("null", "Discard all samples (playback) or generate zero samples (capture)"),
            e("pipewire", "PipeWire Sound Server"),
            e("default", "Default ALSA Output (currently PipeWire Media Server)"),
            e("hw:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
            e("hw:CARD=sofhdadsp,DEV=6", "sof-hda-dsp, "),
            e("plughw:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
            e("plughw:CARD=sofhdadsp,DEV=6", "sof-hda-dsp, "),
            e("sysdefault:CARD=sofhdadsp", "sof-hda-dsp, "),
            e("dsnoop:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
        ];
        let got = pick_alsa_inputs(&entries);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "default");
        assert_eq!(got[0].name, "System default (PipeWire)");
        assert!(got[0].is_default);
    }

    #[test]
    fn alsa_inputs_without_a_server_list_one_plughw_per_capture_device() {
        let e = |p: &str, d: &str| (p.to_string(), d.to_string());
        let entries = vec![
            e("null", "Discard all samples"),
            e("hw:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
            e("plughw:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
            e("plughw:CARD=sofhdadsp,DEV=6", "sof-hda-dsp, "),
            e("dsnoop:CARD=sofhdadsp,DEV=0", "sof-hda-dsp, "),
            e("plughw:CARD=Yeti,DEV=0", "Blue Yeti, USB Audio"),
        ];
        let got = pick_alsa_inputs(&entries);
        let ids: Vec<&str> = got.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["plughw:CARD=sofhdadsp,DEV=0", "plughw:CARD=sofhdadsp,DEV=6", "plughw:CARD=Yeti,DEV=0"]);
        let names: Vec<&str> = got.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["sof-hda-dsp (device 0)", "sof-hda-dsp (device 6)", "Blue Yeti, USB Audio"]);
        assert!(got[0].is_default && !got[1].is_default);
    }

    #[test]
    fn parses_avfoundation_device_list() {
        let text = "[AVFoundation indev @ 0x1] AVFoundation video devices:
[AVFoundation indev @ 0x1] [0] FaceTime HD Camera
[AVFoundation indev @ 0x1] [1] Studio Display Camera
[AVFoundation indev @ 0x1] [2] Capture screen 0
[AVFoundation indev @ 0x1] AVFoundation audio devices:
[AVFoundation indev @ 0x1] [0] MacBook Pro Microphone
";
        assert_eq!(parse_avfoundation_devices(text), vec![(0, "FaceTime HD Camera".to_string()), (1, "Studio Display Camera".to_string())]);
    }

    #[test]
    fn parses_dshow_device_list() {
        let text = "[dshow @ 0000020] \"Integrated Camera\" (video)
[dshow @ 0000020]   Alternative name \"@device_pnp_\\\\?\\usb#vid_04f2\"
[dshow @ 0000020] \"OBS Virtual Camera\" (video)
[dshow @ 0000020] \"Microphone Array (Realtek)\" (audio)
dummy: Immediate exit requested
";
        assert_eq!(parse_dshow_devices(text), vec!["Integrated Camera".to_string(), "OBS Virtual Camera".to_string()]);
    }
}
