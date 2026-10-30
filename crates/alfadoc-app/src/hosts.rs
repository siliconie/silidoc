//! Builds each engine's UI app for the Alfadoc window.
//!
//! The engines expect a host to provide platform services (file dialogs, file I/O). This is the
//! minimum a desktop host needs: native Open and Save dialogs through `rfd`. Clipboard bridges,
//! printing, audio and auto-recovery that the engines' own apps wire up are not connected yet;
//! the editors work without them (see docs/SHELL.md for the list).

use deckcraft_ui_egui::SlideApp;
use gridcraft_ui_egui::SheetApp;
use wordcraft_ui_egui::WordApp;

fn path_string(p: std::path::PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

const SHEET_EXTS: &[&str] = &["gridcraft", "xlsx", "xlsm", "xltx", "xls", "xlsb", "ods", "csv", "tsv", "txt", "json"];
const SLIDE_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];

// ---------------------------------------------------------------- GridCraft

pub fn grid_app(ctx: &egui::Context) -> SheetApp {
    let mut session = gridcraft_engine::Session::new();
    session.new_workbook();
    let app = SheetApp::new(session, grid_services());
    SheetApp::setup_context_for_language(ctx, app.ui.dark, app.ui.language);
    app
}

fn grid_services() -> gridcraft_ui_egui::Services {
    gridcraft_ui_egui::Services {
        pick_open: Some(Box::new(|| rfd::FileDialog::new().add_filter("Spreadsheets", SHEET_EXTS).pick_file().map(path_string))),
        pick_save: Some(Box::new(|name: &str| rfd::FileDialog::new().set_file_name(name).save_file().map(path_string))),
        pick_picture: Some(Box::new(|| rfd::FileDialog::new().add_filter("Pictures", PICTURE_EXTS).pick_file().map(path_string))),
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

fn word_services() -> wordcraft_ui_egui::Services {
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

fn deck_services() -> deckcraft_ui_egui::Services {
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
        ..Default::default()
    }
}
