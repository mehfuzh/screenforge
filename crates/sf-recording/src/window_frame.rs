//! Resize another app's window through the Accessibility API, so a window can be brought to a
//! chosen aspect ratio before it is recorded. Needs Accessibility access; without it (and on
//! other platforms) [`set_window_frame`] returns `false` and the window is left as it is.

use sf_project::schema::common::Rect;

/// `current` resized to `ratio` (width / height): keeps the height and centre where it can and
/// shrinks to fit inside `screen`. Whole points.
pub fn fit_aspect(current: Rect, ratio: f64, screen: Rect) -> Rect {
    let (mut w, mut h) = (current.height * ratio, current.height);
    let scale = (screen.width / w).min(screen.height / h).min(1.0);
    w = (w * scale).round();
    h = (h * scale).round();
    let cx = current.x + current.width / 2.0;
    let cy = current.y + current.height / 2.0;
    let x = (cx - w / 2.0).clamp(screen.x, screen.x + screen.width - w).round();
    let y = (cy - h / 2.0).clamp(screen.y, screen.y + screen.height - h).round();
    Rect { x, y, width: w, height: h }
}

/// Move and resize the window of `pid` whose frame is (closest to) `current` to `target`, both in
/// global screen points with a top-left origin. Returns whether the window accepted the frame.
#[cfg(target_os = "macos")]
pub fn set_window_frame(pid: i32, current: Rect, target: Rect) -> bool {
    imp::set_window_frame(pid, current, target)
}

#[cfg(not(target_os = "macos"))]
pub fn set_window_frame(pid: i32, current: Rect, target: Rect) -> bool {
    let _ = (pid, current, target);
    false
}

#[cfg(target_os = "macos")]
mod imp {
    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::{CGPoint, CGSize};
    use sf_project::schema::common::Rect;
    use std::ffi::c_void;

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CGPOINT: u32 = 1;
    const AX_VALUE_CGSIZE: u32 = 2;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: CFTypeRef) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> i32;
        fn AXValueCreate(kind: u32, value: *const c_void) -> AXValueRef;
        fn AXValueGetValue(value: AXValueRef, kind: u32, out: *mut c_void) -> bool;
    }

    /// Owned CF object released on drop.
    struct Owned(CFTypeRef);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn attr(element: AXUIElementRef, name: &str) -> Option<Owned> {
        let key = CFString::new(name);
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut out) };
        (err == AX_SUCCESS && !out.is_null()).then(|| Owned(out))
    }

    fn value<T: Default>(element: AXUIElementRef, name: &str, kind: u32) -> Option<T> {
        let v = attr(element, name)?;
        let mut out = T::default();
        unsafe { AXValueGetValue(v.0, kind, &mut out as *mut T as *mut c_void) }.then_some(out)
    }

    fn set<T>(element: AXUIElementRef, name: &str, kind: u32, v: &T) -> bool {
        let boxed = Owned(unsafe { AXValueCreate(kind, v as *const T as *const c_void) });
        if boxed.0.is_null() {
            return false;
        }
        let key = CFString::new(name);
        unsafe { AXUIElementSetAttributeValue(element, key.as_concrete_TypeRef(), boxed.0) == AX_SUCCESS }
    }

    pub fn set_window_frame(pid: i32, current: Rect, target: Rect) -> bool {
        let app = Owned(unsafe { AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            return false;
        }
        unsafe { AXUIElementSetMessagingTimeout(app.0, 0.5) };
        let Some(windows) = attr(app.0, "AXWindows") else { return false };
        let list = windows.0 as CFArrayRef;
        // there is no public link from a CGWindowID to its AX element: take the window whose
        // frame matches the one we saw
        let mut best: Option<(f64, AXUIElementRef)> = None;
        for i in 0..unsafe { CFArrayGetCount(list) } {
            let w = unsafe { CFArrayGetValueAtIndex(list, i) } as AXUIElementRef;
            let (Some(pos), Some(size)) = (value::<CGPoint>(w, "AXPosition", AX_VALUE_CGPOINT), value::<CGSize>(w, "AXSize", AX_VALUE_CGSIZE)) else { continue };
            let d = (pos.x - current.x).abs() + (pos.y - current.y).abs() + (size.width - current.width).abs() + (size.height - current.height).abs();
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, w));
            }
        }
        let Some((_, w)) = best else { return false };
        let size = CGSize::new(target.width, target.height);
        let pos = CGPoint::new(target.x, target.y);
        // size, move, then size again: a window may refuse a size that would not fit where it is
        let a = set(w, "AXSize", AX_VALUE_CGSIZE, &size);
        let b = set(w, "AXPosition", AX_VALUE_CGPOINT, &pos);
        let c = set(w, "AXSize", AX_VALUE_CGSIZE, &size);
        (a || c) && b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 25.0, width: 1440.0, height: 875.0 };

    #[test]
    fn keeps_height_and_centre() {
        let r = fit_aspect(Rect { x: 200.0, y: 100.0, width: 800.0, height: 450.0 }, 1.0, SCREEN);
        assert_eq!(r, Rect { x: 375.0, y: 100.0, width: 450.0, height: 450.0 });
    }

    #[test]
    fn shrinks_to_fit_and_stays_on_screen() {
        let r = fit_aspect(Rect { x: 0.0, y: 25.0, width: 600.0, height: 800.0 }, 16.0 / 9.0, SCREEN);
        assert_eq!(r.width, 1422.0);
        assert_eq!(r.height, 800.0);
        assert!(r.x >= SCREEN.x && r.x + r.width <= SCREEN.x + SCREEN.width);
        assert!(r.y >= SCREEN.y && r.y + r.height <= SCREEN.y + SCREEN.height);
    }
}
