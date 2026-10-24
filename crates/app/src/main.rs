// No console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod editor;
mod logger;
mod view;

use estivi_core::config::Config;

slint::include_modules!();

fn select_backend(renderer: &str) -> Result<(), slint::PlatformError> {
    let mut selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into());
    // winit 0.30 has no Wayland data-device: the compositor pins the drag cursor
    // at the window edge. X11/XWayland implements XDND, so the pointer can enter.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        use slint::winit_030::{SlintEvent, winit};
        use winit::platform::x11::EventLoopBuilderExtX11;
        let mut builder = winit::event_loop::EventLoop::<SlintEvent>::with_user_event();
        builder.with_x11();
        selector = selector.with_winit_event_loop_builder(builder);
    }
    selector.select()
}

fn main() {
    let config_path = Config::location();
    logger::init(config_path.with_file_name("estivi.log"));
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        log::error!("configuración: {e:#}; se usan valores por defecto");
        Config::default()
    });
    // SLINT_BACKEND in the environment still wins, for troubleshooting.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        let renderer = if config.ui.gpu_render { "femtovg" } else { "software" };
        if let Err(e) = select_backend(renderer) {
            log::warn!("no se pudo elegir el renderizador {renderer}: {e}");
        }
    }
    if let Err(e) = app::run(config, config_path) {
        log::error!("{e:#}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Estivi")
            .set_description(format!("{e:#}"))
            .show();
        std::process::exit(1);
    }
}
