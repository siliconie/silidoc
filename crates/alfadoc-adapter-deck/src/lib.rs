//! DeckCraft behind Alfadoc's traits: a **table on a slide** is a linkable region.
//!
//! A link points at a table by `Anchor::DeckShape { slide_id, shape_name }`: the slide's id and the
//! table shape's *name* (Selection Pane / `shape.rename`). Names and ids survive a `.pptx` round
//! trip, so a link keeps working after the deck went through PowerPoint.
//!
//! Slide tables hold text. Values travel as text with the stable mapping in
//! [`alfadoc_bridge::textcell`] (a number comes back as a number, `007` stays text).
//!
//! Writes go through DeckCraft's own `text.set` command, one per changed cell, so undo, the
//! journal and the revision counter all keep working. `text.set` acts on the current slide, so the
//! adapter goes to the table's slide and returns to the slide the user was on.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::borrow::BorrowMut;
use alfadoc_bridge::{BridgeError, CellChange, Engine, EngineKind, Region, RegionAccess, TableChoice, preview_of, textcell};
use alfadoc_model::Anchor;
use deckcraft_engine::model::{Shape, ShapeId, ShapeKind, SlideId};
use deckcraft_engine::{EngineError, Session, find_command};
use serde_json::{Value as Json, json};

/// The most cells one link may cover (DeckCraft itself caps tables at 1000 × 1000).
pub const MAX_REGION_CELLS: usize = 1_000_000;
/// Groups nest at most this deep when looking a shape up by name.
const MAX_GROUP_DEPTH: usize = 64;

/// A table on a slide, as listed for the "link this table" picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub slide_id: String,
    pub slide_index: usize,
    pub shape_name: String,
    pub rows: usize,
    pub cols: usize,
}

/// `S` is the session it works on: an owned [`Session`] (the default), or `&mut Session` to work on
/// the one a live editor window holds ([`DeckEngine::borrowed`]).
pub struct DeckEngine<S = Session> {
    session: S,
}

impl Default for DeckEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DeckEngine {
    /// A session with one new presentation open.
    pub fn new() -> Self {
        DeckEngine { session: Session::with_new() }
    }

    pub fn from_session(session: Session) -> Self {
        DeckEngine { session }
    }
}

impl<'a> DeckEngine<&'a mut Session> {
    /// Works on a session somebody else owns, for as long as the borrow lasts.
    pub fn borrowed(session: &'a mut Session) -> Self {
        DeckEngine { session }
    }
}

impl<S: BorrowMut<Session>> DeckEngine<S> {
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

    /// Every table in the presentation that has a name, in slide order (top-level shapes and
    /// shapes inside groups).
    pub fn tables(&self) -> Result<Vec<TableRef>, BridgeError> {
        let doc = self.s().doc().map_err(map_err)?;
        let mut out = vec![];
        for (slide_index, slide) in doc.doc.slides.iter().enumerate() {
            collect_tables(&slide.shapes, &slide.id.to_string(), slide_index, 0, &mut out);
        }
        Ok(out)
    }

    /// Gives the table shape `shape` on the slide at `slide_index` the name a link will use.
    /// (`shape.rename` acts on the current slide, so this goes there and comes back.)
    pub fn name_table(&mut self, slide_index: usize, shape: u32, name: &str) -> Result<(), BridgeError> {
        let back = self.current_slide()?;
        self.go(slide_index)?;
        let result = self.run("shape.rename", &json!({ "id": shape, "name": name })).map(|_| ());
        self.go(back)?;
        result
    }

    /// Every table of the presentation (named or not), slide by slide, for a picker.
    pub fn table_choices(&self) -> Result<Vec<TableChoice>, BridgeError> {
        fn walk(shapes: &[Shape], slide_index: usize, depth: usize, out: &mut Vec<TableChoice>) {
            if depth > MAX_GROUP_DEPTH {
                return;
            }
            for s in shapes {
                if let ShapeKind::Table(t) = &s.kind {
                    let first: Vec<String> = t.rows.first().map(|r| r.cells.iter().map(|c| c.text.text()).collect()).unwrap_or_default();
                    out.push(TableChoice { nth: out.len() + 1, rows: t.n_rows(), cols: t.n_cols(), preview: preview_of(first.into_iter()), place: format!("Slide {}", slide_index + 1) });
                }
                walk(s.children(), slide_index, depth + 1, out);
            }
        }
        let doc = self.s().doc().map_err(map_err)?;
        let mut out = vec![];
        for (i, slide) in doc.doc.slides.iter().enumerate() {
            walk(&slide.shapes, i, 0, &mut out);
        }
        Ok(out)
    }

    /// How many tables the presentation holds, named or not.
    pub fn table_count(&self) -> Result<usize, BridgeError> {
        Ok(self.all_tables()?.len())
    }

    /// Names the `nth` table (1-based, slide by slide) and returns the id of its slide.
    pub fn name_nth_table(&mut self, nth: usize, name: &str) -> Result<String, BridgeError> {
        let all = self.all_tables()?;
        let (slide_index, shape, slide_id) = nth
            .checked_sub(1)
            .and_then(|i| all.get(i))
            .cloned()
            .ok_or_else(|| BridgeError::AnchorNotFound(format!("the presentation has {} table(s); there is no table {nth}", all.len())))?;
        self.name_table(slide_index, shape, name)?;
        Ok(slide_id)
    }

    /// (slide index, shape id, slide id) of every table.
    fn all_tables(&self) -> Result<Vec<(usize, u32, String)>, BridgeError> {
        fn walk(shapes: &[Shape], slide_index: usize, slide_id: &str, depth: usize, out: &mut Vec<(usize, u32, String)>) {
            if depth > MAX_GROUP_DEPTH {
                return;
            }
            for s in shapes {
                if matches!(s.kind, ShapeKind::Table(_)) {
                    out.push((slide_index, s.id.0, slide_id.to_string()));
                }
                walk(s.children(), slide_index, slide_id, depth + 1, out);
            }
        }
        let doc = self.s().doc().map_err(map_err)?;
        let mut out = vec![];
        for (i, slide) in doc.doc.slides.iter().enumerate() {
            walk(&slide.shapes, i, &slide.id.to_string(), 0, &mut out);
        }
        Ok(out)
    }

    fn run(&mut self, id: &str, params: &Json) -> Result<Json, BridgeError> {
        self.sm().execute(id, params).map_err(map_err)
    }

    fn current_slide(&self) -> Result<usize, BridgeError> {
        Ok(self.s().doc().map_err(map_err)?.selection.slide)
    }

    fn go(&mut self, index: usize) -> Result<(), BridgeError> {
        if self.current_slide()? != index {
            self.run("slide.go", &json!({ "index": index }))?;
        }
        Ok(())
    }

    fn resolve(&self, anchor: &Anchor) -> Result<Found, BridgeError> {
        let Anchor::DeckShape { slide_id, shape_name } = anchor else {
            return Err(BridgeError::Unsupported("DeckCraft links are anchored by slide id and shape name".into()));
        };
        let id: u32 = slide_id.parse().map_err(|_| BridgeError::AnchorNotFound(format!("`{slide_id}` is not a slide id")))?;
        let doc = self.s().doc().map_err(map_err)?;
        // The slide id is only a hint: a presentation that went through a file may number its
        // slides differently. The shape name is what identifies the table, so when the hinted
        // slide has no such shape the other slides are searched.
        let hinted = doc.doc.slide_index(SlideId(id)).filter(|i| doc.doc.slides.get(*i).is_some_and(|s| find_named(&s.shapes, shape_name, 0).is_some()));
        let slide_index = hinted
            .or_else(|| doc.doc.slides.iter().position(|s| find_named(&s.shapes, shape_name, 0).is_some()))
            .ok_or_else(|| BridgeError::AnchorNotFound(format!("no slide has a shape named `{shape_name}` (looked first on slide {id})")))?;
        let slide = doc.doc.slides.get(slide_index).ok_or_else(|| BridgeError::AnchorNotFound(format!("no slide {id}")))?;
        let shape = find_named(&slide.shapes, shape_name, 0).ok_or_else(|| BridgeError::AnchorNotFound(format!("no shape named `{shape_name}`")))?;
        let ShapeKind::Table(table) = &shape.kind else {
            return Err(BridgeError::AnchorNotFound(format!("`{shape_name}` is not a table")));
        };
        if table.n_rows().saturating_mul(table.n_cols()) > MAX_REGION_CELLS {
            return Err(BridgeError::BadParams(format!("`{shape_name}` is larger than {MAX_REGION_CELLS} cells")));
        }
        Ok(Found { slide_index, shape: shape.id, rows: table.n_rows(), cols: table.n_cols() })
    }
}

struct Found {
    slide_index: usize,
    shape: ShapeId,
    rows: usize,
    cols: usize,
}

fn find_named<'a>(shapes: &'a [Shape], name: &str, depth: usize) -> Option<&'a Shape> {
    if depth > MAX_GROUP_DEPTH {
        return None;
    }
    for s in shapes {
        if s.name == name {
            return Some(s);
        }
        if let Some(found) = find_named(s.children(), name, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn collect_tables(shapes: &[Shape], slide_id: &str, slide_index: usize, depth: usize, out: &mut Vec<TableRef>) {
    if depth > MAX_GROUP_DEPTH {
        return;
    }
    for s in shapes {
        if let ShapeKind::Table(t) = &s.kind
            && !s.name.is_empty()
        {
            out.push(TableRef { slide_id: slide_id.to_string(), slide_index, shape_name: s.name.clone(), rows: t.n_rows(), cols: t.n_cols() });
        }
        collect_tables(s.children(), slide_id, slide_index, depth + 1, out);
    }
}

impl<S: BorrowMut<Session>> Engine for DeckEngine<S> {
    fn kind(&self) -> EngineKind {
        EngineKind::Deck
    }

    fn execute(&mut self, command: &str, params: &Json) -> Result<Json, BridgeError> {
        self.run(command, params)
    }

    /// DeckCraft marks queries by leaving them out of its journal and undo history.
    fn is_mutating(&self, command: &str) -> bool {
        find_command(command).is_some_and(|spec| spec.journal || spec.undoable)
    }

    fn revision(&self) -> Option<u64> {
        self.s().active().map(|d| d.revision)
    }
}

impl<S: BorrowMut<Session>> RegionAccess for DeckEngine<S> {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        let found = self.resolve(anchor)?;
        let doc = self.s().doc().map_err(map_err)?;
        let slide = doc.doc.slides.get(found.slide_index).ok_or_else(|| BridgeError::AnchorNotFound("slide vanished".into()))?;
        let shape = deckcraft_engine::model::find_shape(&slide.shapes, found.shape).ok_or_else(|| BridgeError::AnchorNotFound("table vanished".into()))?;
        let ShapeKind::Table(table) = &shape.kind else {
            return Err(BridgeError::AnchorNotFound("table vanished".into()));
        };
        let mut region = Region::default();
        for (r, row) in table.rows.iter().enumerate() {
            for (c, cell) in row.cells.iter().enumerate() {
                // A cell covered by a merge shows another cell's text.
                if cell.h_merge || cell.v_merge {
                    continue;
                }
                let text = cell.text.text();
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
        let found = self.resolve(anchor)?;
        // Check everything first, so a bad change leaves the table untouched.
        for ch in changes {
            if usize::try_from(ch.row).map_or(true, |r| r >= found.rows) || usize::from(ch.col) >= found.cols {
                return Err(BridgeError::BadParams(format!("change at row {}, column {} is outside the linked table", ch.row, ch.col)));
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        let back = self.current_slide()?;
        let steps_before = self.s().doc().map_err(map_err)?.history.undo.len();
        self.go(found.slide_index)?;
        let mut result = Ok(());
        for ch in changes {
            let text = ch.value.as_ref().map(textcell::to_text).unwrap_or_default();
            let params = json!({ "id": found.shape.0, "text": text, "cell": [ch.row, ch.col] });
            if let Err(e) = self.run("text.set", &params) {
                result = Err(e);
                break;
            }
        }
        // One sync pass is one undo step: keep the state before the first write and drop the
        // steps the later writes added. (At the history limit the count cannot show how many
        // were added, so the steps are left as they are.)
        if let Ok(st) = self.sm().doc_mut() {
            let keep = steps_before + 1;
            if st.history.undo.len() > keep {
                st.history.undo.truncate(keep);
            }
        }
        // Back to the slide the user was looking at, whether or not every write worked.
        let returned = self.go(back);
        result.and(returned)
    }
}

fn map_err(e: EngineError) -> BridgeError {
    match e {
        EngineError::UnknownCommand(id) => BridgeError::UnknownCommand(id),
        other => BridgeError::Engine(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::Value;

    fn anchor(slide: &str, name: &str) -> Anchor {
        Anchor::DeckShape { slide_id: slide.into(), shape_name: name.into() }
    }

    /// A deck whose first slide has a 3 × 2 table named "sales".
    fn deck_with_table() -> (DeckEngine, Anchor) {
        let mut d = DeckEngine::new();
        let made = d.execute("insert.table", &json!({ "rows": 3, "cols": 2 })).unwrap();
        let id = made["id"].as_u64().unwrap() as u32;
        d.name_table(0, id, "sales").unwrap();
        let slide_id = d.session().doc().unwrap().doc.slides[0].id.to_string();
        (d, anchor(&slide_id, "sales"))
    }

    #[test]
    fn lists_named_tables() {
        let (d, _) = deck_with_table();
        let t = d.tables().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].shape_name.as_str(), t[0].rows, t[0].cols, t[0].slide_index), ("sales", 3, 2, 0));
    }

    #[test]
    fn writes_cells_and_reads_them_back_with_their_types() {
        let (mut d, a) = deck_with_table();
        let changes = [
            CellChange { row: 0, col: 0, value: Some(Value::Text("Item".into())) },
            CellChange { row: 1, col: 0, value: Some(Value::Text("Pens".into())) },
            CellChange { row: 1, col: 1, value: Some(Value::Int(12)) },
            CellChange { row: 2, col: 1, value: Some(Value::Float(3.5)) },
        ];
        d.write_changes(&a, &changes).unwrap();
        let r = d.read_region(&a).unwrap();
        assert_eq!(r.cells.get(&(0, 0)), Some(&Value::Text("Item".into())));
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(12)));
        assert_eq!(r.cells.get(&(2, 1)), Some(&Value::Float(3.5)));
        assert_eq!(r.cells.len(), 4);
    }


    #[test]
    fn every_table_can_be_offered_in_a_picker() {
        let (d, _) = deck_with_table();
        let choices = d.table_choices().unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!((choices[0].nth, choices[0].rows, choices[0].cols), (1, 3, 2));
        assert!(choices[0].label().starts_with("Slide 1 · Table 1 · 3×2"));
    }

    #[test]
    fn a_sync_pass_is_one_undo_step() {
        let (mut d, a) = deck_with_table();
        let changes = [
            CellChange { row: 0, col: 0, value: Some(Value::Text("Item".into())) },
            CellChange { row: 1, col: 0, value: Some(Value::Text("Pens".into())) },
            CellChange { row: 1, col: 1, value: Some(Value::Int(12)) },
        ];
        d.write_changes(&a, &changes).unwrap();
        assert_eq!(d.read_region(&a).unwrap().cells.len(), 3);
        d.execute("edit.undo", &json!({})).unwrap();
        assert!(d.read_region(&a).unwrap().cells.is_empty(), "one undo takes back the whole pass");
    }

    #[test]
    fn clearing_a_cell_removes_it_from_the_region() {
        let (mut d, a) = deck_with_table();
        d.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(1)) }]).unwrap();
        d.write_changes(&a, &[CellChange { row: 0, col: 0, value: None }]).unwrap();
        assert!(d.read_region(&a).unwrap().cells.is_empty());
    }

    #[test]
    fn text_that_looks_like_a_number_stays_text() {
        let (mut d, a) = deck_with_table();
        d.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Text("007".into())) }]).unwrap();
        assert_eq!(d.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Text("007".into())));
    }

    #[test]
    fn writing_returns_to_the_slide_the_user_was_on() {
        let (mut d, a) = deck_with_table();
        d.execute("slide.new", &json!({})).unwrap();
        d.execute("slide.go", &json!({ "index": 1 })).unwrap();
        d.write_changes(&a, &[CellChange { row: 0, col: 0, value: Some(Value::Int(5)) }]).unwrap();
        assert_eq!(d.session().doc().unwrap().selection.slide, 1);
        assert_eq!(d.read_region(&a).unwrap().cells.get(&(0, 0)), Some(&Value::Int(5)));
    }

    #[test]
    fn out_of_range_changes_are_refused_and_change_nothing() {
        let (mut d, a) = deck_with_table();
        let before = d.revision();
        let bad = [CellChange { row: 0, col: 0, value: Some(Value::Int(1)) }, CellChange { row: 3, col: 0, value: Some(Value::Int(2)) }];
        assert!(matches!(d.write_changes(&a, &bad), Err(BridgeError::BadParams(_))));
        assert_eq!(d.revision(), before);
    }

    #[test]
    fn a_wrong_slide_id_still_finds_the_named_table() {
        let (d, _) = deck_with_table();
        assert_eq!(d.read_region(&anchor("424242", "sales")).unwrap().cells.len(), 0);
    }

    #[test]
    fn unknown_anchors_are_errors_not_panics() {
        let (d, _) = deck_with_table();
        assert!(matches!(d.read_region(&anchor("999999", "missing")), Err(BridgeError::AnchorNotFound(_))));
        assert!(matches!(d.read_region(&anchor("x", "sales")), Err(BridgeError::AnchorNotFound(_))));
        assert!(matches!(d.read_region(&Anchor::GridName { name: "n".into() }), Err(BridgeError::Unsupported(_))));
    }

    /// The upstream surface this adapter touches. A rename in DeckCraft fails here, at update time.
    #[test]
    fn every_command_the_adapter_uses_still_exists() {
        let d = DeckEngine::new();
        let ids: Vec<&str> = d.session().commands().iter().map(|c| c.id).collect();
        for needed in ["insert.table", "shape.rename", "text.set", "slide.go", "slide.new", "file.new"] {
            assert!(ids.contains(&needed), "DeckCraft command {needed} vanished upstream");
        }
        let doc = d.session().doc().unwrap();
        let _ = (doc.revision, doc.selection.slide, doc.doc.slides.len());
    }

    #[test]
    fn queries_are_not_mutating_and_edits_are() {
        let d = DeckEngine::new();
        assert!(!d.is_mutating("text.get"));
        assert!(d.is_mutating("text.set"));
    }
}
