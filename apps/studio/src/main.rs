#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod area_select;
mod window_select;
mod audio;
mod editor;
mod export;
mod mic_meter;
mod permissions;
mod picker;
mod recorder;
mod sidebar;
mod theme;
mod timeline;
mod tray;
mod vibrancy;
mod widgets;

fn main() -> eframe::Result {
    sf_recording::clock::init();
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).expect("app icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon)
            .with_inner_size(app::PICKER_SIZE)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_title("ScreenForge"),
        renderer: eframe::Renderer::Wgpu,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native("ScreenForge", options, Box::new(|cc| {
        let app = app::App::new(cc);
        Ok(Box::new(app))
    }))
}
