//! WordCraft behind Alfadoc's traits: a **tagged table** in the document is a linkable region.
//!
//! A link points at a table by `Anchor::WordControl { tag }`. The tag is stored in the table's
//! *Alt Text description* (Table Properties › Alt Text, `w:tblDescription`), for example
//! `alfadoc:link=L1`. That text is saved in `.docx`, so a link survives a round trip through Word.
//! (Word's content controls cannot wrap a table without leaving the rest of the document
//! ambiguous, so the alt text carries the tag instead.)
//!
//! Cells hold text. Values travel as text with the stable mapping in [`alfadoc_bridge::textcell`].
//!
//! Writes use WordCraft's own commands (`text.insert`, `text.delete`) with the cell's text
//! selected, so undo, track changes and document protection all apply. The user's selection is
//! put back afterwards. One sync pass is one undo step.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::borrow::BorrowMut;
use alfadoc_bridge::{BridgeError, CellChange, Engine, EngineKind, Region, RegionAccess, TableChoice, preview_of, textcell};
use alfadoc_model::variant::{variant_tag, visible};
use alfadoc_model::{Anchor, DimVector};
use serde_json::{Value as Json, json};
use wordcraft_engine::doc::{Block, Blocks, ControlRange, Path, Pos, StoryRef, Table};
use wordcraft_engine::{CmdError, Selection, Session};

/// The most cells one link may cover.
pub const MAX_REGION_CELLS: usize = 1_000_000;
/// Tables nest at most this deep when looking one up by tag.
const MAX_TABLE_DEPTH: usize = 24;

/// A tagged table, as listed for the "link this table" picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub tag: String,
    pub rows: usize,
    pub cols: usize,
}

/// `S` is the session it works on: an owned [`Session`] (the default), or `&mut Session` to work on
/// the one a live editor window holds ([`WordEngine::borrowed`]).
pub struct WordEngine<S = Session> {
    session: S,
}

impl WordEngine {
    /// A session over a new blank document.
    pub fn new() -> Self {
        WordEngine { session: Session::new(wordcraft_engine::doc::Document::new()) }
    }

    pub fn from_session(session: Session) -> Self {
        WordEngine { session }
    }
}

impl<'a> WordEngine<&'a mut Session> {
    /// Works on a session somebody else owns, for as long as the borrow lasts.
    pub fn borrowed(session: &'a mut Session) -> Self {
        WordEngine { session }
    }
}

impl<S: BorrowMut<Session>> WordEngine<S> {
    fn s(&self) -> &Session {
        let r: &Session = self.session.borrow();
        r
    }

    fn sm(&mut self) -> &mut Session {
        let r: &mut Session = self.session.borrow_mut();
        r
    }

    pub fn session(&self) -> &Session {
        self.s()
    }

    pub fn session_mut(&mut self) -> &mut Session {
        self.sm()
    }

    /// Every table that carries a tag starting with `alfadoc:` (top-level and nested).
    pub fn tables(&self) -> Vec<TableRef> {
        let mut out = vec![];
        if let Some(body) = self.s().doc.story(StoryRef::Body) {
            collect(body, &[], 0, &mut out);
        }
        out
    }

    /// Tags the table the caret is in, so a link can find it. Replaces any earlier tag.
    pub fn tag_selected_table(&mut self, tag: &str) -> Result<(), BridgeError> {
        self.run("table.altText", &json!({ "text": tag })).map(|_| ())
    }

    /// Every table of the body (tagged or not), for a picker.
    pub fn table_choices(&self) -> Vec<TableChoice> {
        self.all_table_paths()
            .iter()
            .enumerate()
            .filter_map(|(i, path)| {
                let table = self.table(path).ok()?;
                let (rows, cols) = dimensions(table);
                let first = table.rows.first().map(|r| r.cells.iter().map(|c| cell_text(&c.blocks)).collect::<Vec<_>>()).unwrap_or_default();
                Some(TableChoice { nth: i + 1, rows, cols, preview: preview_of(first.into_iter()), place: String::new() })
            })
            .collect()
    }

    /// Wraps the selected text in a plain-text content control tagged `tag`: an **inline field**
    /// a link can keep in step with one cell. With nothing selected the field starts empty.
    pub fn insert_field(&mut self, tag: &str) -> Result<(), BridgeError> {
        self.run("developer.plainText", &json!({ "tag": tag, "title": "Alfadoc link" })).map(|_| ())
    }

    /// Wraps the selected text in a **variant block**: it belongs to the audience or language
    /// named by `variant` (`alfadoc:variant={"audience":"client"}`).
    pub fn mark_variant(&mut self, variant: &DimVector) -> Result<(), BridgeError> {
        self.run("developer.richText", &json!({ "tag": variant_tag(variant), "title": "Variant" })).map(|_| ())
    }

    /// Deletes every variant block that does not belong to `active` (text and all) and returns
    /// how many went. One undo brings them all back.
    pub fn keep_variants(&mut self, active: &DimVector) -> Result<usize, BridgeError> {
        let mut doomed: Vec<ControlRange> = self.s().doc.control_ranges(StoryRef::Body).into_iter().filter(|r| !visible(Some(r.control.tag.as_str()), active)).collect();
        // Last in the document first, so the positions of the others stay right.
        doomed.sort_by(|a, b| (&b.start.path, b.start.off).cmp(&(&a.start.path, a.start.off)));
        let saved = self.s().sel.clone();
        let mut result = Ok(());
        for (i, r) in doomed.iter().enumerate() {
            let after_end = Pos { off: r.end.off + '\u{FFFC}'.len_utf8(), ..r.end.clone() };
            self.sm().sel = Selection { anchor: r.start.clone(), focus: after_end };
            if i > 0 {
                self.sm().join_next_undo();
            }
            if let Err(e) = self.run("text.delete", &json!({})) {
                result = Err(e);
                break;
            }
        }
        self.sm().sel = saved;
        result.map(|()| doomed.len())
    }

    /// The inline field tagged `tag`, if the body has one.
    fn field_range(&self, tag: &str) -> Option<ControlRange> {
        self.s().doc.control_ranges(StoryRef::Body).into_iter().find(|r| r.control.tag == tag)
    }

    /// The field behind `anchor`, when the tag names a field and no table.
    fn field_for(&self, anchor: &Anchor) -> Option<ControlRange> {
        let Anchor::WordControl { tag } = anchor else {
            return None;
        };
        if self.find(tag).is_some() { None } else { self.field_range(tag) }
    }

    /// Replaces a field's text. Only cell (0, 0) exists.
    fn write_field(&mut self, range: &ControlRange, changes: &[CellChange]) -> Result<(), BridgeError> {
        if let Some(bad) = changes.iter().find(|c| c.row != 0 || c.col != 0) {
            return Err(BridgeError::BadParams(format!("change at row {}, column {} is outside the linked field", bad.row, bad.col)));
        }
        let Some(change) = changes.last() else {
            return Ok(());
        };
        let new_text = change.value.as_ref().map(textcell::to_text).unwrap_or_default();
        let old_text = if range.control.showing_placeholder { String::new() } else { self.s().doc.control_text(range) };
        if old_text == new_text {
            return Ok(());
        }
        let saved = self.s().sel.clone();
        self.sm().sel = Selection { anchor: range.content_start(), focus: range.end.clone() };
        let result = if new_text.is_empty() { self.run("text.delete", &json!({})) } else { self.run("text.insert", &json!({ "text": new_text })) };
        self.sm().sel = saved;
        result.map(|_| ())
    }

    /// How many tables the document body holds, tagged or not (nested ones included).
    pub fn table_count(&self) -> usize {
        self.all_table_paths().len()
    }

    /// Tags the `nth` table (1-based, in document order) with `tag`, wherever the caret is. The
    /// user's selection is put back.
    pub fn tag_table(&mut self, nth: usize, tag: &str) -> Result<(), BridgeError> {
        let paths = self.all_table_paths();
        let path = nth.checked_sub(1).and_then(|i| paths.get(i)).ok_or_else(|| BridgeError::AnchorNotFound(format!("the document has {} table(s); there is no table {nth}", paths.len())))?;
        let at = {
            let table = self.table(path)?;
            let cell = table.rows.first().and_then(|r| r.cells.first()).ok_or_else(|| BridgeError::AnchorNotFound("the table has no cells".into()))?;
            let block = cell.blocks.iter().position(|b| b.as_para().is_some()).ok_or_else(|| BridgeError::AnchorNotFound("the table's first cell has no paragraph".into()))?;
            let mut p = path.0.clone();
            for part in [0usize, 0, block] {
                p.push(u32::try_from(part).map_err(|_| BridgeError::BadParams("cell too large".into()))?);
            }
            Pos::new(StoryRef::Body, Path(p), 0)
        };
        let saved = self.s().sel.clone();
        self.sm().sel = Selection { anchor: at.clone(), focus: at };
        let result = self.run("table.altText", &json!({ "text": tag })).map(|_| ());
        self.sm().sel = saved;
        result
    }

    fn all_table_paths(&self) -> Vec<Path> {
        let mut out = vec![];
        if let Some(body) = self.s().doc.story(StoryRef::Body) {
            collect_paths(body, &[], 0, &mut out);
        }
        out
    }

    fn run(&mut self, id: &str, params: &Json) -> Result<Json, BridgeError> {
        self.sm().run(id, params).map_err(map_err)
    }

    fn find(&self, tag: &str) -> Option<Found> {
        let body = self.s().doc.story(StoryRef::Body)?;
        find_in(body, &[], tag, 0)
    }

    fn resolve(&self, anchor: &Anchor) -> Result<Found, BridgeError> {
        let Anchor::WordControl { tag } = anchor else {
            return Err(BridgeError::Unsupported("WordCraft links are anchored by a table tag".into()));
        };
        let found = self.find(tag).ok_or_else(|| BridgeError::AnchorNotFound(format!("no table tagged `{tag}`")))?;
        if found.rows.saturating_mul(found.cols) > MAX_REGION_CELLS {
            return Err(BridgeError::BadParams(format!("the table tagged `{tag}` is larger than {MAX_REGION_CELLS} cells")));
        }
        Ok(found)
    }

    fn table(&self, path: &Path) -> Result<&Table, BridgeError> {
        self.s().doc.table(StoryRef::Body, path).ok_or_else(|| BridgeError::AnchorNotFound("the table was removed".into()))
    }
}

impl Default for WordEngine {
    fn default() -> Self {
        Self::new()
    }
}

struct Found {
    path: Path,
    rows: usize,
    cols: usize,
}

fn tag_of(t: &Table) -> Option<&str> {
    t.props.description.as_deref().map(str::trim).filter(|d| !d.is_empty())
}

fn dimensions(t: &Table) -> (usize, usize) {
    (t.rows.len(), t.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0))
}

fn child_prefix(prefix: &[u32], block: usize, row: usize, cell: usize) -> Option<Vec<u32>> {
    let mut p = prefix.to_vec();
    p.push(u32::try_from(block).ok()?);
    p.push(u32::try_from(row).ok()?);
    p.push(u32::try_from(cell).ok()?);
    Some(p)
}

fn find_in(blocks: &Blocks, prefix: &[u32], tag: &str, depth: usize) -> Option<Found> {
    if depth > MAX_TABLE_DEPTH {
        return None;
    }
    for (i, block) in blocks.iter().enumerate() {
        let Block::Table(t) = &**block else { continue };
        if tag_of(t) == Some(tag) {
            let mut path = prefix.to_vec();
            path.push(u32::try_from(i).ok()?);
            let (rows, cols) = dimensions(t);
            return Some(Found { path: Path(path), rows, cols });
        }
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                let inner = child_prefix(prefix, i, r, c)?;
                if let Some(found) = find_in(&cell.blocks, &inner, tag, depth + 1) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn collect_paths(blocks: &Blocks, prefix: &[u32], depth: usize, out: &mut Vec<Path>) {
    if depth > MAX_TABLE_DEPTH {
        return;
    }
    for (i, block) in blocks.iter().enumerate() {
        let Block::Table(t) = &**block else { continue };
        let Ok(at) = u32::try_from(i) else { continue };
        let mut path = prefix.to_vec();
        path.push(at);
        out.push(Path(path));
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                if let Some(inner) = child_prefix(prefix, i, r, c) {
                    collect_paths(&cell.blocks, &inner, depth + 1, out);
                }
            }
        }
    }
}

fn collect(blocks: &Blocks, prefix: &[u32], depth: usize, out: &mut Vec<TableRef>) {
    if depth > MAX_TABLE_DEPTH {
        return;
    }
    for (i, block) in blocks.iter().enumerate() {
        let Block::Table(t) = &**block else { continue };
        if let Some(tag) = tag_of(t)
            && tag.starts_with("alfadoc:")
        {
            let (rows, cols) = dimensions(t);
            out.push(TableRef { tag: tag.to_string(), rows, cols });
        }
        for (r, row) in t.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                if let Some(inner) = child_prefix(prefix, i, r, c) {
                    collect(&cell.blocks, &inner, depth + 1, out);
                }
            }
        }
    }
}

/// The text of a cell: its paragraphs joined by line breaks (nested tables are skipped).
fn cell_text(blocks: &Blocks) -> String {
    let lines: Vec<String> = blocks.iter().filter_map(|b| b.as_para().map(|p| p.plain_text())).collect();
    lines.join("\n")
}

impl<S: BorrowMut<Session>> Engine for WordEngine<S> {
    fn kind(&self) -> EngineKind {
        EngineKind::Word
    }

    fn execute(&mut self, command: &str, params: &Json) -> Result<Json, BridgeError> {
        self.run(command, params)
    }

    fn is_mutating(&self, command: &str) -> bool {
        self.s().registry.get(command).is_some_and(|spec| spec.mutates)
    }
}

impl<S: BorrowMut<Session>> RegionAccess for WordEngine<S> {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        if let Some(range) = self.field_for(anchor) {
            let mut region = Region::default();
            let text = if range.control.showing_placeholder { String::new() } else { self.s().doc.control_text(&range) };
            if !text.is_empty() {
                region.cells.insert((0, 0), textcell::from_text(&text));
            }
            return Ok(region);
        }
        let found = self.resolve(anchor)?;
        let table = self.table(&found.path)?;
        let mut region = Region::default();
        for (r, row) in table.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                let text = cell_text(&cell.blocks);
                if text.is_empty() {
                    continue;
                }
                let row_key = u32::try_from(r).map_err(|_| BridgeError::BadParams("table has too many rows".into()))?;
                let col_key = u16::try_from(c).map_err(|_| BridgeError::BadParams("table has too many columns".into()))?;
                region.cells.insert((row_key, col_key), textcell::from_text(&text));
            }
        }
        Ok(region)
    }

    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
        if let Some(range) = self.field_for(anchor) {
            return self.write_field(&range, changes);
        }
        let found = self.resolve(anchor)?;
        // Check everything first, so a bad change leaves the table untouched.
        {
            let table = self.table(&found.path)?;
            for ch in changes {
                let ok = usize::try_from(ch.row).ok().and_then(|r| table.rows.get(r)).is_some_and(|row| usize::from(ch.col) < row.cells.len());
                if !ok {
                    return Err(BridgeError::BadParams(format!("change at row {}, column {} is outside the linked table", ch.row, ch.col)));
                }
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        let saved: Selection = self.s().sel.clone();
        let mut result = Ok(());
        // One sync pass is one undo step: every write after the first joins the first one's step.
        let mut joined = false;
        for ch in changes {
            match self.write_cell(&found.path, ch, joined) {
                Ok(ran) => joined |= ran,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.sm().sel = saved;
        result
    }
}

impl<S: BorrowMut<Session>> WordEngine<S> {
    /// Replaces one cell's text with the text of `ch.value` (or empties it).
    /// Returns whether a command ran (a cell that already held the text needs none). With `join`
    /// the command becomes part of the previous undo step.
    fn write_cell(&mut self, table_path: &Path, ch: &CellChange, join: bool) -> Result<bool, BridgeError> {
        let (row, col) = (usize::try_from(ch.row).map_err(|_| BridgeError::BadParams("row too large".into()))?, usize::from(ch.col));
        let new_text = ch.value.as_ref().map(textcell::to_text).unwrap_or_default();
        let (first, last, old_text, end) = {
            let table = self.table(table_path)?;
            let cell = table.rows.get(row).and_then(|r| r.cells.get(col)).ok_or_else(|| BridgeError::BadParams("cell vanished".into()))?;
            let paras: Vec<(usize, usize)> = cell.blocks.iter().enumerate().filter_map(|(i, b)| b.as_para().map(|p| (i, p.text.len()))).collect();
            let (Some(&(first, _)), Some(&(last, end))) = (paras.first(), paras.last()) else {
                return Err(BridgeError::Engine("the cell has no paragraph to write into".into()));
            };
            (first, last, cell_text(&cell.blocks), end)
        };
        if old_text == new_text {
            return Ok(false);
        }
        let path_of = |block: usize| -> Result<Path, BridgeError> {
            let mut p = table_path.0.clone();
            p.push(u32::try_from(row).map_err(|_| BridgeError::BadParams("row too large".into()))?);
            p.push(u32::try_from(col).map_err(|_| BridgeError::BadParams("column too large".into()))?);
            p.push(u32::try_from(block).map_err(|_| BridgeError::BadParams("cell too large".into()))?);
            Ok(Path(p))
        };
        self.sm().sel = Selection { anchor: Pos::new(StoryRef::Body, path_of(first)?, 0), focus: Pos::new(StoryRef::Body, path_of(last)?, end) };
        if join {
            self.sm().join_next_undo();
        }
        if new_text.is_empty() {
            self.run("text.delete", &json!({}))?;
        } else {
            self.run("text.insert", &json!({ "text": new_text }))?;
        }
        Ok(true)
    }
}

fn map_err(e: CmdError) -> BridgeError {
    match e {
        CmdError::Unknown(id) => BridgeError::UnknownCommand(id),
        CmdError::Params(m) => BridgeError::BadParams(m),
        other => BridgeError::Engine(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::Value;

    fn anchor(tag: &str) -> Anchor {
        Anchor::WordControl { tag: tag.into() }
    }

    /// A document holding a 3 × 2 table tagged `alfadoc:link=L1`, with the caret left inside it.
    fn doc_with_table() -> (WordEngine, Anchor) {
        let mut w = WordEngine::new();
        w.execute("insert.table", &json!({ "rows": 3, "cols": 2 })).unwrap();
        w.tag_selected_table("alfadoc:link=L1").unwrap();
        (w, anchor("alfadoc:link=L1"))
    }

    #[test]
    fn lists_tagged_tables() {
        let (w, _) = doc_with_table();
        assert_eq!(w.tables(), vec![TableRef { tag: "alfadoc:link=L1".into(), rows: 3, cols: 2 }]);
    }

    #[test]
    fn writes_cells_and_reads_them_back_with_their_types() {
        let (mut w, a) = doc_with_table();
        let changes = [
            CellChange { row: 0, col: 0, value: Some(Value::Text("Item".into())) },
            CellChange { row: 1, col: 0, value: Some(Value::Text("Pens".into())) },
            CellChange { row: 1, col: 1, value: Some(Value::Int(12)) },
            CellChange { row: 2, col: 1, value: Some(Value::Float(3.5)) },
        ];
        w.write_changes(&a, &changes).unwrap();
        let r = w.read_region(&a).unwrap();
        assert_eq!(r.cells.get(&(0, 0)), Some(&Value::Text("Item".into())));
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(12)));
        assert_eq!(r.cells.get(&(2, 1)), Some(&Value::Float(3.5)));
        assert_eq!(r.cells.len(), 4);
    }


    #[test]
    fn a_sync_pass_is_one_undo_step() {
        let (mut w, a) = doc_with_table();
        let changes = [
            CellChange { row: 0, col: 0, value: Some(Value::Text("Item".into())) },
            CellChange { row: 1, col: 0, value: Some(Value::Text("Pens".into())) },
            CellChange { row: 1, col: 1, value: Some(Value::Int(12)) },
        ];
        w.write_changes(&a, &changes).unwrap();
        assert_eq!(w.read_region(&a).unwrap().cells.len(), 3);
        w.execute("edit.undo", &json!({})).unwrap();
        assert!(w.read_region(&a).unwrap().cells.is_empty(), "one undo takes back the whole pass");
    }

    #[test]
    fn a_selected_word_becomes_a_live_inline_field() {
        let mut w = WordEngine::new();
        w.execute("text.insert", &json!({ "text": "Total: 12 EUR" })).unwrap();
        let para = Path(vec![0]);
        w.session_mut().sel = Selection { anchor: Pos::new(StoryRef::Body, para.clone(), 7), focus: Pos::new(StoryRef::Body, para, 9) };
        w.insert_field("alfadoc:link=F1").unwrap();
        let a = anchor("alfadoc:link=F1");
        assert_eq!(w.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Int(12)));
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(15)) }]).unwrap();
        assert_eq!(w.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Int(15)));
        let text = w.session().doc.plain_text(StoryRef::Body);
        assert!(text.contains("Total: ") && text.contains("15") && text.contains(" EUR"), "{text}");
        assert!(!text.contains("12"), "{text}");
        w.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(w.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Int(12)));
        assert!(w.write_changes(&a, &[CellChange { row: 1, col: 0, value: None }]).is_err());
    }

    #[test]
    fn variant_blocks_not_for_this_audience_can_be_removed() {
        let mut w = WordEngine::new();
        w.execute("text.insert", &json!({ "text": "Keep. Drop. End." })).unwrap();
        let para = Path(vec![0]);
        w.session_mut().sel = Selection { anchor: Pos::new(StoryRef::Body, para.clone(), 6), focus: Pos::new(StoryRef::Body, para, 12) };
        let internal: DimVector = [("audience".to_string(), "internal".to_string())].into_iter().collect();
        let client: DimVector = [("audience".to_string(), "client".to_string())].into_iter().collect();
        w.mark_variant(&internal).unwrap();
        // Shown for its own audience: nothing is removed.
        assert_eq!(w.keep_variants(&internal).unwrap(), 0);
        assert!(w.session().doc.plain_text(StoryRef::Body).contains("Drop"));
        // For another audience the block goes, text and all; the rest stays.
        assert_eq!(w.keep_variants(&client).unwrap(), 1);
        let text = w.session().doc.plain_text(StoryRef::Body);
        assert!(!text.contains("Drop") && text.contains("Keep.") && text.contains("End."), "{text}");
        w.execute("edit.undo", &json!({})).unwrap();
        assert!(w.session().doc.plain_text(StoryRef::Body).contains("Drop"));
    }

    #[test]
    fn every_table_can_be_offered_in_a_picker() {
        let (w, _) = doc_with_table();
        let choices = w.table_choices();
        assert_eq!(choices.len(), 1);
        assert_eq!((choices[0].nth, choices[0].rows, choices[0].cols), (1, 3, 2));
        assert!(choices[0].label().starts_with("Table 1 · 3×2"));
    }

    #[test]
    fn rewriting_a_cell_replaces_its_text() {
        let (mut w, a) = doc_with_table();
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Text("first".into())) }]).unwrap();
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Text("second".into())) }]).unwrap();
        assert_eq!(w.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Text("second".into())));
    }

    #[test]
    fn clearing_a_cell_removes_it_from_the_region() {
        let (mut w, a) = doc_with_table();
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(1)) }]).unwrap();
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: None }]).unwrap();
        assert!(w.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn text_that_looks_like_a_number_stays_text() {
        let (mut w, a) = doc_with_table();
        w.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Text("007".into())) }]).unwrap();
        assert_eq!(w.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Text("007".into())));
    }

    #[test]
    fn the_users_selection_is_put_back() {
        let (mut w, a) = doc_with_table();
        let before = w.session().sel.clone();
        w.write_changes(&a, &[CellChange { row: 2, col: 1, value: Some(Value::Int(5)) }]).unwrap();
        assert_eq!(w.session().sel, before);
    }

    #[test]
    fn out_of_range_changes_are_refused_and_change_nothing() {
        let (mut w, a) = doc_with_table();
        let bad = [CellChange { row: 0, col: 0, value: Some(Value::Int(1)) }, CellChange { row: 3, col: 0, value: Some(Value::Int(2)) }];
        assert!(matches!(w.write_changes(&a, &bad), Err(BridgeError::BadParams(_))));
        assert!(w.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn unknown_anchors_are_errors_not_panics() {
        let (w, _) = doc_with_table();
        assert!(matches!(w.read_region(&anchor("alfadoc:link=nope")), Err(BridgeError::AnchorNotFound(_))));
        assert!(matches!(w.read_region(&Anchor::GridName { name: "n".into() }), Err(BridgeError::Unsupported(_))));
    }

    /// The upstream surface this adapter touches. A rename in WordCraft fails here, at update time.
    #[test]
    fn every_command_the_adapter_uses_still_exists() {
        let w = WordEngine::new();
        for needed in ["insert.table", "table.altText", "text.insert", "text.delete"] {
            assert!(w.session().registry.get(needed).is_some(), "WordCraft command {needed} vanished upstream");
        }
    }

    #[test]
    fn queries_are_not_mutating_and_edits_are() {
        let w = WordEngine::new();
        assert!(w.is_mutating("text.insert"));
        assert!(!w.is_mutating("no.such.command"));
    }
}
