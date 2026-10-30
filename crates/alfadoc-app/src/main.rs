//! Alfadoc: GridCraft, WordCraft and DeckCraft in one window.
//!
//! `alfadoc [--tab sheet|doc|slides]`

mod ctxstate;
mod graphics;
mod hosts;
mod shell;

fn main() -> eframe::Result {
    let start = shell::Tab::from_args(std::env::args().skip(1));
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
            Ok(Box::new(shell::Shell::new(start)))
        }),
    )
}
