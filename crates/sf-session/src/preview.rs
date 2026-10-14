//! `QuickLook/Preview.png`: the thumbnail Screen Studio writes next to every project. The recent
//! projects panel and Finder's Quick Look use it. Rendered headless from the project itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use sf_render::{Compositor, Gpu};

use crate::Session;

/// Height of the preview image in pixels; width follows the project aspect.
pub const PREVIEW_HEIGHT: u32 = 540;

/// Render one representative frame and write it to `QuickLook/Preview.png` inside `project`.
/// The frame is taken 10 % into the output, or at 1 s, whichever is later, so title screens and
/// the first zoom-in are usually visible.
pub fn write_quicklook_preview(project: &Path) -> Result<()> {
    let gpu = Gpu::headless()?;
    let mut session = Session::open(project, gpu.clone())?;
    let mut comp = Compositor::new(gpu);
    let size = session.output_size(PREVIEW_HEIGHT);
    let total = session.timeline.duration_ms();
    let at = (total * 0.1).max(1000.0).min((total - 100.0).max(0.0));
    // warm the camera/cursor springs so the frame looks like playback
    let mut t = (at - 2000.0).max(0.0);
    let mut desc = None;
    while t <= at + 1e-6 {
        desc = Some(session.frame_desc_at(t, size)?);
        t += 1000.0 / 60.0;
    }
    let Some(desc) = desc else { anyhow::bail!("empty project") };
    comp.render(&desc);
    let px = comp.read_pixels()?;
    let out = project.join(sf_project::package::QUICKLOOK_PREVIEW);
    std::fs::create_dir_all(out.parent().unwrap())?;
    // write next to it and rename, so readers never see a half-written file
    let tmp = out.with_file_name("Preview.tmp.png");
    image::save_buffer(&tmp, &px, size.width, size.height, image::ColorType::Rgba8)?;
    std::fs::rename(&tmp, &out)?;
    Ok(())
}

/// Projects with a preview being written in the background, with the number of writes.
static PENDING: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

fn set_pending(project: &Path, delta: isize) {
    let mut guard = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let n = map.entry(project.to_path_buf()).or_insert(0);
    *n = (*n as isize + delta).max(0) as usize;
    if *n == 0 {
        map.remove(project);
    }
}

/// True while `write_quicklook_preview_async` is still writing a preview for `project`.
pub fn is_pending(project: &Path) -> bool {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(project))
}

/// `write_quicklook_preview` on a background thread; errors are logged, never fatal.
pub fn write_quicklook_preview_async(project: &Path) {
    let project = project.to_path_buf();
    set_pending(&project, 1);
    let spawned = std::thread::Builder::new()
        .name("quicklook-preview".into())
        .spawn({
            let project = project.clone();
            move || {
                if let Err(e) = write_quicklook_preview(&project) {
                    tracing::warn!("QuickLook preview for {} failed: {e:#}", project.display());
                }
                set_pending(&project, -1);
            }
        });
    if spawned.is_err() {
        set_pending(&project, -1);
    }
}
