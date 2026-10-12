//! `QuickLook/Preview.png`: the thumbnail Screen Studio writes next to every project. The recent
//! projects panel and Finder's Quick Look use it. Rendered headless from the project itself.

use std::path::Path;

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
    image::save_buffer(&out, &px, size.width, size.height, image::ColorType::Rgba8)?;
    Ok(())
}

/// `write_quicklook_preview` on a background thread; errors are logged, never fatal.
pub fn write_quicklook_preview_async(project: &Path) {
    let project = project.to_path_buf();
    std::thread::Builder::new()
        .name("quicklook-preview".into())
        .spawn(move || {
            if let Err(e) = write_quicklook_preview(&project) {
                tracing::warn!("QuickLook preview for {} failed: {e:#}", project.display());
            }
        })
        .ok();
}
