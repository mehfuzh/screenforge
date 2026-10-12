//! Menu bar (status bar) icon shown while ScreenForge runs, like Screen Studio's.
//! Shows the elapsed time while recording; the menu opens the picker, a project, or quits.
//!
//! The icon is built from raw RGBA through `NSBitmapImageRep` (no PNG decoding through ImageIO,
//! which crashed with SIGBUS on some machines). The menu comes from `muda`.

use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};

pub enum TrayAction {
    ShowPicker,
    OpenProject,
    Quit,
}

pub struct Tray {
    _menu: Menu,
    ids: [MenuId; 3],
    last_title: Option<String>,
    #[cfg(target_os = "macos")]
    item: objc2::rc::Retained<objc2_app_kit::NSStatusItem>,
}

impl Tray {
    pub fn new() -> Option<Self> {
        let menu = Menu::new();
        let record = MenuItem::new("New Recording…", true, None);
        let open = MenuItem::new("Open Project…", true, None);
        let quit = MenuItem::new("Quit ScreenForge", true, None);
        let ids = [record.id().clone(), open.id().clone(), quit.id().clone()];
        menu.append_items(&[&record, &open, &PredefinedMenuItem::separator(), &quit]).ok()?;
        #[cfg(target_os = "macos")]
        let item = macos::create_status_item(&menu)?;
        Some(Tray {
            _menu: menu,
            ids,
            last_title: None,
            #[cfg(target_os = "macos")]
            item,
        })
    }

    /// Text next to the icon (recording timer); `None` clears it.
    pub fn set_title(&mut self, title: Option<String>) {
        if self.last_title != title {
            #[cfg(target_os = "macos")]
            macos::set_title(&self.item, title.as_deref());
            self.last_title = title;
        }
    }

    /// Drain pending menu events.
    pub fn poll(&mut self) -> Vec<TrayAction> {
        let mut out = vec![];
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == self.ids[0] {
                out.push(TrayAction::ShowPicker);
            } else if e.id == self.ids[1] {
                out.push(TrayAction::OpenProject);
            } else if e.id == self.ids[2] {
                out.push(TrayAction::Quit);
            }
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use muda::ContextMenu;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBitmapImageRep, NSCellImagePosition, NSDeviceRGBColorSpace, NSImage, NSMenu, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
    use objc2_foundation::{NSSize, NSString};

    pub fn create_status_item(menu: &muda::Menu) -> Option<Retained<NSStatusItem>> {
        let mtm = MainThreadMarker::new()?;
        let png = include_bytes!("../assets/tray-template.png");
        let img = image::load_from_memory(png).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        let rgba = img.into_raw();
        let image = unsafe {
            let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                mtm.alloc::<NSBitmapImageRep>(),
                std::ptr::null_mut(),
                w as isize,
                h as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                (w * 4) as isize,
                32,
            )?;
            let dst = rep.bitmapData();
            if dst.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), dst, rgba.len());
            // the PNG is 2× (44 px for a 22 pt menu bar icon)
            let image = NSImage::initWithSize(mtm.alloc::<NSImage>(), NSSize::new(w as f64 / 2.0, h as f64 / 2.0));
            image.addRepresentation(&rep);
            image.setTemplate(true);
            image
        };
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            button.setImage(Some(&image));
            button.setImagePosition(NSCellImagePosition::ImageLeft);
        }
        let ns_menu = menu.ns_menu() as *mut NSMenu;
        if !ns_menu.is_null() {
            item.setMenu(Some(unsafe { &*ns_menu }));
        }
        Some(item)
    }

    pub fn set_title(item: &NSStatusItem, title: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(button) = item.button(mtm) {
            button.setTitle(&NSString::from_str(title.unwrap_or("")));
        }
    }
}
