//! Cursor recorder: `cursors.json` + `cursors/<id>.png`.
//!
//! Like polyrecorder, all public and private `NSCursor` class cursors are preloaded at
//! start (deduplicated by their TIFF bytes, public names winning), saved as PNG (largest
//! representation) with `hotSpot`/`standardSize` in points, and the system cursor is polled
//! at ~60 Hz through `NSCursor.currentSystemCursor`. Each distinct image is identified by
//! the hash of its TIFF representation; unknown images get `custom-<hash>` ids with
//! `systemCursor: false`. Cursor changes are reported to the input recorder so it can
//! synthesize `mouseMoved` events carrying the new `cursorId`.

use std::path::Path;

use sf_project::recording::CursorInfo;

/// FNV-1a 64-bit (deterministic across runs, unlike `DefaultHasher` keys in the future).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Serialize `cursors.json` exactly like polyrecorder: one object per line, sorted keys.
pub fn write_cursors_json(dir: &Path, cursors: &[CursorInfo]) -> std::io::Result<()> {
    let mut out = String::from("[\n");
    for (i, c) in cursors.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        let v = crate::metadata::sort_keys(serde_json::to_value(c).map_err(std::io::Error::other)?);
        out.push_str(&serde_json::to_string(&v).map_err(std::io::Error::other)?);
    }
    out.push_str("\n]");
    std::fs::write(dir.join("cursors.json"), out)
}

#[cfg(target_os = "macos")]
pub use imp::CursorRecorder;
#[cfg(target_os = "linux")]
pub use linux::CursorRecorder;
#[cfg(target_os = "windows")]
pub use windows_impl::CursorRecorder;

/// Windows: the standard cursors are preloaded (same ids as the macOS catalogue where the
/// shapes correspond) and the shown cursor is polled at ~60 Hz through `GetCursorInfo`; a
/// cursor image not seen before gets a `custom-<hash>` id.
#[cfg(target_os = "windows")]
mod windows_impl {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use sf_project::recording::CursorInfo;
    use sf_project::schema::common::{Point, Size};

    use super::{fnv1a64, write_cursors_json};
    use crate::log::RecLog;
    use crate::win::cursor as wc;
    use crate::{Error, Result};

    const COMPONENT: &str = "CursorRecorder";

    struct Shared {
        dir: PathBuf,
        log: RecLog,
        /// Pixels per point of the primary monitor (cursor bitmaps follow its DPI).
        scale: f64,
        /// image hash → id
        by_hash: Mutex<HashMap<u64, Arc<str>>>,
        /// cursor handle → id (handles of standard cursors are stable; others are re-hashed)
        by_handle: Mutex<HashMap<isize, Arc<str>>>,
        infos: Mutex<Vec<CursorInfo>>,
        current: Mutex<Arc<str>>,
        stop: AtomicBool,
    }

    fn encode_png(img: &wc::Image) -> Option<Vec<u8>> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out).write_image(&img.rgba, img.width as u32, img.height as u32, image::ExtendedColorType::Rgba8).ok()?;
        Some(out)
    }

    impl Shared {
        /// Save `img` as `id` unless the image (or id) is known. Returns the id now standing
        /// for this image.
        fn register(&self, id: &str, system: bool, img: &wc::Image, hash: u64) -> Result<Arc<str>> {
            {
                let mut by_hash = self.by_hash.lock();
                if let Some(existing) = by_hash.get(&hash) {
                    return Ok(existing.clone());
                }
                if self.infos.lock().iter().any(|c| c.id == id) {
                    return Ok(Arc::from(id));
                }
                by_hash.insert(hash, Arc::from(id));
            }
            let png = encode_png(img).ok_or_else(|| Error::Capture(format!("cursor {id}: png encode failed")))?;
            let path = self.dir.join("cursors").join(format!("{id}.png"));
            std::fs::write(&path, png).map_err(|source| Error::Io { path, source })?;
            let mut infos = self.infos.lock();
            infos.push(CursorInfo {
                id: id.to_string(),
                hot_spot: Point::new(img.hot_x / self.scale, img.hot_y / self.scale),
                standard_size: Size { width: img.width as f64 / self.scale, height: img.height as f64 / self.scale },
                system_cursor: system,
                extra: Default::default(),
            });
            write_cursors_json(&self.dir, &infos).map_err(|source| Error::Io { path: self.dir.join("cursors.json"), source })?;
            Ok(Arc::from(id))
        }
    }

    pub struct CursorRecorder {
        shared: Arc<Shared>,
        thread: Option<JoinHandle<()>>,
    }

    impl CursorRecorder {
        pub fn start(dir: &Path, log: RecLog, on_change: Box<dyn Fn(&str) + Send>) -> Result<Self> {
            let cursors_dir = dir.join("cursors");
            std::fs::create_dir_all(&cursors_dir).map_err(|source| Error::Io { path: cursors_dir.clone(), source })?;
            let scale = crate::win::monitors().into_iter().find(|m| m.is_primary).map(|m| m.scale).unwrap_or(1.0);
            let shared = Arc::new(Shared {
                dir: dir.to_path_buf(),
                log: log.clone(),
                scale,
                by_hash: Mutex::new(HashMap::new()),
                by_handle: Mutex::new(HashMap::new()),
                infos: Mutex::new(Vec::new()),
                current: Mutex::new(Arc::from("arrow")),
                stop: AtomicBool::new(false),
            });
            let mut loaded = 0usize;
            for (id, res) in wc::KNOWN {
                let Some(handle) = wc::load(*res) else { continue };
                let Some(img) = wc::image(handle) else { continue };
                loaded += 1;
                match shared.register(id, true, &img, fnv1a64(&img.rgba)) {
                    Ok(stored) => {
                        shared.by_handle.lock().insert(handle, stored);
                    }
                    Err(e) => log.warning(COMPONENT, format!("Could not save cursor {id}: {e}")),
                }
            }
            if shared.infos.lock().is_empty() {
                return Err(Error::Capture("could not render any system cursor".into()));
            }
            log.notice(COMPONENT, format!("Preloaded system cursors. loaded={loaded} saved={} scale={scale}", shared.infos.lock().len()));
            let thread_shared = shared.clone();
            let thread = std::thread::Builder::new()
                .name("sf-cursor-poll".into())
                .spawn(move || poll_loop(thread_shared, on_change))
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            Ok(CursorRecorder { shared, thread: Some(thread) })
        }

        pub fn current_id(&self) -> Arc<str> {
            self.shared.current.lock().clone()
        }

        pub fn cursors(&self) -> Vec<CursorInfo> {
            self.shared.infos.lock().clone()
        }

        pub fn stop(&mut self) {
            self.shared.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for CursorRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn poll_loop(shared: Arc<Shared>, on_change: Box<dyn Fn(&str) + Send>) {
        let interval = Duration::from_micros(16_667);
        let mut last_handle: Option<isize> = None;
        let mut next = Instant::now();
        while !shared.stop.load(Ordering::Relaxed) {
            next += interval;
            if let Some((handle, visible)) = wc::current() {
                // a hidden cursor (typing) keeps its last id, like macOS
                if visible && handle != 0 && last_handle != Some(handle) {
                    last_handle = Some(handle);
                    let known = shared.by_handle.lock().get(&handle).cloned();
                    let id: Option<Arc<str>> = match known {
                        Some(id) => Some(id),
                        None => wc::image(handle).map(|img| {
                            let hash = fnv1a64(&img.rgba);
                            let custom = format!("custom-{hash:016x}");
                            let id = match shared.register(&custom, false, &img, hash) {
                                Ok(id) => {
                                    if *id == *custom {
                                        shared.log.notice(COMPONENT, format!("Saved new cursor image. id={id}"));
                                    }
                                    id
                                }
                                Err(e) => {
                                    shared.log.warning(COMPONENT, format!("Could not save cursor {custom}: {e}"));
                                    Arc::from(custom.as_str())
                                }
                            };
                            // app cursors are often re-created with new handles: remember by handle too
                            let mut by_handle = shared.by_handle.lock();
                            if by_handle.len() > 256 {
                                by_handle.clear();
                            }
                            by_handle.insert(handle, id.clone());
                            id
                        }),
                    };
                    if let Some(id) = id {
                        let changed = {
                            let mut cur = shared.current.lock();
                            if *cur != id {
                                *cur = id.clone();
                                true
                            } else {
                                false
                            }
                        };
                        if changed {
                            on_change(&id);
                        }
                    }
                }
            }
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    }
}

/// Linux: X11 does not expose the theme's cursor images portably, so the recording gets one
/// drawn arrow (`arrow`, like the macOS id) and the editor renders it smoothly over the video.
#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::sync::Arc;

    use sf_project::recording::CursorInfo;
    use sf_project::schema::common::{Point, Size};

    use crate::log::RecLog;
    use crate::{Error, Result};

    const COMPONENT: &str = "CursorRecorder";
    /// Arrow outline in points (tip at the hot spot), drawn at 2× into a 24×32 pt image.
    const ARROW: [(f64, f64); 7] = [(4.0, 4.0), (4.0, 21.0), (8.2, 17.0), (11.2, 24.0), (13.6, 23.0), (10.6, 16.0), (16.0, 16.0)];

    fn inside(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
        let mut c = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    }

    /// Black arrow with a white border, 4× supersampled.
    fn arrow_png() -> Option<Vec<u8>> {
        let (w, h, scale) = (48u32, 64u32, 2.0);
        let mut img = image::RgbaImage::new(w, h);
        for py in 0..h {
            for px in 0..w {
                let (mut fill, mut edge) = (0u32, 0u32);
                for sy in 0..4 {
                    for sx in 0..4 {
                        let x = (px as f64 + (sx as f64 + 0.5) / 4.0) / scale;
                        let y = (py as f64 + (sy as f64 + 0.5) / 4.0) / scale;
                        if inside(&ARROW, x, y) {
                            // a sample is border when a neighbour ~1 pt away falls outside
                            let near_out = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.7, -0.7), (0.7, 0.7), (0.7, -0.7), (-0.7, 0.7)]
                                .iter()
                                .any(|(dx, dy)| !inside(&ARROW, x + dx * 1.1, y + dy * 1.1));
                            if near_out { edge += 1 } else { fill += 1 }
                        }
                    }
                }
                let a = ((fill + edge) * 255 / 16) as u8;
                if a > 0 {
                    let v = (edge * 255 / (fill + edge).max(1)) as u8;
                    img.put_pixel(px, py, image::Rgba([v, v, v, a]));
                }
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }

    pub struct CursorRecorder {
        id: Arc<str>,
        cursors: Vec<CursorInfo>,
    }

    impl CursorRecorder {
        pub fn start(dir: &Path, log: RecLog, on_change: Box<dyn Fn(&str) + Send>) -> Result<Self> {
            let folder = dir.join("cursors");
            std::fs::create_dir_all(&folder).map_err(|source| Error::Io { path: folder.clone(), source })?;
            let png = arrow_png().ok_or_else(|| Error::Capture("could not draw the cursor".into()))?;
            let path = folder.join("arrow.png");
            std::fs::write(&path, png).map_err(|source| Error::Io { path, source })?;
            let info = CursorInfo { id: "arrow".into(), hot_spot: Point::new(4.0, 4.0), standard_size: Size { width: 24.0, height: 32.0 }, system_cursor: true, extra: Default::default() };
            let cursors = vec![info];
            super::write_cursors_json(dir, &cursors).map_err(|source| Error::Io { path: dir.join("cursors.json"), source })?;
            on_change("arrow");
            log.notice(COMPONENT, "Cursor: drawn arrow (X11 cursor images are not captured).");
            Ok(CursorRecorder { id: "arrow".into(), cursors })
        }

        pub fn current_id(&self) -> Arc<str> {
            self.id.clone()
        }

        pub fn cursors(&self) -> Vec<CursorInfo> {
            self.cursors.clone()
        }

        pub fn stop(&mut self) {}
    }
}

/// `(width, height, png)` of the arrow cursor; main thread only. Used by the smoke example.
#[cfg(target_os = "macos")]
pub fn arrow_cursor_png() -> Option<(usize, usize, Vec<u8>)> {
    let image = objc2_app_kit::NSCursor::arrowCursor().image();
    let (w, h, rgba) = imp::rasterize(&image)?;
    Some((w, h, imp::encode_png(w, h, &rgba)?))
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, Sel};
    use objc2::runtime::MessageReceiver;
    use objc2::{ClassType, Message};
    use objc2_app_kit::{NSBitmapImageRep, NSCursor, NSImage};
    use parking_lot::Mutex;
    use sf_project::recording::CursorInfo;
    use sf_project::schema::common::{Point, Size};

    use super::{fnv1a64, write_cursors_json};
    use crate::log::RecLog;
    use crate::{Error, Result};

    const COMPONENT: &str = "CursorRecorder";

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        /// Initializes the AppKit application environment for a process without an
        /// `NSApplication` main loop; without it the `NSCursor` class getters return nil.
        fn NSApplicationLoad() -> bool;
    }

    /// `(Screen Studio id, NSCursor class selector)` in registration priority order: public
    /// cursors first (so a private alias of a public image keeps the public name), then the
    /// private ones polyrecorder also knows about.
    const KNOWN: &[(&str, &str)] = &[
        ("arrow", "arrowCursor"),
        ("iBeam", "IBeamCursor"),
        ("pointingHand", "pointingHandCursor"),
        ("closedHand", "closedHandCursor"),
        ("openHand", "openHandCursor"),
        ("crosshair", "crosshairCursor"),
        ("resizeLeft", "resizeLeftCursor"),
        ("resizeRight", "resizeRightCursor"),
        ("resizeLeftRight", "resizeLeftRightCursor"),
        ("resizeUp", "resizeUpCursor"),
        ("resizeDown", "resizeDownCursor"),
        ("resizeUpDown", "resizeUpDownCursor"),
        ("contextualMenu", "contextualMenuCursor"),
        ("disappearingItem", "disappearingItemCursor"),
        ("dragCopy", "dragCopyCursor"),
        ("dragLink", "dragLinkCursor"),
        ("operationNotAllowed", "operationNotAllowedCursor"),
        ("iBeamCursorForVerticalLayout", "IBeamCursorForVerticalLayout"),
        ("zoomIn", "zoomInCursor"),
        ("zoomOut", "zoomOutCursor"),
        ("columnResize", "columnResizeCursor"),
        ("rowResize", "rowResizeCursor"),
        // private
        ("help", "_helpCursor"),
        ("busyButClickable", "busyButClickableCursor"),
        ("busyButClickable", "_busyButClickableCursor"),
        ("topLeftResize", "_topLeftResizeCursor"),
        ("topRightResize", "_topRightResizeCursor"),
        ("bottomLeftResize", "_bottomLeftResizeCursor"),
        ("bottomRightResize", "_bottomRightResizeCursor"),
        ("windowResizeEast", "_windowResizeEastCursor"),
        ("windowResizeWest", "_windowResizeWestCursor"),
        ("windowResizeNorth", "_windowResizeNorthCursor"),
        ("windowResizeSouth", "_windowResizeSouthCursor"),
        ("windowResizeNorthEast", "_windowResizeNorthEastCursor"),
        ("windowResizeNorthWest", "_windowResizeNorthWestCursor"),
        ("windowResizeSouthEast", "_windowResizeSouthEastCursor"),
        ("windowResizeSouthWest", "_windowResizeSouthWestCursor"),
        ("windowResizeEastWest", "_windowResizeEastWestCursor"),
        ("windowResizeNorthSouth", "_windowResizeNorthSouthCursor"),
        ("windowResizeNorthEastSouthWest", "_windowResizeNorthEastSouthWestCursor"),
        ("windowResizeNorthWestSouthEast", "_windowResizeNorthWestSouthEastCursor"),
        ("closedHand", "_closedHandCursor"),
        ("crosshair", "_crosshairCursor"),
        ("genericDrag", "_genericDragCursor"),
        ("hand", "_handCursor"),
        ("horizontalResize", "_horizontalResizeCursor"),
        ("verticalResize", "_verticalResizeCursor"),
        ("overrideHelp", "_overrideHelpCursor"),
        ("zoomIn", "_zoomInCursor"),
        ("zoomOut", "_zoomOutCursor"),
    ];

    /// Call `+[NSCursor <selector>]` if the class responds to it.
    fn class_cursor(cls: &AnyClass, selector: &str) -> Option<Retained<NSCursor>> {
        let name = std::ffi::CString::new(selector).ok()?;
        let sel = Sel::register(&name);
        if !cls.metaclass().responds_to(sel) {
            return None;
        }
        // SAFETY: all `*…Cursor` class methods take no arguments and return an
        // autoreleased NSCursor (or nil).
        let ptr: *mut NSCursor = unsafe { cls.send_message(sel, ()) };
        unsafe { Retained::retain(ptr) }
    }

    struct Snapshot {
        hash: u64,
        /// Straight-alpha RGBA pixels of the largest representation.
        rgba: Vec<u8>,
        pixels: (usize, usize),
        hot_spot: Point,
        size: Size,
    }

    fn snapshot(cursor: &NSCursor) -> Option<Snapshot> {
        let image: Retained<NSImage> = cursor.image();
        let (w, h, rgba) = rasterize(&image)?;
        let hash = fnv1a64(&rgba);
        let hs = cursor.hotSpot();
        let sz = image.size();
        Some(Snapshot {
            hash,
            rgba,
            pixels: (w, h),
            hot_spot: Point::new(hs.x, hs.y),
            size: Size { width: sz.width, height: sz.height },
        })
    }

    /// Draw the largest representation of `image` into a CoreGraphics bitmap and return straight
    /// RGBA pixels. Deliberately avoids NSImage's TIFF/PNG encoders: ImageIO crashes (SIGBUS) in
    /// this process, see git history.
    pub(crate) fn rasterize(image: &NSImage) -> Option<(usize, usize, Vec<u8>)> {
        use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
        use core_graphics::color_space::CGColorSpace;
        use core_graphics::context::CGContext;
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        use core_graphics::image::CGImage;
        use foreign_types::ForeignType;

        // largest bitmap rep → its CGImage; otherwise whatever NSImage picks
        let mut best: Option<(i64, Retained<NSBitmapImageRep>)> = None;
        for rep in image.representations().iter() {
            let Some(bmp) = rep.downcast_ref::<NSBitmapImageRep>() else { continue };
            let area = (bmp.pixelsWide() * bmp.pixelsHigh()) as i64;
            if best.as_ref().is_none_or(|(a, _)| area > *a) {
                best = Some((area, bmp.retain()));
            }
        }
        // `CGImageRef` as an opaque struct so objc2's debug type check matches `^{CGImage=}`
        #[repr(C)]
        struct CGImageOpaque {
            _p: [u8; 0],
        }
        unsafe impl objc2::encode::RefEncode for CGImageOpaque {
            const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGImage", &[]));
        }
        let raw: *mut CGImageOpaque = match &best {
            Some((_, bmp)) => unsafe { objc2::msg_send![&**bmp, CGImage] },
            None => unsafe {
                objc2::msg_send![&*image, CGImageForProposedRect: std::ptr::null_mut::<objc2_foundation::NSRect>(), context: std::ptr::null_mut::<objc2::runtime::AnyObject>(), hints: std::ptr::null_mut::<objc2::runtime::AnyObject>()]
            },
        };
        if raw.is_null() {
            return None;
        }
        // the returned image is autoreleased/owned by the rep: borrow it without releasing
        let borrowed = unsafe { CGImage::from_ptr(raw as *mut _) };
        let cg = borrowed.clone();
        std::mem::forget(borrowed);
        let (w, h) = (cg.width(), cg.height());
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            return None;
        }
        let space = CGColorSpace::create_device_rgb();
        let mut ctx = CGContext::create_bitmap_context(None, w, h, 8, w * 4, &space, kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        ctx.draw_image(CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64)), &cg);
        let mut rgba = ctx.data().to_vec();
        rgba.truncate(w * h * 4);
        for px in rgba.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Some((w, h, rgba))
    }

    pub(crate) fn encode_png(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out).write_image(rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8).ok()?;
        Some(out)
    }

    struct Shared {
        dir: PathBuf,
        log: RecLog,
        /// TIFF hash → id
        by_hash: Mutex<HashMap<u64, Arc<str>>>,
        infos: Mutex<Vec<CursorInfo>>,
        current: Mutex<Arc<str>>,
        stop: AtomicBool,
    }

    impl Shared {
        fn register(&self, id: &str, system: bool, snap: &Snapshot) -> Result<bool> {
            {
                let mut by_hash = self.by_hash.lock();
                if by_hash.contains_key(&snap.hash) || self.infos.lock().iter().any(|c| c.id == id) {
                    return Ok(false);
                }
                by_hash.insert(snap.hash, Arc::from(id));
            }
            let png = encode_png(snap.pixels.0, snap.pixels.1, &snap.rgba).ok_or_else(|| Error::Capture(format!("cursor {id}: png encode failed")))?;
            let path = self.dir.join("cursors").join(format!("{id}.png"));
            std::fs::write(&path, png).map_err(|source| Error::Io { path, source })?;
            let mut infos = self.infos.lock();
            infos.push(CursorInfo {
                id: id.to_string(),
                hot_spot: snap.hot_spot,
                standard_size: snap.size,
                system_cursor: system,
                extra: Default::default(),
            });
            write_cursors_json(&self.dir, &infos).map_err(|source| Error::Io { path: self.dir.join("cursors.json"), source })?;
            Ok(true)
        }
    }

    /// Polls the system cursor and maintains `cursors.json` / `cursors/`.
    pub struct CursorRecorder {
        shared: Arc<Shared>,
        thread: Option<JoinHandle<()>>,
    }

    impl CursorRecorder {
        /// Preload the catalog into `dir/cursors`, then start polling. `on_change(id)` is
        /// invoked from the poll thread whenever the cursor image changes.
        pub fn start(dir: &Path, log: RecLog, on_change: Box<dyn Fn(&str) + Send>) -> Result<Self> {
            let cursors_dir = dir.join("cursors");
            std::fs::create_dir_all(&cursors_dir).map_err(|source| Error::Io { path: cursors_dir.clone(), source })?;
            let shared = Arc::new(Shared {
                dir: dir.to_path_buf(),
                log: log.clone(),
                by_hash: Mutex::new(HashMap::new()),
                infos: Mutex::new(Vec::new()),
                current: Mutex::new(Arc::from("arrow")),
                stop: AtomicBool::new(false),
            });

            if !unsafe { NSApplicationLoad() } {
                log.warning(COMPONENT, "NSApplicationLoad failed; system cursors may not preload.");
            }
            let cls = NSCursor::class();
            let mut loaded = 0usize;
            let mut dropped = Vec::new();
            for (id, selector) in KNOWN {
                let Some(cursor) = class_cursor(cls, selector) else { continue };
                let Some(snap) = snapshot(&cursor) else { continue };
                loaded += 1;
                match shared.register(id, true, &snap) {
                    Ok(true) => {}
                    Ok(false) => {
                        let existing = shared.by_hash.lock().get(&snap.hash).cloned().unwrap_or_default();
                        dropped.push(format!("{id} > existing: {existing}"));
                    }
                    Err(e) => log.warning(COMPONENT, format!("Could not save cursor {id}: {e}")),
                }
            }
            log.notice(
                COMPONENT,
                format!("Preloaded system cursors. loaded={loaded} saved={} dropped=[{}]", shared.infos.lock().len(), dropped.join(", ")),
            );

            let thread_shared = shared.clone();
            let thread = std::thread::Builder::new()
                .name("sf-cursor-poll".into())
                .spawn(move || poll_loop(thread_shared, on_change))
                .map_err(|source| Error::Io { path: "thread".into(), source })?;
            Ok(CursorRecorder { shared, thread: Some(thread) })
        }

        /// Id of the cursor currently shown (`arrow`, `iBeam`, `custom-…`).
        pub fn current_id(&self) -> Arc<str> {
            self.shared.current.lock().clone()
        }

        /// Snapshot of `cursors.json`.
        pub fn cursors(&self) -> Vec<CursorInfo> {
            self.shared.infos.lock().clone()
        }

        /// Stop polling (the files stay on disk).
        pub fn stop(&mut self) {
            self.shared.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for CursorRecorder {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn poll_loop(shared: Arc<Shared>, on_change: Box<dyn Fn(&str) + Send>) {
        let interval = Duration::from_micros(16_667);
        let mut last_hash: Option<u64> = None;
        let mut next = Instant::now();
        while !shared.stop.load(Ordering::Relaxed) {
            next += interval;
            #[allow(deprecated)]
            let current = NSCursor::currentSystemCursor();
            if let Some(cursor) = current {
                if let Some(snap) = snapshot(&cursor) {
                    if last_hash != Some(snap.hash) {
                        last_hash = Some(snap.hash);
                        let known = shared.by_hash.lock().get(&snap.hash).cloned();
                        let id: Arc<str> = match known {
                            Some(id) => id,
                            None => {
                                let id = format!("custom-{:016x}", snap.hash);
                                match shared.register(&id, false, &snap) {
                                    Ok(_) => shared.log.notice(COMPONENT, format!("Saved new cursor image. id={id}")),
                                    Err(e) => shared.log.warning(COMPONENT, format!("Could not save cursor {id}: {e}")),
                                }
                                Arc::from(id.as_str())
                            }
                        };
                        let changed = {
                            let mut cur = shared.current.lock();
                            if *cur != id {
                                *cur = id.clone();
                                true
                            } else {
                                false
                            }
                        };
                        if changed {
                            on_change(&id);
                        }
                    }
                }
            }
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf_project::schema::common::{Point, Size};

    #[test]
    fn cursors_json_matches_polyrecorder_layout() {
        let dir = tempfile::tempdir().unwrap();
        let infos = vec![CursorInfo {
            id: "arrow".into(),
            hot_spot: Point::new(5.0, 5.0),
            standard_size: Size { width: 28.0, height: 40.0 },
            system_cursor: true,
            extra: Default::default(),
        }];
        write_cursors_json(dir.path(), &infos).unwrap();
        let text = std::fs::read_to_string(dir.path().join("cursors.json")).unwrap();
        assert_eq!(
            text,
            "[\n{\"hotSpot\":{\"x\":5,\"y\":5},\"id\":\"arrow\",\"standardSize\":{\"height\":40,\"width\":28},\"systemCursor\":true}\n]"
        );
        let back = sf_project::recording::load_cursors(dir.path()).unwrap();
        assert_eq!(back, infos);
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
    }
}

