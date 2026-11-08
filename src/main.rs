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
mod memory;
mod paint;
mod patterns;
mod pixel_ops;
mod recent;
mod retouch;
mod rotate;
mod scissors;
mod selection;
mod snap;
mod store;
mod swatches;
mod system;
#[cfg(test)]
mod testing;
mod text;
mod ui_kit;
mod vector;
mod xform;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    #[cfg(windows)]
    restrict_dll_search();
    let _ = memory::tune();
    diag::start();
    diag::install_panic_hook();
    i18n::init();
    // The first file opens at start, the others (several picked in the
    // file manager) as tabs once the window is up.
    let mut paths = std::env::args_os().skip(1).map(PathBuf::from);
    let path = paths.next();
    system::open(paths);

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
        // Room for every menu (the French ones are the widest).
        .with_min_inner_size([660.0, 400.0])
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
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(system::macos::configure));
    }
    // The GPU's real limits, not egui's fixed 8192 px textures (more than
    // some GPUs have): images are split into textures of that size at most.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        // On Windows, Direct3D 12 rather than Vulkan: with Vulkan, resizing
        // the window can leave its surface stuck at the old size (seen on
        // NVIDIA). OpenGL stays as a last resort for GPUs without D3D12.
        // WGPU_BACKEND (vulkan, gl…) still chooses, for a driver that fails.
        #[cfg(windows)]
        if std::env::var_os("WGPU_BACKEND").is_none() {
            setup.instance_descriptor.backends =
                eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL;
        }
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
    .inspect_err(system::startup_failed)
}

/// DLLs loaded by name come from System32 (and the folders the app adds),
/// never from the .exe's folder or the current one, where a DLL could be
/// planted (Downloads). The .exe's own imports get the same rule at link
/// time (`/DEPENDENTLOADFLAG`, build.rs).
#[cfg(windows)]
fn restrict_dll_search() {
    const LOAD_LIBRARY_SEARCH_USER_DIRS: u32 = 0x400;
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetDefaultDllDirectories(flags: u32) -> i32;
        fn SetDllDirectoryW(path: *const u16) -> i32;
    }
    // SAFETY: plain flags, and a zero-terminated empty string: it takes
    // the current folder out of the search of older calls.
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS);
        SetDllDirectoryW([0u16].as_ptr());
    }
}
