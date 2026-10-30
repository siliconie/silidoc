//! The browser entry point: the same shell as the desktop app, drawn by eframe's web runner
//! (WebGPU where the browser has it, WebGL2 otherwise).
//!
//! **Experimental.** What differs from the desktop app:
//! * Open and Import use the browser's file picker; Save, Save As and Export download a file.
//! * There is no file system, so a link cannot point at a file on disk.
//! * The editors' own Open/Save menus are not connected; use Alfadoc's buttons.

use eframe::wasm_bindgen::JsCast as _;

const CANVAS_ID: &str = "alfadoc_canvas";
const LOADING_ID: &str = "alfadoc_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let mut options = eframe::WebOptions::default();
        // Ask for no more than the browser's adapter offers, so older GPUs start.
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
            let default_descriptor = std::sync::Arc::clone(&setup.device_descriptor);
            setup.device_descriptor = std::sync::Arc::new(move |adapter| {
                let mut descriptor = default_descriptor(adapter);
                descriptor.required_limits = descriptor.required_limits.or_worse_values_from(&adapter.limits());
                descriptor
            });
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(|cc| {
                    cc.egui_ctx.set_embed_viewports(true);
                    Ok(Box::new(crate::shell::Shell::new(crate::shell::Tab::Sheet, None)))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>Alfadoc failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}
