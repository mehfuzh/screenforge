//! Frosted-glass background for the floating toolbar and its recent-projects panel
//! (macOS `NSVisualEffectView`).
//!
//! winit's content view owns the Metal layer, so a subview cannot be drawn *below* it. The
//! effect view is therefore inserted into the window frame view as a sibling positioned under
//! the content view. It is sized to the pill and gets rounded corners; the UI draws the pill
//! with a light tint on top of it.

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode, NSWindowStyleMask, NSWindowTitleVisibility};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    thread_local! {
        static EFFECT: RefCell<Option<Retained<NSVisualEffectView>>> = const { RefCell::new(None) };
        static PANEL: RefCell<Option<Retained<NSVisualEffectView>>> = const { RefCell::new(None) };
    }

    /// Editor window chrome like Screen Studio: no native title bar strip, the traffic lights
    /// float over our own toolbar. Call after winit has applied `Decorations(true)`.
    pub fn transparent_titlebar() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else { return };
        if !window.styleMask().contains(NSWindowStyleMask::Titled) {
            return;
        }
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setStyleMask(window.styleMask() | NSWindowStyleMask::FullSizeContentView);
    }

    /// Show (or move) the frosted pill. `rect` is in egui points, top-left origin. `None` also
    /// removes the panel glass.
    pub fn set(rect: Option<[f32; 4]>, window_height_pt: f32, radius: f32) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else { return };
        let Some(content) = window.contentView() else { return };
        pin_contents_to_bottom(&content, rect.is_some());
        EFFECT.with(|slot| place(&mut slot.borrow_mut(), rect, window_height_pt, radius, &content, mtm));
        if rect.is_none() {
            PANEL.with(|slot| place(&mut slot.borrow_mut(), None, 0.0, 0.0, &content, mtm));
        }
    }

    /// The same glass behind the panel that opens above the pill (recent projects).
    pub fn set_panel(rect: Option<[f32; 4]>, window_height_pt: f32, radius: f32) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else { return };
        let Some(content) = window.contentView() else { return };
        PANEL.with(|slot| {
            let mut slot = slot.borrow_mut();
            match (rect, slot.as_ref()) {
                // keep the view while the bar is up: re-creating it on every toggle flashes
                (None, Some(v)) => v.setHidden(true),
                (None, None) => {}
                (Some(_), _) => place(&mut slot, rect, window_height_pt, radius, &content, mtm),
            }
        });
    }

    fn place(slot: &mut Option<Retained<NSVisualEffectView>>, rect: Option<[f32; 4]>, window_height_pt: f32, radius: f32, content: &objc2_app_kit::NSView, mtm: MainThreadMarker) {
        match rect {
            None => {
                if let Some(v) = slot.take() {
                    v.removeFromSuperview();
                }
            }
            Some([x, y, w, h]) => {
                let frame = NSRect::new(NSPoint::new(x as f64, (window_height_pt - y - h) as f64), NSSize::new(w as f64, h as f64));
                // frame in the superview's (window frame view) coordinates
                let origin = content.frame().origin;
                let frame = NSRect::new(NSPoint::new(frame.origin.x + origin.x, frame.origin.y + origin.y), frame.size);
                if slot.is_none() {
                    let Some(superview) = (unsafe { content.superview() }) else { return };
                    let effect = NSVisualEffectView::initWithFrame(mtm.alloc::<NSVisualEffectView>(), frame);
                    effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
                    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                    effect.setState(NSVisualEffectState::Active);
                    effect.setWantsLayer(true);
                    if let Some(layer) = effect.layer() {
                        layer.setCornerRadius(radius as f64);
                        layer.setMasksToBounds(true);
                    }
                    let _ = NSAutoresizingMaskOptions::ViewWidthSizable;
                    superview.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, Some(content));
                    *slot = Some(effect);
                } else if let Some(v) = slot.as_ref() {
                    v.setFrame(frame);
                    v.setHidden(false);
                }
            }
        }
    }

    /// While the floating bar is shown the window grows upwards for menus and panels, with the
    /// bar pinned to its bottom edge. Until the first frame at the new size is presented, Core
    /// Animation shows the previous one; with the Metal layer's default `resize` gravity that
    /// frame is stretched over the new height (a tall, distorted bar). Pinning the contents to
    /// the bottom shows it unscaled where the bar really is. `false` restores `resize` (editor).
    fn pin_contents_to_bottom(content: &objc2_app_kit::NSView, pin: bool) {
        use objc2_foundation::NSString;
        let Some(root) = content.layer() else { return };
        let mut layers = vec![root.clone()];
        // SAFETY: read on the main thread; the array is copied before the layers are touched
        if let Some(subs) = unsafe { root.sublayers() } {
            layers.extend(subs.iter());
        }
        for layer in layers {
            // "bottom" is in the layer's own coordinates: visually the top in a flipped hierarchy
            let gravity = match (pin, layer.contentsAreFlipped()) {
                (false, _) => "resize",
                (true, false) => "bottom",
                (true, true) => "top",
            };
            layer.setContentsGravity(&NSString::from_str(gravity));
        }
    }

    pub fn is_active() -> bool {
        EFFECT.with(|s| s.borrow().is_some())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn set(_rect: Option<[f32; 4]>, _window_height_pt: f32, _radius: f32) {}
    pub fn set_panel(_rect: Option<[f32; 4]>, _window_height_pt: f32, _radius: f32) {}
    pub fn transparent_titlebar() {}
    pub fn is_active() -> bool {
        false
    }
}

pub use imp::{is_active, set, set_panel, transparent_titlebar};
