//! Text caret position through the Accessibility API, so automatic zooms can follow typing.
//! Asks the focused UI element for its selected text range and the screen bounds of that range.
//! Needs Accessibility access; without it (or in apps that do not expose text, such as many
//! terminals and canvas-based editors) every query returns `None` and zooms follow the mouse.

/// Caret centre in global screen points (top-left origin), if the focused element exposes it.
#[cfg(target_os = "macos")]
pub fn caret_position() -> Option<(f64, f64)> {
    imp::caret_position()
}

#[cfg(not(target_os = "macos"))]
pub fn caret_position() -> Option<(f64, f64)> {
    None
}

/// Whether this process may use the Accessibility API (no prompt).
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    unsafe { imp::AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_trusted() -> bool {
    false
}

/// Show the system "allow Accessibility" prompt (adds the app to the list). Returns the current state.
#[cfg(target_os = "macos")]
pub fn request_accessibility() -> bool {
    imp::request()
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod imp {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::CGRect;
    use std::ffi::c_void;

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const AX_VALUE_CGRECT: u32 = 3;
    const AX_VALUE_CFRANGE: u32 = 4;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CFRange {
        location: isize,
        length: isize,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementCopyParameterizedAttributeValue(element: AXUIElementRef, attribute: CFStringRef, parameter: CFTypeRef, value: *mut CFTypeRef) -> i32;
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

    fn bounds_for(element: AXUIElementRef, range: CFRange) -> Option<CGRect> {
        let param = Owned(unsafe { AXValueCreate(AX_VALUE_CFRANGE, &range as *const CFRange as *const c_void) });
        if param.0.is_null() {
            return None;
        }
        let key = CFString::new("AXBoundsForRange");
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyParameterizedAttributeValue(element, key.as_concrete_TypeRef(), param.0, &mut out) };
        if err != AX_SUCCESS || out.is_null() {
            return None;
        }
        let value = Owned(out);
        let mut rect = CGRect::default();
        let ok = unsafe { AXValueGetValue(value.0, AX_VALUE_CGRECT, &mut rect as *mut CGRect as *mut c_void) };
        // some apps answer with an empty rect at the origin when they have nothing useful
        (ok && (rect.origin.x != 0.0 || rect.origin.y != 0.0) && rect.size.height > 0.0).then_some(rect)
    }

    /// PID of the frontmost application (`NSWorkspace.sharedWorkspace.frontmostApplication`).
    fn frontmost_pid() -> Option<i32> {
        use objc2::runtime::{AnyClass, AnyObject};
        use objc2::msg_send;
        let cls = AnyClass::get(c"NSWorkspace")?;
        unsafe {
            let ws: *mut AnyObject = msg_send![cls, sharedWorkspace];
            if ws.is_null() {
                return None;
            }
            let app: *mut AnyObject = msg_send![ws, frontmostApplication];
            if app.is_null() {
                return None;
            }
            let pid: i32 = msg_send![app, processIdentifier];
            (pid > 0).then_some(pid)
        }
    }

    pub fn request() -> bool {
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        let opts = CFDictionary::from_CFType_pairs(&[(CFString::new("AXTrustedCheckOptionPrompt"), CFBoolean::true_value())]);
        unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) }
    }

    pub fn caret_position() -> Option<(f64, f64)> {
        // the focused app's element works where the system-wide one answers
        // kAXErrorCannotComplete (seen on macOS 26), so ask the app first
        let from_app = || {
            let app = Owned(unsafe { AXUIElementCreateApplication(frontmost_pid()?) });
            if app.0.is_null() {
                return None;
            }
            // never stall the input writer on a busy app
            unsafe { AXUIElementSetMessagingTimeout(app.0, 0.05) };
            attr(app.0, "AXFocusedUIElement")
        };
        let from_system = || {
            let system = Owned(unsafe { AXUIElementCreateSystemWide() });
            if system.0.is_null() {
                return None;
            }
            unsafe { AXUIElementSetMessagingTimeout(system.0, 0.05) };
            attr(system.0, "AXFocusedUIElement")
        };
        let focused = from_app().or_else(from_system)?;
        unsafe { AXUIElementSetMessagingTimeout(focused.0, 0.05) };
        let sel = attr(focused.0, "AXSelectedTextRange")?;
        let mut range = CFRange::default();
        if !unsafe { AXValueGetValue(sel.0, AX_VALUE_CFRANGE, &mut range as *mut CFRange as *mut c_void) } {
            return None;
        }
        // an empty range often has no bounds: use the character before the caret (its right
        // edge) or, at the start of the text, the character after it (its left edge)
        let rect = bounds_for(focused.0, range)
            .or_else(|| {
                (range.location > 0).then(|| bounds_for(focused.0, CFRange { location: range.location - 1, length: 1 })).flatten().map(|mut r| {
                    r.origin.x += r.size.width;
                    r.size.width = 0.0;
                    r
                })
            })
            .or_else(|| {
                bounds_for(focused.0, CFRange { location: range.location, length: 1 }).map(|mut r| {
                    r.size.width = 0.0;
                    r
                })
            })?;
        let x = rect.origin.x + if range.length > 0 { rect.size.width } else { rect.size.width / 2.0 };
        Some((x, rect.origin.y + rect.size.height / 2.0))
    }
}
