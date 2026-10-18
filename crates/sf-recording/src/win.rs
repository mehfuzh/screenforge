//! Win32 helpers shared by the Windows recorders: monitors (with their DPI scale and DXGI
//! output index), top-level windows, the system cursor's image, the text caret, moving other
//! apps' windows and hiding our own from the capture.
//!
//! Coordinates: Windows works in physical pixels of a virtual screen whose origin is the
//! primary monitor's top-left corner. The recorder's "points" are those pixels divided by the
//! DPI scale of the monitor they are on, which is the logical space winit/egui use to place
//! windows, so the recorded bounds, cursor positions and the app's own overlays agree.

use std::ffi::c_void;
use std::sync::OnceLock;

use sf_project::schema::common::Rect;
use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS, HDC, HMONITOR,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, GA_ROOT, GWL_EXSTYLE, GW_OWNER, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_RESTORE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_DISPLAY_AFFINITY, WS_EX_TOOLWINDOW,
};

/// `MONITORINFO.dwFlags` bit of the primary monitor.
const MONITORINFOF_PRIMARY: u32 = 1;

/// Make this process per-monitor DPI aware (once), so every Win32 call below reports physical
/// pixels. The app already is (winit sets it); command-line tools and tests are not.
pub fn ensure_dpi_aware() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // fails when the awareness was already set (by winit or a manifest): fine either way
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    });
}

/// A monitor as Windows reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub handle: isize,
    /// `\\.\DISPLAY<n>`
    pub device: String,
    /// Our display id: the `<n>` of the device name.
    pub id: u32,
    /// Virtual-screen pixels.
    pub rect_px: Rect,
    /// DPI / 96.
    pub scale: f64,
    pub refresh_rate: f64,
    pub is_primary: bool,
}

impl Monitor {
    /// Bounds in points (pixels / scale).
    pub fn rect_points(&self) -> Rect {
        Rect { x: self.rect_px.x / self.scale, y: self.rect_px.y / self.scale, width: self.rect_px.width / self.scale, height: self.rect_px.height / self.scale }
    }
}

fn wide(s: &[u16]) -> String {
    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

fn monitor_scale(h: HMONITOR) -> f64 {
    let (mut x, mut y) = (96u32, 96u32);
    if unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.is_err() || x == 0 {
        return 1.0;
    }
    (x as f64 / 96.0).max(0.5)
}

fn monitor_info(h: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(h, &mut info.monitorInfo) }.as_bool() {
        return None;
    }
    let device = wide(&info.szDevice);
    let id: u32 = device.trim_start_matches("\\\\.\\DISPLAY").parse().unwrap_or(0);
    let r = info.monitorInfo.rcMonitor;
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    let refresh_rate = if unsafe { EnumDisplaySettingsW(PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() && mode.dmDisplayFrequency > 1 {
        mode.dmDisplayFrequency as f64
    } else {
        60.0
    };
    Some(Monitor {
        handle: h.0 as isize,
        device,
        id,
        rect_px: Rect { x: r.left as f64, y: r.top as f64, width: (r.right - r.left) as f64, height: (r.bottom - r.top) as f64 },
        scale: monitor_scale(h),
        refresh_rate,
        is_primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
    })
}

/// All monitors, primary first.
pub fn monitors() -> Vec<Monitor> {
    ensure_dpi_aware();
    unsafe extern "system" fn cb(h: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        if let Some(m) = monitor_info(h) {
            out.push(m);
        }
        BOOL(1)
    }
    let mut out: Vec<Monitor> = Vec::new();
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut Vec<Monitor> as isize)) };
    out.sort_by_key(|m| (!m.is_primary, m.id));
    out
}

/// Scale of the monitor containing the physical point (nearest one outside every monitor).
pub fn scale_at(x_px: f64, y_px: f64) -> f64 {
    ensure_dpi_aware();
    let h = unsafe { MonitorFromPoint(POINT { x: x_px.round() as i32, y: y_px.round() as i32 }, MONITOR_DEFAULTTONEAREST) };
    monitor_scale(h)
}

/// Physical pixel position → points.
pub fn to_points(x_px: f64, y_px: f64) -> (f64, f64) {
    let s = scale_at(x_px, y_px);
    (x_px / s, y_px / s)
}

/// Current mouse position in points.
pub fn mouse_position() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some(to_points(p.x as f64, p.y as f64))
}

/// Index of the monitor's output on the default DXGI adapter (what ffmpeg's `ddagrab`
/// calls `output_idx`), when it has one.
pub fn dxgi_output_index(monitor_handle: isize) -> Option<u32> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let adapter = unsafe { factory.EnumAdapters1(0) }.ok()?;
    for i in 0..16 {
        let Ok(output) = (unsafe { adapter.EnumOutputs(i) }) else { break };
        let Ok(desc) = (unsafe { output.GetDesc() }) else { continue };
        if desc.Monitor.0 as isize == monitor_handle {
            return Some(i);
        }
    }
    None
}

/// A top-level window of another app.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub hwnd: isize,
    pub title: String,
    /// Executable name without extension (e.g. `chrome`).
    pub app: String,
    pub pid: u32,
    pub minimized: bool,
    /// Physical pixels of the visible frame (without the invisible resize borders).
    pub rect_px: Rect,
    /// Scale of the monitor the window is on.
    pub scale: f64,
}

impl Window {
    pub fn rect_points(&self) -> Rect {
        Rect { x: self.rect_px.x / self.scale, y: self.rect_px.y / self.scale, width: self.rect_px.width / self.scale, height: self.rect_px.height / self.scale }
    }
}

/// Our 32-bit window id ↔ `HWND` (handles are 32-bit values sign-extended on 64-bit Windows).
pub fn hwnd_from_id(id: u32) -> HWND {
    HWND((id as i32) as isize as *mut c_void)
}
pub fn id_from_hwnd(h: HWND) -> u32 {
    h.0 as isize as u32
}

/// Visible frame of a window in physical pixels (DWM's extended frame bounds, falling back to
/// `GetWindowRect`).
pub fn window_frame_px(h: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    let ok = unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32) }.is_ok()
        || unsafe { GetWindowRect(h, &mut r) }.is_ok();
    ok.then(|| Rect { x: r.left as f64, y: r.top as f64, width: (r.right - r.left) as f64, height: (r.bottom - r.top) as f64 })
}

fn is_cloaked(h: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4) }.is_ok() && cloaked != 0
}

fn window_title(h: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(h) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Executable file stem of a process.
pub fn process_name(pid: u32) -> String {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return String::new() };
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let name = if unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok() {
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        String::new()
    };
    let _ = unsafe { CloseHandle(handle) };
    name
}

/// Top-level app windows in z-order (front to back): visible, not cloaked (other virtual
/// desktops, UWP shells), not tool windows, with a title and a real frame.
pub fn windows() -> Vec<Window> {
    ensure_dpi_aware();
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<Window>) };
        if let Some(w) = unsafe { describe(h) } {
            out.push(w);
        }
        BOOL(1)
    }
    unsafe fn describe(h: HWND) -> Option<Window> {
        if !unsafe { IsWindowVisible(h) }.as_bool() || is_cloaked(h) {
            return None;
        }
        let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        // owned windows (dialogs, tooltips) belong to their owner
        if !unsafe { GetWindow(h, GW_OWNER) }.map(|o| o.0.is_null()).unwrap_or(true) {
            return None;
        }
        if unsafe { GetAncestor(h, GA_ROOT) } != h {
            return None;
        }
        let title = window_title(h);
        if title.is_empty() {
            return None;
        }
        let rect_px = window_frame_px(h)?;
        if rect_px.width < 2.0 || rect_px.height < 2.0 {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        let minimized = unsafe { IsIconic(h) }.as_bool();
        let mon = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY) };
        Some(Window { hwnd: h.0 as isize, title, app: process_name(pid), pid, minimized, rect_px, scale: monitor_scale(mon) })
    }
    let mut out: Vec<Window> = Vec::new();
    let _ = unsafe { EnumWindows(Some(cb), LPARAM(&mut out as *mut Vec<Window> as isize)) };
    out
}

/// Top-level windows of this process (the app's own toolbar, overlays, panels).
pub fn own_windows() -> Vec<u32> {
    let me = std::process::id();
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        let (me, out) = unsafe { &mut *(data.0 as *mut (u32, Vec<u32>)) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        if pid == *me && unsafe { GetAncestor(h, GA_ROOT) } == h {
            out.push(id_from_hwnd(h));
        }
        BOOL(1)
    }
    let mut data = (me, Vec::new());
    let _ = unsafe { EnumWindows(Some(cb), LPARAM(&mut data as *mut (u32, Vec<u32>) as isize)) };
    data.1
}

/// Hide (or show again) a window from screen capture (`WDA_EXCLUDEFROMCAPTURE`, Windows 10 2004+).
pub fn set_excluded_from_capture(id: u32, excluded: bool) -> bool {
    let affinity: WINDOW_DISPLAY_AFFINITY = if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    unsafe { SetWindowDisplayAffinity(hwnd_from_id(id), affinity) }.is_ok()
}

/// Bring a window to the front (restoring it when minimized).
pub fn activate(id: u32) -> bool {
    let h = hwnd_from_id(id);
    if unsafe { IsIconic(h) }.as_bool() {
        let _ = unsafe { ShowWindow(h, SW_RESTORE) };
    }
    unsafe { SetForegroundWindow(h) }.as_bool()
}

/// Move and resize the window of `pid` whose frame is closest to `current` to `target` (both
/// in points). The invisible resize borders DWM adds around the visible frame are kept.
pub fn set_window_frame(pid: i32, current: Rect, target: Rect) -> bool {
    let candidates: Vec<Window> = windows().into_iter().filter(|w| w.pid == pid as u32).collect();
    let best = candidates.into_iter().min_by(|a, b| {
        let d = |w: &Window| {
            let r = w.rect_points();
            (r.x - current.x).abs() + (r.y - current.y).abs() + (r.width - current.width).abs() + (r.height - current.height).abs()
        };
        d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let Some(w) = best else { return false };
    let h = HWND(w.hwnd as *mut c_void);
    let mut outer = RECT::default();
    if unsafe { GetWindowRect(h, &mut outer) }.is_err() {
        return false;
    }
    // outer (GetWindowRect) minus visible frame: the invisible borders on each side
    let (dl, dt) = (outer.left as f64 - w.rect_px.x, outer.top as f64 - w.rect_px.y);
    let (dr, db) = (outer.right as f64 - (w.rect_px.x + w.rect_px.width), outer.bottom as f64 - (w.rect_px.y + w.rect_px.height));
    let s = w.scale;
    let x = (target.x * s + dl).round() as i32;
    let y = (target.y * s + dt).round() as i32;
    let cx = (target.width * s - dl + dr).round() as i32;
    let cy = (target.height * s - dt + db).round() as i32;
    unsafe { SetWindowPos(h, None, x, y, cx, cy, SWP_NOZORDER | SWP_NOACTIVATE) }.is_ok()
}

/// The text caret of the focused window in points (classic Win32 controls report it; many
/// browser and Electron apps do not).
pub fn caret_position() -> Option<(f64, f64)> {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{GetGUIThreadInfo, GUITHREADINFO};
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    unsafe { GetGUIThreadInfo(0, &mut info) }.ok()?;
    if info.hwndCaret.0.is_null() {
        return None;
    }
    let r = info.rcCaret;
    let mut p = POINT { x: r.left, y: r.top };
    if !unsafe { ClientToScreen(info.hwndCaret, &mut p) }.as_bool() {
        return None;
    }
    let h = (r.bottom - r.top).max(1) as f64;
    Some(to_points(p.x as f64 + (r.right - r.left) as f64 / 2.0, p.y as f64 + h / 2.0))
}

/// A screenshot of a physical-pixel rectangle of the virtual screen, as opaque RGBA (what the
/// area/window overlays show behind themselves, since their windows cannot be transparent on
/// Direct3D surfaces).
pub fn screenshot_px(x: i32, y: i32, w: i32, h: i32) -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Graphics::Gdi::{BitBlt, GetDC, ReleaseDC, CAPTUREBLT, SRCCOPY};
    if w <= 0 || h <= 0 {
        return None;
    }
    ensure_dpi_aware();
    let screen = unsafe { GetDC(None) };
    if screen.0.is_null() {
        return None;
    }
    let result = (|| {
        let dib = cursor::Dib::new(screen, w, h)?;
        unsafe { BitBlt(dib.dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT) }.ok()?;
        let src = dib.pixels();
        let mut rgba = Vec::with_capacity(src.len());
        for px in src.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Some(rgba)
    })();
    let _ = unsafe { ReleaseDC(None::<HWND>, screen) };
    result.map(|rgba| (w as u32, h as u32, rgba))
}

/// The system cursor: its image rendered to straight-alpha RGBA, hot spot and the handle that
/// identifies it cheaply between polls.
pub mod cursor {
    use std::ffi::c_void;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetObjectW, ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfoExW, LoadCursorW, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFOEXW, IDC_APPSTARTING, IDC_ARROW,
        IDC_CROSS, IDC_HAND, IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, IDC_UPARROW, IDC_WAIT,
    };

    /// `(Screen Studio id, system cursor)` for the standard cursors, so recordings use the
    /// same ids as macOS ones where the shapes correspond.
    pub const KNOWN: &[(&str, windows::core::PCWSTR)] = &[
        ("arrow", IDC_ARROW),
        ("iBeam", IDC_IBEAM),
        ("pointingHand", IDC_HAND),
        ("crosshair", IDC_CROSS),
        ("resizeLeftRight", IDC_SIZEWE),
        ("resizeUpDown", IDC_SIZENS),
        ("windowResizeNorthWestSouthEast", IDC_SIZENWSE),
        ("windowResizeNorthEastSouthWest", IDC_SIZENESW),
        ("openHand", IDC_SIZEALL),
        ("operationNotAllowed", IDC_NO),
        ("busyButClickable", IDC_APPSTARTING),
        ("wait", IDC_WAIT),
        ("help", IDC_HELP),
        ("upArrow", IDC_UPARROW),
    ];

    /// The image of a cursor.
    #[derive(Debug, Clone)]
    pub struct Image {
        pub width: usize,
        pub height: usize,
        /// Straight-alpha RGBA.
        pub rgba: Vec<u8>,
        /// Hot spot in pixels.
        pub hot_x: f64,
        pub hot_y: f64,
    }

    /// `(handle, visible)` of the cursor shown right now.
    pub fn current() -> Option<(isize, bool)> {
        let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        unsafe { GetCursorInfo(&mut info) }.ok()?;
        Some((info.hCursor.0 as isize, info.flags == CURSOR_SHOWING))
    }

    /// Load a standard cursor.
    pub fn load(id: windows::core::PCWSTR) -> Option<isize> {
        unsafe { LoadCursorW(None, id) }.ok().map(|h| h.0 as isize)
    }

    pub(super) struct Dib {
        pub(super) dc: HDC,
        bmp: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u8,
        len: usize,
    }

    impl Dib {
        pub(super) fn new(screen: HDC, w: i32, h: i32) -> Option<Dib> {
            let dc = unsafe { CreateCompatibleDC(Some(screen)) };
            if dc.0.is_null() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = match unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) } {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = unsafe { DeleteDC(dc) };
                    return None;
                }
            };
            let old = unsafe { SelectObject(dc, HGDIOBJ(bmp.0)) };
            Some(Dib { dc, bmp, old, bits: bits as *mut u8, len: (w * h * 4) as usize })
        }

        fn fill(&self, v: u8) {
            unsafe { std::ptr::write_bytes(self.bits, v, self.len) };
        }

        pub(super) fn pixels(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.bits, self.len) }
        }
    }

    impl Drop for Dib {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(HGDIOBJ(self.bmp.0));
                let _ = DeleteDC(self.dc);
            }
        }
    }

    /// Render cursor `handle` (an `HCURSOR`) to RGBA. Drawn over black and over white so both
    /// alpha-blended and classic mask (XOR) cursors come out with a usable alpha channel.
    pub fn image(handle: isize) -> Option<Image> {
        let icon = HICON(handle as *mut c_void);
        let mut info = ICONINFOEXW { cbSize: std::mem::size_of::<ICONINFOEXW>() as u32, ..Default::default() };
        if !unsafe { GetIconInfoExW(icon, &mut info) }.as_bool() {
            return None;
        }
        // the bitmaps are ours to free
        let color = info.hbmColor;
        let mask = info.hbmMask;
        let dims = |bmp: HBITMAP| -> Option<(i32, i32)> {
            if bmp.0.is_null() {
                return None;
            }
            let mut b = BITMAP::default();
            let n = unsafe { GetObjectW(HGDIOBJ(bmp.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut b as *mut BITMAP as *mut c_void)) };
            (n > 0).then_some((b.bmWidth, b.bmHeight))
        };
        let (w, h) = match (dims(color), dims(mask)) {
            (Some(d), _) => d,
            // a mask-only cursor stacks the AND and XOR masks vertically
            (None, Some((mw, mh))) => (mw, mh / 2),
            (None, None) => (0, 0),
        };
        if !color.0.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
        }
        if !mask.0.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(mask.0)) };
        }
        if w <= 0 || h <= 0 || w > 512 || h > 512 {
            return None;
        }
        let screen = unsafe { GetDC(None) };
        if screen.0.is_null() {
            return None;
        }
        let result = (|| {
            let black = Dib::new(screen, w, h)?;
            let white = Dib::new(screen, w, h)?;
            black.fill(0);
            white.fill(255);
            unsafe { DrawIconEx(black.dc, 0, 0, icon, w, h, 0, None, DI_NORMAL) }.ok()?;
            unsafe { DrawIconEx(white.dc, 0, 0, icon, w, h, 0, None, DI_NORMAL) }.ok()?;
            let (b, wh) = (black.pixels(), white.pixels());
            let mut rgba = Vec::with_capacity(b.len());
            for (pb, pw) in b.chunks_exact(4).zip(wh.chunks_exact(4)) {
                // BGRA from GDI; alpha = 1 - (white - black)
                let a = 255u32.saturating_sub((pw[0] as u32 + pw[1] as u32 + pw[2] as u32).saturating_sub(pb[0] as u32 + pb[1] as u32 + pb[2] as u32) / 3);
                let un = |c: u8| if a > 0 { ((c as u32 * 255 + a / 2) / a).min(255) as u8 } else { 0 };
                rgba.extend_from_slice(&[un(pb[2]), un(pb[1]), un(pb[0]), a as u8]);
            }
            Some(rgba)
        })();
        let _ = unsafe { ReleaseDC(None::<HWND>, screen) };
        let rgba = result?;
        Some(Image { width: w as usize, height: h as usize, rgba, hot_x: info.xHotspot as f64, hot_y: info.yHotspot as f64 })
    }
}
