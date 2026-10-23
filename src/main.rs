// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod ai;
mod app;
mod camera_raw;
mod cms;
mod color;
mod compose;
mod config;
mod diag;
mod document;
mod elements;
mod export;
mod filters;
mod grabcut;
mod guides;
mod handles;
mod i18n;
mod liquify;
mod mask_ops;
mod paint;
mod pixel_ops;
mod recent;
mod retouch;
mod rotate;
mod scissors;
mod selection;
mod snap;
mod swatches;
mod text;
mod ui_kit;
mod vector;
mod xform;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    diag::start();
    diag::install_panic_hook();
    i18n::init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    let viewport = egui::ViewportBuilder::default()
        .with_title("Atelier")
        // Window and taskbar icon; on Wayland the icon comes from the
        // .desktop file matching this app id.
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                .expect("valid icon PNG"),
        )
        .with_app_id("io.github.Shhu.Atelier")
        .with_inner_size([1200.0, 800.0])
        .with_min_inner_size([600.0, 400.0])
        .with_drag_and_drop(true);
    // Own title bar (src/app/titlebar.rs). On macOS the system's window
    // buttons stay, over it; elsewhere there is no system frame at all.
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_decorations(false);
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    // The GPU's real limits, not egui's fixed 8192 px textures (more than
    // some GPUs have): images are split into textures of that size at most.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        let descriptor = std::sync::Arc::clone(&setup.device_descriptor);
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = descriptor(adapter);
            let limits = adapter.limits();
            descriptor.required_limits.max_texture_dimension_2d = limits.max_texture_dimension_2d;
            descriptor.required_limits.max_buffer_size = limits.max_buffer_size;
            descriptor
        });
    }

    eframe::run_native(
        "Atelier",
        options,
        Box::new(|cc| Ok(Box::new(app::AtelierApp::new(cc, path)))),
    )
}
