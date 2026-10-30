//! A file on disk (`.xlsx`, `.docx` or `.pptx`) as the end of a link.
//!
//! The file is opened in a private engine session (it does not appear in its tab). Links read and
//! write that session; when a sync changed it, the file is written back. The file is watched: when
//! something else (Excel, Word, PowerPoint…) saves it, the session is reloaded and the links sync.
//!
//! Writing goes through the engine's own file writer, so features it does not model can be lost
//! the first time Alfadoc writes a file. A copy of the original is kept beside it as
//! `<name>.alfadoc-backup.<ext>` before that first write.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use alfadoc_adapter_deck::DeckEngine;
use alfadoc_adapter_grid::GridEngine;
use alfadoc_adapter_word::WordEngine;
use alfadoc_bridge::{BridgeError, CellChange, Engine, Region, RegionAccess};
use alfadoc_model::Anchor;
use gridcraft_engine::io::{base64_decode, base64_encode};
use serde_json::json;

/// What identifies one saved state of a file cheaply: its modification time and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { modified: meta.modified().ok()?, len: meta.len() })
}

/// The kinds of file a link can point into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Xlsx,
    Docx,
    Pptx,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Xlsx => "xlsx",
            Kind::Docx => "docx",
            Kind::Pptx => "pptx",
        }
    }
}

/// Which kind of file `path` names, if it is one this module can link.
pub fn kind_of(path: &str) -> Option<Kind> {
    let lower = path.trim().to_ascii_lowercase();
    [Kind::Xlsx, Kind::Docx, Kind::Pptx].into_iter().find(|k| lower.ends_with(&format!(".{}", k.ext())))
}

/// Where inside the file a new link points.
pub enum Spot {
    /// A sheet name and a cell range, for a workbook.
    Cells { sheet: String, range: String },
    /// The nth table (1-based, in reading order), for a document or a presentation.
    Table(usize),
}

enum Inner {
    Grid(GridEngine),
    Word(WordEngine),
    Deck(DeckEngine),
}

pub struct ExternalFile {
    inner: Inner,
    kind: Kind,
    stamp: Option<Stamp>,
    dirty: bool,
}

fn backup_path(path: &Path, kind: Kind) -> PathBuf {
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}.alfadoc-backup.{}", kind.ext()))
}

impl ExternalFile {
    /// Reads `path` into a fresh private session.
    pub fn open(path: &Path) -> Result<Self, String> {
        if cfg!(target_arch = "wasm32") {
            return Err("Linking a file on disk is not available in the browser.".into());
        }
        let kind = kind_of(&path.to_string_lossy()).ok_or_else(|| format!("{} is not an .xlsx, .docx or .pptx file", path.display()))?;
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let stamp = stamp_of(path);
        let name = path.file_name().map_or_else(|| format!("linked.{}", kind.ext()), |n| n.to_string_lossy().into_owned());
        let fail = |e: String| format!("cannot open {}: {e}", path.display());
        let inner = match kind {
            Kind::Xlsx => {
                let mut engine = GridEngine::from_session(gridcraft_engine::Session::new());
                engine.execute("file.open", &json!({ "name": name, "base64": base64_encode(&bytes) })).map_err(|e| fail(e.to_string()))?;
                Inner::Grid(engine)
            }
            Kind::Docx => {
                let doc = wordcraft_engine::io::open_bytes(&name, &bytes).map_err(fail)?;
                Inner::Word(WordEngine::from_session(wordcraft_engine::Session::new(doc)))
            }
            Kind::Pptx => {
                let mut engine = DeckEngine::from_session(deckcraft_engine::Session::new());
                engine.execute("file.openBytes", &json!({ "name": name, "data": base64_encode(&bytes) })).map_err(|e| fail(e.to_string()))?;
                Inner::Deck(engine)
            }
        };
        Ok(ExternalFile { inner, kind, stamp, dirty: false })
    }

    /// True when the file on disk is not the one this session was loaded from or last wrote.
    pub fn changed_on_disk(&self, path: &Path) -> bool {
        stamp_of(path) != self.stamp
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Marks the place a new link points at (a defined name, a table tag or a shape name) and
    /// returns the anchor that finds it again.
    pub fn anchor_for(&mut self, link_id: &str, spot: &Spot) -> Result<Anchor, String> {
        let name = format!("alfadoc_{link_id}");
        match (&mut self.inner, spot) {
            (Inner::Grid(g), Spot::Cells { sheet, range }) => {
                g.define_region(&name, sheet.trim(), range.trim()).map_err(|e| e.to_string())?;
                Ok(Anchor::GridName { name })
            }
            (Inner::Word(w), Spot::Table(n)) => {
                let tag = alfadoc_model::links::link_tag(link_id);
                w.tag_table(*n, &tag).map_err(|e| e.to_string())?;
                Ok(Anchor::WordControl { tag })
            }
            (Inner::Deck(d), Spot::Table(n)) => {
                let slide_id = d.name_nth_table(*n, &name).map_err(|e| e.to_string())?;
                Ok(Anchor::DeckShape { slide_id, shape_name: name })
            }
            _ => Err(format!("this is a .{} file; that kind of place does not fit it", self.kind.ext())),
        }
    }

    fn to_bytes(&mut self) -> Result<Vec<u8>, String> {
        match &mut self.inner {
            Inner::Grid(g) => {
                let saved = g.execute("file.saveBytes", &json!({ "format": "xlsx" })).map_err(|e| e.to_string())?;
                let text = saved["base64"].as_str().ok_or("the spreadsheet engine returned no data")?;
                base64_decode(text).ok_or_else(|| "the spreadsheet engine returned invalid data".to_string())
            }
            Inner::Word(w) => wordcraft_engine::io::save_bytes("linked.docx", &w.session().doc),
            Inner::Deck(d) => {
                let saved = d.execute("file.saveBytes", &json!({ "format": "pptx" })).map_err(|e| e.to_string())?;
                let text = saved["data"].as_str().ok_or("the presentation engine returned no data")?;
                base64_decode(text).ok_or_else(|| "the presentation engine returned invalid data".to_string())
            }
        }
    }

    /// Writes the session back to `path` (through a temporary file) and remembers the new stamp,
    /// so the watcher does not mistake Alfadoc's own write for an outside change.
    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let bytes = self.to_bytes()?;
        let backup = backup_path(path, self.kind);
        if path.exists() && !backup.exists() {
            std::fs::copy(path, &backup).map_err(|e| format!("cannot keep a backup of {}: {e}", path.display()))?;
        }
        let tmp = path.with_extension(format!("{}.alfadoc-tmp", self.kind.ext()));
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        self.stamp = stamp_of(path);
        self.dirty = false;
        Ok(())
    }
}

impl RegionAccess for ExternalFile {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        match &self.inner {
            Inner::Grid(g) => g.read_region(anchor),
            Inner::Word(w) => w.read_region(anchor),
            Inner::Deck(d) => d.read_region(anchor),
        }
    }

    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
        match &mut self.inner {
            Inner::Grid(g) => g.write_changes(anchor, changes)?,
            Inner::Word(w) => w.write_changes(anchor, changes)?,
            Inner::Deck(d) => d.write_changes(anchor, changes)?,
        }
        self.dirty = true;
        Ok(())
    }
}

/// Lends an [`ExternalFile`] to `sync_link` without giving it away.
pub struct Lend<'a>(pub &'a mut ExternalFile);

impl RegionAccess for Lend<'_> {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        self.0.read_region(anchor)
    }

    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
        self.0.write_changes(anchor, changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_word_and_powerpoint_files_are_linkable() {
        assert_eq!(kind_of("C:\\data\\Budget.XLSX"), Some(Kind::Xlsx));
        assert_eq!(kind_of("report.docx"), Some(Kind::Docx));
        assert_eq!(kind_of("deck.PPTX "), Some(Kind::Pptx));
        assert_eq!(kind_of("a.xls"), None);
        assert_eq!(kind_of("a.xlsm"), None);
        assert_eq!(kind_of("a.doc"), None);
    }

    #[test]
    fn the_backup_sits_beside_the_file() {
        let p = backup_path(Path::new("dir/Budget.xlsx"), Kind::Xlsx);
        assert_eq!(p, Path::new("dir/Budget.alfadoc-backup.xlsx"));
        assert_eq!(backup_path(Path::new("r.docx"), Kind::Docx), Path::new("r.alfadoc-backup.docx"));
    }

    #[test]
    fn a_table_in_a_docx_file_is_tagged_saved_and_found_again() {
        let dir = std::env::temp_dir().join(format!("alfadoc-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.docx");
        let mut w = WordEngine::new();
        w.execute("insert.table", &json!({ "rows": 2, "cols": 2 })).unwrap();
        std::fs::write(&path, wordcraft_engine::io::save_bytes("t.docx", &w.session().doc).unwrap()).unwrap();

        let mut x = ExternalFile::open(&path).unwrap();
        let anchor = x.anchor_for("abc12345", &Spot::Table(1)).unwrap();
        assert!(x.anchor_for("abc12345", &Spot::Table(2)).is_err());
        x.write_changes(&anchor, &[CellChange { row: 0, col: 0, value: Some(alfadoc_model::Value::Text("hi".into())) }]).unwrap();
        x.save(&path).unwrap();

        let again = ExternalFile::open(&path).unwrap();
        assert_eq!(again.read_region(&anchor).unwrap().cells.get(&(0, 0)), Some(&alfadoc_model::Value::Text("hi".into())));
        std::fs::remove_dir_all(&dir).ok();
    }
}
