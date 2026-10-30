//! Moves the three editors' documents in and out of a `.alfadoc` package.
//!
//! Each editor's document is stored in its own interchange format inside the package
//! (`sheets/main.xlsx`, `document/main.docx`, `slides/main.pptx`), so a package is also readable by
//! anything that opens those formats. Only the editors that have been opened are written; parts of
//! editors that were not touched are kept as loaded.
//!
//! The engines remember a file's name as its save location. A document loaded from a package has
//! that name cleared again, so the editors' own Save never writes a stray `main.xlsx` next to the
//! program.

use alfadoc_model::Alfadoc;
use deckcraft_ui_egui::SlideApp;
use gridcraft_engine::io::{base64_decode, base64_encode};
use gridcraft_ui_egui::SheetApp;
use serde_json::json;
use wordcraft_ui_egui::WordApp;

pub const SHEET_PART: &str = "sheets/main.xlsx";
pub const DOC_PART: &str = "document/main.docx";
pub const DECK_PART: &str = "slides/main.pptx";

/// Milliseconds since 1970.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

/// A fresh package for a document that has never been saved.
pub fn new_package() -> Alfadoc {
    Alfadoc::new(&new_id(), "Untitled", now_ms())
}

/// A random-looking, version-4-shaped identifier (no external RNG: the clock and a counter are
/// enough to keep two documents created by one user apart).
pub(crate) fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let count = COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mix = |mut x: u64| {
        x ^= x >> 30;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    let hi = mix((nanos as u64) ^ count);
    let lo = mix(((nanos >> 64) as u64) ^ count.rotate_left(17) ^ hi);
    format!("{:08x}-{:04x}-4{:03x}-{:01x}{:03x}-{:012x}", hi >> 32, (hi >> 16) & 0xffff, hi & 0xfff, 8 + ((lo >> 60) & 3), (lo >> 48) & 0xfff, lo & 0xffff_ffff_ffff)
}

/// Which editor opens a file with this extension (case-insensitive), for Import.
pub fn editor_for(extension: &str) -> Option<crate::shell::Tab> {
    use crate::shell::Tab;
    match extension.to_ascii_lowercase().as_str() {
        "xlsx" | "xlsm" | "xltx" | "xls" | "xlsb" | "ods" | "csv" | "tsv" | "gridcraft" => Some(Tab::Sheet),
        "docx" | "docm" | "dotx" | "doc" | "odt" | "rtf" | "wordcraft" => Some(Tab::Document),
        "pptx" | "potx" | "ppsx" | "deckcraft" => Some(Tab::Slides),
        _ => None,
    }
}

// ---------------------------------------------------------------- GridCraft

pub fn save_sheet(app: &mut SheetApp) -> Result<Vec<u8>, String> {
    let r = app.session.execute("file.saveBytes", json!({ "format": "xlsx" })).map_err(|e| e.to_string())?;
    let text = r["base64"].as_str().ok_or("the spreadsheet engine returned no data")?;
    base64_decode(text).ok_or_else(|| "the spreadsheet engine returned invalid data".to_string())
}

pub fn load_sheet(app: &mut SheetApp, name: &str, bytes: &[u8]) -> Result<(), String> {
    app.session.execute("file.open", json!({ "name": name, "base64": base64_encode(bytes) })).map_err(|e| e.to_string())?;
    if let Ok(doc) = app.session.doc_mut() {
        doc.path = None;
    }
    Ok(())
}

// ---------------------------------------------------------------- WordCraft

pub fn save_document(app: &WordApp) -> Result<Vec<u8>, String> {
    wordcraft_engine::io::save_bytes("Alfadoc.docx", &app.session.doc)
}

pub fn load_document(app: &mut WordApp, name: &str, bytes: &[u8]) -> Result<(), String> {
    app.session.run("file.open", &json!({ "path": name, "data": base64_encode(bytes) })).map_err(|e| format!("{e:?}"))?;
    app.session.path = None;
    Ok(())
}

// ---------------------------------------------------------------- DeckCraft

pub fn save_deck(app: &mut SlideApp) -> Result<Vec<u8>, String> {
    let r = app.session.execute("file.saveBytes", &json!({ "format": "pptx" })).map_err(|e| e.to_string())?;
    let text = r["data"].as_str().ok_or("the presentation engine returned no data")?;
    base64_decode(text).ok_or_else(|| "the presentation engine returned invalid data".to_string())
}

pub fn load_deck(app: &mut SlideApp, name: &str, bytes: &[u8]) -> Result<(), String> {
    app.session.execute("file.openBytes", &json!({ "name": name, "data": base64_encode(bytes) })).map_err(|e| e.to_string())?;
    if let Ok(doc) = app.session.doc_mut() {
        doc.path = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_pick_the_editor() {
        use crate::shell::Tab;
        assert_eq!(editor_for("XLSX"), Some(Tab::Sheet));
        assert_eq!(editor_for("docx"), Some(Tab::Document));
        assert_eq!(editor_for("pptx"), Some(Tab::Slides));
        assert_eq!(editor_for("zip"), None);
    }

    #[test]
    fn ids_look_like_uuids_and_differ() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_eq!(a.as_bytes()[14], b'4');
    }
}
