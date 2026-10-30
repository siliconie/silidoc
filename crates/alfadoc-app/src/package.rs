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

/// The folders the parts live in. An editor with several documents open stores the one that was
/// active as `main.<ext>` and the others as `doc2.<ext>`, `doc3.<ext>`, …
pub const SHEET_DIR: &str = "sheets/";
pub const DOC_DIR: &str = "document/";
pub const DECK_DIR: &str = "slides/";

/// Replaces the parts under `prefix` with `docs` (the active document first).
pub fn put_parts(parts: &mut std::collections::BTreeMap<String, Vec<u8>>, prefix: &str, ext: &str, docs: Vec<Vec<u8>>) {
    parts.retain(|name, _| !name.starts_with(prefix));
    for (i, bytes) in docs.into_iter().enumerate() {
        let name = if i == 0 { format!("{prefix}main.{ext}") } else { format!("{prefix}doc{}.{ext}", i + 1) };
        parts.insert(name, bytes);
    }
}

/// The parts under `prefix` in the order to load them: the others first and `main` last, so the
/// document that was active when the package was saved is the active one again.
pub fn parts_for(parts: &std::collections::BTreeMap<String, Vec<u8>>, prefix: &str) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(String, Vec<u8>)> = parts.iter().filter(|(name, _)| name.starts_with(prefix)).map(|(n, b)| (n.clone(), b.clone())).collect();
    found.sort_by_key(|(name, _)| name.rsplit('/').next().is_some_and(|f| f.starts_with("main.")));
    found
}

/// Milliseconds since 1970.
pub fn now_ms() -> u64 {
    u64::try_from(unix_nanos() / 1_000_000).unwrap_or(0)
}

/// Nanoseconds since 1970. (`SystemTime::now()` panics in the browser, which has its own clock.)
fn unix_nanos() -> u128 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() * 1_000_000.0) as u128
    }
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
    let nanos = unix_nanos();
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

/// Saves every open workbook, the active one first.
pub fn save_all_sheets(app: &mut SheetApp) -> Result<Vec<Vec<u8>>, String> {
    let was = app.session.active_index();
    let count = app.session.documents().len();
    let order: Vec<usize> = std::iter::once(was).chain((0..count).filter(|i| *i != was)).collect();
    let (mut out, mut failed) = (vec![], None);
    for i in order {
        app.session.set_active(i);
        match save_sheet(app) {
            Ok(bytes) => out.push(bytes),
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    app.session.set_active(was);
    failed.map_or(Ok(out), Err)
}

/// Opens a workbook. A workbook the editor started with and nobody touched is closed afterwards,
/// so opening a file replaces the blank one instead of sitting beside it.
pub fn load_sheet(app: &mut SheetApp, name: &str, bytes: &[u8]) -> Result<(), String> {
    let blank = match app.session.documents() {
        [only] if only.revision == 0 && only.path.is_none() => Some(only.uid),
        _ => None,
    };
    app.session.execute("file.open", json!({ "name": name, "base64": base64_encode(bytes) })).map_err(|e| e.to_string())?;
    if let Ok(doc) = app.session.doc_mut() {
        doc.path = None;
    }
    if let Some(index) = blank.and_then(|uid| app.session.document_index(uid)) {
        app.session.close_document(index);
    }
    Ok(())
}

// ---------------------------------------------------------------- WordCraft

pub fn save_document(app: &WordApp) -> Result<Vec<u8>, String> {
    wordcraft_engine::io::save_bytes("Alfadoc.docx", &app.session.doc)
}

/// Saves every open document, the active one first.
pub fn save_all_documents(app: &mut WordApp) -> Result<Vec<Vec<u8>>, String> {
    let was = app.session.document_id();
    let ids: Vec<u64> = std::iter::once(was).chain(app.session.documents().iter().map(|d| d.id).filter(|id| *id != was)).collect();
    let (mut out, mut failed) = (vec![], None);
    for id in ids {
        if let Err(e) = app.session.activate(id) {
            failed = Some(format!("{e:?}"));
            break;
        }
        match save_document(app) {
            Ok(bytes) => out.push(bytes),
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    let _ = app.session.activate(was);
    failed.map_or(Ok(out), Err)
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

/// Saves every open presentation, the active one first.
pub fn save_all_decks(app: &mut SlideApp) -> Result<Vec<Vec<u8>>, String> {
    let was = app.session.active_index();
    let count = app.session.documents().len();
    let order: Vec<usize> = was.into_iter().chain((0..count).filter(|i| Some(*i) != was)).collect();
    let (mut out, mut failed) = (vec![], None);
    for i in order {
        app.session.set_active(i);
        match save_deck(app) {
            Ok(bytes) => out.push(bytes),
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    if let Some(was) = was {
        app.session.set_active(was);
    }
    failed.map_or(Ok(out), Err)
}

/// Opens a presentation. One the editor started with and nobody touched is closed afterwards.
pub fn load_deck(app: &mut SlideApp, name: &str, bytes: &[u8]) -> Result<(), String> {
    let blank = match app.session.documents() {
        [only] if only.revision == 0 && only.path.is_none() => Some(only.uid),
        _ => None,
    };
    app.session.execute("file.openBytes", &json!({ "name": name, "data": base64_encode(bytes) })).map_err(|e| e.to_string())?;
    if let Ok(doc) = app.session.doc_mut() {
        doc.path = None;
    }
    if let Some(index) = blank.and_then(|uid| app.session.documents().iter().position(|d| d.uid == uid)) {
        app.session.close_document(index);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_documents_are_stored_and_found_again_with_main_last() {
        let mut parts = std::collections::BTreeMap::new();
        parts.insert("sheets/old.xlsx".to_string(), vec![9]);
        put_parts(&mut parts, SHEET_DIR, "xlsx", vec![vec![1], vec![2], vec![3]]);
        assert_eq!(parts.keys().cloned().collect::<Vec<_>>(), ["sheets/doc2.xlsx", "sheets/doc3.xlsx", "sheets/main.xlsx"]);
        let found = parts_for(&parts, SHEET_DIR);
        assert_eq!(found.last().map(|(n, b)| (n.as_str(), b.clone())), Some(("sheets/main.xlsx", vec![1])));
        assert_eq!(found.len(), 3);
        assert!(parts_for(&parts, DOC_DIR).is_empty());
    }

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
