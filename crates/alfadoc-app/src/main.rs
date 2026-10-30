//! Alfadoc: GridCraft, WordCraft and DeckCraft in one window.
//!
//! `alfadoc [--tab sheet|doc|slides|data] [file.alfadoc]`
//!
//! Release builds on Windows run without a console window.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod ctxstate;
mod data_view;
mod external;
#[cfg(not(target_arch = "wasm32"))]
mod graphics;
mod hosts;
mod linking;
mod package;
mod shell;
#[cfg(target_arch = "wasm32")]
mod web;

/// The browser build starts from `web::start`; this binary has no native entry point there.
#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let start = shell::Tab::from_args(std::env::args().skip(1));
    let startup = shell::Tab::file_from_args(std::env::args().skip(1));
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Alfadoc")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            .with_app_id("org.alfasilicon.alfadoc"),
        ..Default::default()
    };
    graphics::configure(&mut options);
    eframe::run_native(
        "Alfadoc",
        options,
        Box::new(move |cc| {
            // WordCraft opens each document in its own OS window when it can. Alfadoc has one
            // window with tabs, so ask egui to draw every "extra window" inside this one.
            cc.egui_ctx.set_embed_viewports(true);
            Ok(Box::new(shell::Shell::new(start, startup)))
        }),
    )
}
