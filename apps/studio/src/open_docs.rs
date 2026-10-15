//! Projects opened from Finder (double-click, Open With, dropped on the Dock icon).
//!
//! macOS delivers these as an `odoc` Apple Event, not as command-line arguments. Without a
//! handler AppKit falls back to `NSDocumentController` and shows "ScreenForge cannot open files
//! in the … format", so we take the event over and queue the paths for the app to open.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Wake the UI when a document arrives while the app is idle.
pub fn set_context(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// The most recent path Finder asked us to open, if any (earlier ones in the same batch are dropped).
pub fn take() -> Option<PathBuf> {
    PENDING.lock().ok().and_then(|mut v| v.drain(..).last())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn push(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if let Ok(mut v) = PENDING.lock() {
        v.extend(paths);
    }
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

/// Register for the open-documents event. Call on the main thread before the event loop runs.
#[cfg(target_os = "macos")]
pub fn install() {
    mac::install();
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject};

    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
    const FILE_URL: u32 = u32::from_be_bytes(*b"furl");

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "SFOpenDocuments"]
        struct OpenDocuments;

        impl OpenDocuments {
            /// AppKit installs its own `odoc` handler while launching; replacing it here (the
            /// documented place) also catches the document a launch from Finder carries.
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                let manager = NSAppleEventManager::sharedAppleEventManager();
                // (called directly: the typed binding needs the objc2-core-services crate)
                let _: () = unsafe { msg_send![&manager, setEventHandler: self, andSelector: sel!(handleOpen:withReply:), forEventClass: CORE_EVENT_CLASS, andEventID: OPEN_DOCUMENTS] };
            }

            #[unsafe(method(handleOpen:withReply:))]
            fn handle_open(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
                let Some(list) = list else { return };
                // a single document can arrive as a bare descriptor rather than a list
                let items: Vec<_> = if list.numberOfItems() > 0 { (1..=list.numberOfItems()).filter_map(|i| list.descriptorAtIndex(i)).collect() } else { vec![list] };
                let paths = items
                    .iter()
                    .filter_map(|d| -> Option<Retained<NSAppleEventDescriptor>> { unsafe { msg_send![&**d, coerceToDescriptorType: FILE_URL] } })
                    .filter_map(|d| d.fileURLValue())
                    .filter_map(|url| url.to_file_path())
                    .collect();
                super::push(paths);
            }
        }
    );

    pub fn install() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let handler: Retained<OpenDocuments> = unsafe { msg_send![OpenDocuments::alloc(mtm), init] };
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(&handler, sel!(willFinishLaunching:), Some(NSApplicationWillFinishLaunchingNotification), None);
        }
        // the notification centre and the event manager do not retain their targets
        std::mem::forget(handler);
    }
}
