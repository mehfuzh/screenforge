//! Hand-offs to the desktop: open a link or folder, show a file in the file manager, move a
//! file to the Trash. macOS uses `open` and Foundation; Linux uses the freedesktop tools;
//! Windows the shell (`ShellExecute`, Explorer, the Recycle Bin).

use std::path::Path;

/// Name of the system file manager, for button labels.
pub const FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Finder"
} else if cfg!(target_os = "windows") {
    "Explorer"
} else {
    "Files"
};

/// Open a URL, file or folder with its default application.
pub fn open(target: impl AsRef<std::ffi::OsStr>) {
    #[cfg(target_os = "windows")]
    {
        windows_shell::open(target.as_ref());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(program).arg(target).spawn();
    }
}

#[cfg(target_os = "windows")]
mod windows_shell {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn open(target: &OsStr) {
        let verb = wide(OsStr::new("open"));
        let file = wide(target);
        // SAFETY: both strings are NUL-terminated and outlive the call
        let _ = unsafe { ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(file.as_ptr()), None, None, SW_SHOWNORMAL) };
    }

    /// Explorer with the file selected.
    pub fn reveal(path: &Path) {
        let mut c = std::process::Command::new("explorer.exe");
        // Explorer wants `/select,<path>` as one argument, quoted as a whole when it has spaces
        use std::os::windows::process::CommandExt;
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        let _ = c.spawn();
    }

    /// Recycle Bin via the shell (undoable from Explorer).
    pub fn recycle(path: &Path) -> Result<(), String> {
        // a plain absolute path: the shell takes neither relative nor `\\?\` ones
        let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
        let text = abs.to_string_lossy().trim_start_matches(r"\\?\").replace('/', r"\");
        let text = text.trim_end_matches('\\').to_string();
        // double NUL-terminated list
        let mut from: Vec<u16> = OsStr::new(&text).encode_wide().collect();
        from.extend([0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: PCWSTR(from.as_ptr()),
            fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
            ..Default::default()
        };
        let r = unsafe { SHFileOperationW(&mut op) };
        if r == 0 && !op.fAnyOperationsAborted.as_bool() {
            Ok(())
        } else {
            Err(format!("cannot move to the Recycle Bin (shell error {r:#x} for {text})"))
        }
    }
}

/// Show `path` selected in the file manager (Linux: `org.freedesktop.FileManager1`, else its
/// folder is opened).
pub fn reveal(path: &Path) {
    // NSWorkspace, not `open -R`: that sends Finder an Apple Event, which the hardened runtime
    // blocks without the automation entitlement
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSArray, NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }
    #[cfg(target_os = "windows")]
    windows_shell::reveal(path);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let shown = std::process::Command::new("gdbus")
                .args(["call", "--session", "--dest", "org.freedesktop.FileManager1", "--object-path", "/org/freedesktop/FileManager1"])
                .args(["--method", "org.freedesktop.FileManager1.ShowItems"])
                .arg(format!("['{}']", file_uri(&path)))
                .arg("")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !shown {
                open(path.parent().unwrap_or(&path));
            }
        });
    }
}

/// `file://` URI for egui's image loader. Windows paths need the `file:///C:/…` form: with
/// `file://C:\…` the loader takes `C:` for a network host.
pub fn texture_uri(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let p = p.trim_start_matches("//?/");
    if cfg!(windows) && !p.starts_with('/') { format!("file:///{p}") } else { format!("file://{p}") }
}

/// `file://` URI of an absolute path, percent-encoding everything but unreserved characters
/// and `/` (also keeps the URI safe inside a GVariant string).
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Move a file or folder to the Trash, where it can be restored ("Put Back" in Finder,
/// "Restore" in Files and the Recycle Bin). Never deletes: if the Trash cannot take it, it stays put.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let fm = NSFileManager::defaultManager();
        fm.trashItemAtURL_resultingItemURL_error(&url, None).map(|_| ()).map_err(|e| e.localizedDescription().to_string())
    }
    #[cfg(target_os = "windows")]
    {
        windows_shell::recycle(path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let gio = std::process::Command::new("gio").arg("trash").arg(path).output();
        match gio {
            Ok(o) if o.status.success() => Ok(()),
            // no gio (or it failed): the freedesktop Trash in the home directory
            gio => home_trash(path).map_err(|e| match gio {
                Ok(o) => format!("{} ({e})", String::from_utf8_lossy(&o.stderr).trim()),
                Err(_) => e,
            }),
        }
    }
}

/// Move `path` into `$XDG_DATA_HOME/Trash` per the freedesktop Trash spec. From another file
/// system (e.g. a tmpfs `/tmp`, which `gio` refuses) it is copied in first, and the original is
/// removed only once the copy is complete.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn home_trash(path: &Path) -> Result<(), String> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .ok_or("no home directory")?;
    let (files, info) = (data.join("Trash/files"), data.join("Trash/info"));
    std::fs::create_dir_all(&files).and_then(|_| std::fs::create_dir_all(&info)).map_err(|e| e.to_string())?;
    let abs = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let name = abs.file_name().ok_or("no file name")?.to_string_lossy().into_owned();
    // a free name in the Trash: "x", "x.2", "x.3", …
    let mut n = 1;
    let pick = loop {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        if !files.join(&candidate).exists() && !info.join(format!("{candidate}.trashinfo")).exists() {
            break candidate;
        }
        n += 1;
    };
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let encoded = file_uri(&abs).trim_start_matches("file://").to_string();
    let info_file = info.join(format!("{pick}.trashinfo"));
    std::fs::write(&info_file, format!("[Trash Info]\nPath={encoded}\nDeletionDate={stamp}\n")).map_err(|e| e.to_string())?;
    let dest = files.join(&pick);
    let moved = match std::fs::rename(&abs, &dest) {
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => copy_tree(&abs, &dest)
            .inspect_err(|_| {
                let _ = remove_tree(&dest);
            })
            .and_then(|_| remove_tree(&abs)),
        r => r,
    };
    moved.map_err(|e| {
        if !abs.exists() {
            // the copy is complete and only part of the original went: keep the Trash entry
            return format!("moved to the Trash, but some of the original remains: {e}");
        }
        if std::fs::symlink_metadata(&dest).is_err() {
            let _ = std::fs::remove_file(&info_file);
        }
        format!("cannot move to the Trash: {e}")
    })
}

/// Copy a file, symlink or folder tree (symlinks are copied as links, not followed).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `f` with `XDG_DATA_HOME` set (tests run in parallel and share the environment).
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn with_data_home<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("XDG_DATA_HOME", dir);
        f()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recycle_bin_takes_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("My Project.screenforge");
        std::fs::create_dir_all(p.join("recording")).unwrap();
        std::fs::write(p.join("recording/a.txt"), b"x").unwrap();
        move_to_trash(&p).unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn texture_uris_use_the_slash_form() {
        if cfg!(windows) {
            assert_eq!(texture_uri(Path::new(r"C:\Users\me\a b.jpg")), "file:///C:/Users/me/a b.jpg");
            assert_eq!(texture_uri(Path::new(r"\\?\C:\x\a.jpg")), "file:///C:/x/a.jpg");
        } else {
            assert_eq!(texture_uri(Path::new("/home/me/a.jpg")), "file:///home/me/a.jpg");
        }
    }

    #[test]
    fn file_uris_are_encoded() {
        assert_eq!(file_uri(Path::new("/home/me/Movies/Demo 1.mp4")), "file:///home/me/Movies/Demo%201.mp4");
        assert_eq!(file_uri(Path::new("/tmp/it's.mp4")), "file:///tmp/it%27s.mp4");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn home_trash_moves_and_records_the_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("My Project.screenforge");
        std::fs::create_dir_all(p.join("recording")).unwrap();
        let data = tmp.path().join("data");
        with_data_home(&data, || home_trash(&p)).unwrap();
        assert!(!p.exists());
        assert!(tmp.path().join("data/Trash/files/My Project.screenforge/recording").is_dir());
        let info = std::fs::read_to_string(tmp.path().join("data/Trash/info/My Project.screenforge.trashinfo")).unwrap();
        assert!(info.contains("Path=") && info.contains("My%20Project.screenforge"), "{info}");
        // a second item with the same name gets a free name
        std::fs::create_dir_all(&p).unwrap();
        with_data_home(&data, || home_trash(&p)).unwrap();
        assert!(tmp.path().join("data/Trash/files/My Project.screenforge.2").is_dir());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn home_trash_copies_across_file_systems() {
        // a tmpfs /tmp and a Trash under the home directory are different devices
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap();
        let src = tempfile::tempdir_in("/tmp").unwrap();
        let data = tempfile::tempdir_in(&home).unwrap();
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(src.path()).unwrap().dev() == std::fs::metadata(data.path()).unwrap().dev() {
            return; // same file system here: covered by the test above
        }
        let p = src.path().join("Cross.screenforge");
        std::fs::create_dir_all(p.join("recording")).unwrap();
        std::fs::write(p.join("recording/a.mp4"), b"video").unwrap();
        std::os::unix::fs::symlink("recording/a.mp4", p.join("link")).unwrap();
        with_data_home(data.path(), || home_trash(&p)).unwrap();
        assert!(!p.exists());
        let t = data.path().join("Trash/files/Cross.screenforge");
        assert_eq!(std::fs::read(t.join("recording/a.mp4")).unwrap(), b"video");
        assert_eq!(std::fs::read_link(t.join("link")).unwrap(), Path::new("recording/a.mp4"));
        assert!(data.path().join("Trash/info/Cross.screenforge.trashinfo").is_file());
    }
}


/// Shape of the floating toolbar window on Windows. A wgpu Direct3D surface has no alpha
/// channel, so a "transparent" window shows black where nothing is painted; clipping the
/// window to the pill (and whatever menu or panel is open above it) with a window region
/// shows the desktop there instead, and clicks outside go to the windows below.
#[cfg(target_os = "windows")]
pub struct WindowShape {
    hwnd: isize,
    /// Last region sent, in window pixels (`None` = the whole window).
    applied: Option<Option<Vec<[i32; 5]>>>,
}

#[cfg(target_os = "windows")]
impl WindowShape {
    pub fn new(handle: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        use raw_window_handle::RawWindowHandle;
        let hwnd = match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => return None,
        };
        Some(WindowShape { hwnd, applied: None })
    }

    /// Show only `rects` (window points, each with a corner radius), or the whole window for `None`.
    pub fn set(&mut self, rects: Option<&[(egui::Rect, f32)]>, pixels_per_point: f32) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ, HRGN, RGN_OR};
        let px: Option<Vec<[i32; 5]>> = rects.map(|rs| {
            rs.iter()
                .filter(|(r, _)| r.is_positive())
                .map(|(r, radius)| {
                    let (x0, y0) = ((r.left() * pixels_per_point).floor() as i32, (r.top() * pixels_per_point).floor() as i32);
                    let (x1, y1) = ((r.right() * pixels_per_point).ceil() as i32, (r.bottom() * pixels_per_point).ceil() as i32);
                    [x0, y0, x1.max(x0 + 1), y1.max(y0 + 1), (radius * 2.0 * pixels_per_point).round() as i32]
                })
                .collect()
        });
        if self.applied.as_ref() == Some(&px) {
            return;
        }
        let hwnd = HWND(self.hwnd as *mut std::ffi::c_void);
        let ok = match &px {
            None => (unsafe { SetWindowRgn(hwnd, None, true) }) != 0,
            Some(list) => {
                let mut region: Option<HRGN> = None;
                for [x0, y0, x1, y1, d] in list {
                    let r = unsafe { CreateRoundRectRgn(*x0, *y0, *x1, *y1, *d, *d) };
                    if r.0.is_null() {
                        continue;
                    }
                    region = Some(match region {
                        None => r,
                        Some(acc) => {
                            unsafe { CombineRgn(Some(acc), Some(acc), Some(r), RGN_OR) };
                            let _ = unsafe { DeleteObject(HGDIOBJ(r.0)) };
                            acc
                        }
                    });
                }
                match region {
                    // the window owns the region after SetWindowRgn
                    Some(r) => (unsafe { SetWindowRgn(hwnd, Some(r), true) }) != 0,
                    None => false,
                }
            }
        };
        if ok {
            self.applied = Some(px);
        }
    }
}

/// Input region of a transparent X11 window: outside it, clicks go to the windows below. The
/// floating toolbar keeps one size on Linux (resizing and moving an X11 window are two steps,
/// so the bar jumped whenever a menu opened) and narrows its input to the bar instead.
#[cfg(target_os = "linux")]
pub struct InputRegion {
    conn: x11rb::rust_connection::RustConnection,
    window: u32,
    /// Last region sent, in window pixels (`None` = the whole window).
    applied: Option<Option<[i32; 4]>>,
}

#[cfg(target_os = "linux")]
impl InputRegion {
    /// `None` unless the window is an X11 window with a reachable X server.
    pub fn new(handle: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        use raw_window_handle::RawWindowHandle;
        let window = match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => h.window as u32,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => return None,
        };
        let (conn, _) = x11rb::connect(None).ok()?;
        Some(InputRegion { conn, window, applied: None })
    }

    /// Take input only inside `rect` (window points), or everywhere for `None`.
    pub fn set(&mut self, rect: Option<egui::Rect>, pixels_per_point: f32) {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
        use x11rb::protocol::xproto::{ClipOrdering, Rectangle};
        let px = rect.map(|r| {
            let (x0, y0) = ((r.left() * pixels_per_point).floor() as i32, (r.top() * pixels_per_point).floor() as i32);
            let (x1, y1) = ((r.right() * pixels_per_point).ceil() as i32, (r.bottom() * pixels_per_point).ceil() as i32);
            [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
        });
        if self.applied == Some(px) {
            return;
        }
        let sent = match px {
            Some([x, y, w, h]) => {
                let r = Rectangle { x: x as i16, y: y as i16, width: w as u16, height: h as u16 };
                self.conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, self.window, 0, 0, &[r]).is_ok()
            }
            // no mask: back to the window's full shape
            None => self.conn.shape_mask(SO::SET, SK::INPUT, self.window, 0, 0, x11rb::NONE).is_ok(),
        };
        if sent && self.conn.flush().is_ok() {
            self.applied = Some(px);
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod recycle_real {
    /// Recycles a copy of the newest project in the user's projects folder, the way the app does
    /// (`cargo test -p studio recycles_a_project_copy -- --ignored`).
    #[test]
    #[ignore = "touches the real projects folder"]
    fn recycles_a_project_copy() {
        let folder = sf_session::create::default_projects_folder();
        let newest = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "screenforge")).max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok()).expect("a project");
        let copy = folder.join("Recycle Test 12.34.56.screenforge");
        let _ = std::fs::remove_dir_all(&copy);
        std::fs::create_dir_all(copy.join("recording")).unwrap();
        for f in ["project.json", "meta.json"] {
            if newest.join(f).exists() {
                std::fs::copy(newest.join(f), copy.join(f)).unwrap();
            }
        }
        let r = super::move_to_trash(&copy);
        assert!(r.is_ok(), "{r:?}");
        assert!(!copy.exists());
    }
}
