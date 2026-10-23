#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_panel;
mod app;
mod area_select;
mod window_select;
mod audio;
mod camera_bubble;
mod captions;
mod crop;
mod editor;
mod export;
mod masks;
mod mic_meter;
mod open_docs;
mod os;
mod permissions;
mod picker;
mod recorder;
mod sidebar;
mod theme;
mod timeline;
mod tray;
mod unsplash;
mod update;
mod vibrancy;
mod voice_over;
mod widgets;

/// eframe's defaults, except that the backends follow `sf_render::Gpu::default_backends`
/// (Direct3D 12 on Windows: the Vulkan path crashes inside some drivers before an adapter is
/// listed). `WGPU_BACKEND` still overrides.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::from_env().unwrap_or(sf_render::Gpu::default_backends());
    eframe::egui_wgpu::WgpuConfiguration { wgpu_setup: setup.into(), ..Default::default() }
}

fn main() -> eframe::Result {
    // The toolbar and HUD place themselves at absolute screen positions, which Wayland does not
    // allow; run the windows on XWayland. Recording still goes through the screen-cast portal.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    sf_recording::clock::init();
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    open_docs::install();
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).expect("app icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Linux desktops match the window to screenforge.desktop by this id
            .with_app_id("screenforge")
            .with_icon(icon)
            .with_inner_size(app::PICKER_SIZE)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_title("ScreenForge"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native("ScreenForge", options, Box::new(|cc| {
        let app = app::App::new(cc);
        Ok(Box::new(app))
    }))
}
