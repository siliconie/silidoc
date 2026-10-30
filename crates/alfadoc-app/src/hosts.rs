//! Builds each engine's UI app for the Alfadoc window.
//!
//! The engines expect a host to provide platform services. On the desktop that is native Open and
//! Save dialogs through `rfd`, plus the system clipboard (HTML too), printing and audio from
//! `platform.rs`. Auto-recovery is done by the shell for the whole package (`recovery.rs`).

#[cfg(not(target_arch = "wasm32"))]
use crate::platform;
use deckcraft_ui_egui::SlideApp;
use gridcraft_ui_egui::SheetApp;
use wordcraft_ui_egui::WordApp;

#[cfg(not(target_arch = "wasm32"))]
fn path_string(p: std::path::PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(not(target_arch = "wasm32"))]
const SHEET_EXTS: &[&str] = &["gridcraft", "xlsx", "xlsm", "xltx", "xls", "xlsb", "ods", "csv", "tsv", "txt", "json"];
#[cfg(not(target_arch = "wasm32"))]
const SLIDE_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx"];
#[cfg(not(target_arch = "wasm32"))]
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];

// ---------------------------------------------------------------- GridCraft

pub fn grid_app(ctx: &egui::Context) -> SheetApp {
    let mut session = gridcraft_engine::Session::new();
    session.new_workbook();
    let app = SheetApp::new(session, grid_services());
    SheetApp::setup_context_for_language(ctx, app.ui.dark, app.ui.language);
    app
}

/// In the browser the editors' own Open/Save menus have no file system to talk to; Alfadoc's
/// Open, Save, Import and Export buttons take their place.
#[cfg(target_arch = "wasm32")]
fn grid_services() -> gridcraft_ui_egui::Services {
    gridcraft_ui_egui::Services::default()
}

#[cfg(not(target_arch = "wasm32"))]
fn grid_services() -> gridcraft_ui_egui::Services {
    // Keep the owner alive: X11 and Wayland serve clipboard data from this handle.
    let clipboard = platform::clipboard::handle();
    let (html_clipboard, text_clipboard) = (clipboard.clone(), clipboard.clone());
    gridcraft_ui_egui::Services {
        pick_open: Some(Box::new(|| rfd::FileDialog::new().add_filter("Spreadsheets", SHEET_EXTS).pick_file().map(path_string))),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(path_string))),
        pick_picture: Some(Box::new(|| rfd::FileDialog::new().add_filter("Pictures", PICTURE_EXTS).pick_file().map(path_string))),
        copy_html: Some(Box::new(move |html, text| platform::clipboard::set_html(&clipboard, html, text))),
        paste_html: Some(Box::new(move || platform::clipboard::html(&html_clipboard))),
        paste_text: Some(Box::new(move || platform::clipboard::text(&text_clipboard))),
        printers: Some(platform::print::grid::discover()),
        print_native: Some(platform::print::grid::submit()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- WordCraft

pub fn word_app() -> WordApp {
    // The system region picks the paper (A4 or Letter), the units and the decimal separator.
    let region = wordcraft_ui_egui::i18n::system_region();
    let doc = wordcraft_engine::blank_document(&region);
    let mut session = wordcraft_engine::Session::new(doc);
    session.system_region = region;
    WordApp::new(session, word_services())
}

/// In the browser the editors' own Open/Save menus have no file system to talk to; Alfadoc's
/// Open, Save, Import and Export buttons take their place.
#[cfg(target_arch = "wasm32")]
fn word_services() -> wordcraft_ui_egui::Services {
    wordcraft_ui_egui::Services::default()
}

#[cfg(not(target_arch = "wasm32"))]
fn word_services() -> wordcraft_ui_egui::Services {
    let clipboard = platform::clipboard::handle();
    let (text_clipboard, html_clipboard, set_clipboard) = (clipboard.clone(), clipboard.clone(), clipboard);
    wordcraft_ui_egui::Services {
        pick_open: Some(Box::new(|purpose: &str| {
            let mut dialog = rfd::FileDialog::new();
            for (label, exts) in wordcraft_ui_egui::open_filters(purpose) {
                // "*" means any file: a dialog without that filter already offers it.
                if !exts.contains(&"*") {
                    dialog = dialog.add_filter(*label, exts);
                }
            }
            dialog.pick_file().map(path_string)
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(path_string))),
        printers: Some(platform::print::word::discover()),
        print_native: Some(platform::print::word::submit()),
        clipboard_text: Some(Box::new(move || platform::clipboard::text(&text_clipboard))),
        clipboard_html: Some(Box::new(move || platform::clipboard::html(&html_clipboard))),
        // The third argument is MathML for a copied equation; the plain HTML flavour is enough here.
        set_clipboard_html: Some(Box::new(move |html, text, _mathml| platform::clipboard::set_html(&set_clipboard, html, text).is_ok())),
        ..Default::default()
    }
}

// ---------------------------------------------------------------- DeckCraft

pub fn deck_app(ctx: &egui::Context) -> SlideApp {
    deckcraft_ui_egui::install_ime_pass_hooks(ctx);
    let mut app = SlideApp::new(deckcraft_engine::Session::new(), deck_services());
    if let Err(e) = app.run("file.new", serde_json::json!({})) {
        log::error!("new presentation: {e}");
    }
    app
}

/// In the browser the editors' own Open/Save menus have no file system to talk to; Alfadoc's
/// Open, Save, Import and Export buttons take their place.
#[cfg(target_arch = "wasm32")]
fn deck_services() -> deckcraft_ui_egui::Services {
    deckcraft_ui_egui::Services::default()
}

#[cfg(not(target_arch = "wasm32"))]
fn deck_services() -> deckcraft_ui_egui::Services {
    let clipboard = platform::clipboard::handle();
    let (html_clipboard, set_clipboard) = (clipboard.clone(), clipboard);
    deckcraft_ui_egui::Services {
        pick_open: Some(Box::new(|purpose: &str| {
            let dialog = rfd::FileDialog::new();
            let dialog = match purpose {
                "picture" => dialog.add_filter("Pictures", PICTURE_EXTS),
                "audio" => dialog.add_filter("Audio", &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "aif", "aiff"]),
                "video" => dialog.add_filter("Video", &["mp4", "m4v", "mov", "webm", "mkv"]),
                "theme" => dialog.add_filter("Themes and presentations", &["thmx", "pptx", "potx", "ppsx"]),
                _ => dialog.add_filter("Presentations", SLIDE_EXTS).add_filter("Outline", &["txt", "md"]),
            };
            dialog.pick_file().map(path_string)
        })),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(path_string))),
        read: Some(Box::new(|p: &str| std::fs::read(p).map_err(|e| e.to_string()))),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        clipboard_html: Some(Box::new(move || platform::clipboard::html(&html_clipboard))),
        clipboard_set_html: Some(Box::new(move |html, text| platform::clipboard::set_html(&set_clipboard, html, text).is_ok())),
        audio_out: Some(Box::new(platform::audio::CpalOut::default())),
        ..Default::default()
    }
}
