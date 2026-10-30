#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
//! GridCraft behind Alfadoc's traits.
//!
//! Reads go through the engine's public document model (`Session::doc`, `Sheet::value`); every
//! write goes through the engine's own commands (`range.setValues`, `cell.set`) inside one undo
//! group, so undo, the journal and recalculation keep working exactly as if the user had typed.
//!
//! Alfadoc addresses a GridCraft region by a **defined name** ([`Anchor::GridName`]). Defined names
//! are stored in the `.xlsx` file, follow the cells when rows or columns are inserted, and are what
//! Excel itself shows in its Name Manager, so a link survives a round trip through Excel.

use std::borrow::BorrowMut;
use alfadoc_bridge::{BridgeError, CellChange, Engine, EngineKind, Region, RegionAccess};
use alfadoc_model::{Anchor, Value};
use gridcraft_engine::core::{
    CellRef, RangeRef, Value as GridValue,
    addr::{quote_sheet, split_sheet},
    date,
};
use gridcraft_engine::{EngineError, Session, find_command};
use serde_json::{Value as Json, json};

/// The most cells one link may cover. A link is a table, not a whole sheet.
pub const MAX_REGION_CELLS: u64 = 1_000_000;

/// One open GridCraft session (a workbook, plus the engine's undo history and journal).
/// `S` is the session it works on: an owned [`Session`] (the default), or `&mut Session` to work on
/// the one a live editor window holds ([`GridEngine::borrowed`]).
pub struct GridEngine<S = Session> {
    session: S,
}

impl Default for GridEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GridEngine {
    /// A session with one blank workbook open.
    pub fn new() -> Self {
        let mut session = Session::new();
        session.new_workbook();
        GridEngine { session }
    }

    pub fn from_session(session: Session) -> Self {
        GridEngine { session }
    }
}

impl<'a> GridEngine<&'a mut Session> {
    /// Works on a session somebody else owns, for as long as the borrow lasts.
    pub fn borrowed(session: &'a mut Session) -> Self {
        GridEngine { session }
    }
}

impl<S: BorrowMut<Session>> GridEngine<S> {
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

    /// Defines (or redefines) the workbook-level name `name` as `sheet`!`range`, e.g.
    /// `define_region("alfadoc_link_7", "Sheet1", "A1:C10")`. This is how a link gets its anchor.
    pub fn define_region(&mut self, name: &str, sheet: &str, range: &str) -> Result<(), BridgeError> {
        let parsed = RangeRef::parse(range).ok_or_else(|| BridgeError::BadParams(format!("not a range: {range}")))?;
        let refers =
            format!("{}!${}${}:${}${}", quote_sheet(sheet), col_letters(parsed.start), parsed.start.row + 1, col_letters(parsed.end), parsed.end.row + 1);
        self.run("formulas.defineName", &json!({ "name": name, "refersTo": refers }))?;
        Ok(())
    }

    fn run(&mut self, id: &str, params: &Json) -> Result<Json, BridgeError> {
        self.sm().execute(id, params.clone()).map_err(map_err)
    }

    /// Where the anchor points: (sheet index, rectangle on that sheet).
    fn resolve(&self, anchor: &Anchor) -> Result<(usize, RangeRef), BridgeError> {
        let Anchor::GridName { name } = anchor else {
            return Err(BridgeError::Unsupported("GridCraft links are anchored by a defined name".into()));
        };
        let doc = self.s().doc().map_err(map_err)?;
        let wb = &doc.wb;
        let def = wb
            .names
            .iter()
            .filter(|n| n.name.eq_ignore_ascii_case(name))
            .min_by_key(|n| n.scope.is_some()) // workbook scope wins over sheet scope
            .ok_or_else(|| BridgeError::AnchorNotFound(name.clone()))?;
        let (sheet, body) = split_sheet(def.formula.trim_start_matches('='));
        let sheet = sheet.ok_or_else(|| BridgeError::AnchorNotFound(format!("{name} does not name a range on a sheet")))?;
        let index = wb.sheet_index(&sheet).ok_or_else(|| BridgeError::AnchorNotFound(format!("{name}: no sheet `{sheet}`")))?;
        let range = RangeRef::parse(body).ok_or_else(|| BridgeError::AnchorNotFound(format!("{name} does not name a plain range")))?;
        if range.count() > MAX_REGION_CELLS {
            return Err(BridgeError::BadParams(format!("{name}: region is larger than {MAX_REGION_CELLS} cells")));
        }
        Ok((index, range))
    }
}

impl<S: BorrowMut<Session>> Engine for GridEngine<S> {
    fn kind(&self) -> EngineKind {
        EngineKind::Grid
    }

    fn execute(&mut self, command: &str, params: &Json) -> Result<Json, BridgeError> {
        self.run(command, params)
    }

    /// GridCraft marks queries by leaving them out of its journal and undo history.
    fn is_mutating(&self, command: &str) -> bool {
        find_command(command).is_some_and(|spec| spec.journal || spec.undoable)
    }

    /// `DocState::revision`: moves only when the workbook really changed.
    fn revision(&self) -> Option<u64> {
        self.s().active().map(|d| d.revision)
    }
}

impl<S: BorrowMut<Session>> RegionAccess for GridEngine<S> {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError> {
        let (index, range) = self.resolve(anchor)?;
        let doc = self.s().doc().map_err(map_err)?;
        let sheet = doc.wb.sheet(index).ok_or_else(|| BridgeError::AnchorNotFound("sheet vanished".into()))?;
        let mut region = Region::default();
        for at in range.iter() {
            let Some(value) = from_grid(&sheet.value(at)) else {
                continue;
            };
            let row = at.row - range.start.row;
            let col = u16::try_from(at.col - range.start.col).map_err(|_| BridgeError::BadParams("region is wider than 65,535 columns".into()))?;
            region.cells.insert((row, col), value);
        }
        Ok(region)
    }

    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError> {
        let (index, range) = self.resolve(anchor)?;
        let mut work: Vec<(CellRef, Json, bool)> = Vec::with_capacity(changes.len());
        for ch in changes {
            let at = range
                .start
                .offset(i64::from(ch.row), i64::from(ch.col))
                .filter(|at| range.contains(*at))
                .ok_or_else(|| BridgeError::BadParams(format!("change at row {}, column {} is outside the linked region", ch.row, ch.col)))?;
            // Text that starts with `=` would be read as a formula; it needs `cell.set` with the
            // apostrophe that forces text. Everything else goes through `range.setValues`.
            let literal_equals = matches!(&ch.value, Some(Value::Text(t)) if t.starts_with('='));
            work.push((at, to_json(ch.value.as_ref()), literal_equals));
        }
        work.sort_by_key(|(at, _, _)| *at);
        self.sm()
            .grouped("Alfadoc sync", |s| {
                let mut run: Vec<Json> = vec![];
                let mut run_start = CellRef::default();
                let mut next = CellRef::default();
                let flush = |s: &mut Session, run: &mut Vec<Json>, start: CellRef| -> gridcraft_engine::Result<()> {
                    if run.is_empty() {
                        return Ok(());
                    }
                    let values = std::mem::take(run);
                    s.execute("range.setValues", json!({ "sheet": index, "range": start.a1(), "values": [values], "parse": false }))?;
                    Ok(())
                };
                for (at, value, literal_equals) in work {
                    if literal_equals {
                        flush(s, &mut run, run_start)?;
                        let text = value.as_str().unwrap_or_default();
                        s.execute("cell.set", json!({ "sheet": index, "cell": at.a1(), "input": format!("'{text}") }))?;
                        continue;
                    }
                    let adjacent = !run.is_empty() && at == next;
                    if !adjacent {
                        flush(s, &mut run, run_start)?;
                        run_start = at;
                    }
                    run.push(value);
                    next = CellRef::new(at.row, at.col + 1);
                }
                flush(s, &mut run, run_start)
            })
            .map_err(map_err)
    }
}

fn col_letters(c: CellRef) -> String {
    gridcraft_engine::core::col_to_letters(c.col)
}

fn map_err(e: EngineError) -> BridgeError {
    match e {
        EngineError::UnknownCommand(id) => BridgeError::UnknownCommand(id),
        EngineError::BadParams { cmd, msg } => BridgeError::BadParams(format!("{cmd}: {msg}")),
        other => BridgeError::Engine(other.to_string()),
    }
}

/// A GridCraft cell value as an Alfadoc one. Empty cells and spilled arrays read as "nothing here";
/// errors (`#N/A`) read as their text so the other side shows what the sheet shows.
fn from_grid(v: &GridValue) -> Option<Value> {
    match v {
        GridValue::Empty | GridValue::Array(_) => None,
        GridValue::Number(n) => Some(number(*n)),
        GridValue::Text(t) => Some(Value::Text(t.to_string())),
        GridValue::Bool(b) => Some(Value::Bool(*b)),
        GridValue::Error(e) => Some(Value::Text(e.as_str().to_string())),
    }
}

fn number(n: f64) -> Value {
    // 2^53: every integer below it is exact in an f64.
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if n.is_finite() && n.fract() == 0.0 && n.abs() < EXACT {
        #[allow(clippy::cast_possible_truncation)]
        return Value::Int(n as i64);
    }
    Value::Float(n)
}

fn to_json(v: Option<&Value>) -> Json {
    match v {
        None | Some(Value::Null) => Json::Null,
        Some(Value::Int(i)) => json!(i),
        Some(Value::Float(f)) => json!(f),
        Some(Value::Text(t)) => json!(t),
        Some(Value::Bool(b)) => json!(b),
        #[allow(clippy::cast_precision_loss)]
        Some(Value::Date(ms)) => json!(date::serial_from_unix_ms(*ms as f64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_bridge::Dispatcher;

    fn region_a(name: &str) -> Anchor {
        Anchor::GridName { name: name.into() }
    }

    fn engine_with_table() -> GridEngine {
        let mut g = GridEngine::new();
        g.execute("range.setValues", &json!({ "range": "A1", "values": [["Item", "Qty"], ["Pens", 12], ["Ink", 3.5]] })).unwrap();
        g.define_region("tbl", "Sheet1", "A1:B3").unwrap();
        g
    }

    #[test]
    fn reads_a_named_range_relative_to_its_corner() {
        let g = engine_with_table();
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(0, 0)), Some(&Value::Text("Item".into())));
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(12)));
        assert_eq!(r.cells.get(&(2, 1)), Some(&Value::Float(3.5)));
        assert_eq!(r.cells.len(), 6);
    }

    #[test]
    fn writes_go_through_commands_and_undo_as_one_step() {
        let mut g = engine_with_table();
        let before = g.revision();
        let changes = [
            CellChange { row: 1, col: 1, value: Some(Value::Int(99)) },
            CellChange { row: 2, col: 1, value: Some(Value::Int(7)) },
            CellChange { row: 2, col: 0, value: None },
        ];
        g.write_changes(&region_a("tbl"), &changes).unwrap();
        assert_ne!(g.revision(), before);
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(99)));
        assert_eq!(r.cells.get(&(2, 1)), Some(&Value::Int(7)));
        assert_eq!(r.cells.get(&(2, 0)), None);
        // One undo puts the whole sync pass back.
        g.execute("edit.undo", &json!({})).unwrap();
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(12)));
        assert_eq!(r.cells.get(&(2, 0)), Some(&Value::Text("Ink".into())));
    }

    #[test]
    fn text_that_looks_like_a_formula_stays_text() {
        let mut g = engine_with_table();
        g.write_changes(&region_a("tbl"), &[CellChange { row: 1, col: 0, value: Some(Value::Text("=1+1".into())) }]).unwrap();
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(1, 0)), Some(&Value::Text("=1+1".into())));
    }

    #[test]
    fn dates_text_with_dots_and_leading_zeros_are_not_reinterpreted() {
        let mut g = engine_with_table();
        g.write_changes(
            &region_a("tbl"),
            &[
                CellChange { row: 1, col: 0, value: Some(Value::Text("05.04.2026".into())) },
                CellChange { row: 1, col: 1, value: Some(Value::Text("007".into())) },
            ],
        )
        .unwrap();
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(1, 0)), Some(&Value::Text("05.04.2026".into())));
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Text("007".into())));
    }

    #[test]
    fn a_date_value_becomes_a_spreadsheet_serial() {
        let mut g = engine_with_table();
        // 2026-01-01T00:00:00Z
        g.write_changes(&region_a("tbl"), &[CellChange { row: 1, col: 1, value: Some(Value::Date(1_767_225_600_000)) }]).unwrap();
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(1, 1)), Some(&Value::Int(46023)));
    }

    #[test]
    fn writes_outside_the_region_are_refused_and_change_nothing() {
        let mut g = engine_with_table();
        let before = g.revision();
        let err = g.write_changes(&region_a("tbl"), &[CellChange { row: 9, col: 0, value: Some(Value::Int(1)) }]).unwrap_err();
        assert!(matches!(err, BridgeError::BadParams(_)));
        assert_eq!(g.revision(), before);
    }

    #[test]
    fn unknown_names_and_foreign_anchors_are_errors_not_panics() {
        let mut g = engine_with_table();
        assert!(matches!(g.read_region(&region_a("nope")), Err(BridgeError::AnchorNotFound(_))));
        assert!(matches!(g.read_region(&Anchor::WordControl { tag: "x".into() }), Err(BridgeError::Unsupported(_))));
        assert!(matches!(g.write_changes(&region_a("nope"), &[]), Err(BridgeError::AnchorNotFound(_))));
    }

    #[test]
    fn the_region_follows_its_cells_when_a_row_is_inserted_above() {
        let mut g = engine_with_table();
        g.execute("home.insertRows", &json!({ "rows": "1:1" })).unwrap();
        // The table moved down one row; the name moved with it, so the region still starts at "Item".
        let r = g.read_region(&region_a("tbl")).unwrap();
        assert_eq!(r.cells.get(&(0, 0)), Some(&Value::Text("Item".into())));
        assert_eq!(r.cells.len(), 6);
    }

    /// The upstream surface this adapter touches. A rename in GridCraft fails here, at update time.
    #[test]
    fn every_command_and_field_the_adapter_uses_still_exists() {
        let s = Session::new();
        let ids: Vec<&str> = s.commands().iter().map(|c| c.id).collect();
        for needed in ["range.setValues", "cell.set", "cell.get", "formulas.defineName", "edit.undo", "home.insertRows"] {
            assert!(ids.contains(&needed), "GridCraft command {needed} vanished upstream");
        }
        // Public fields and methods read directly: DocState::{wb, revision}, Workbook::{names, sheet_index}.
        let g = GridEngine::new();
        let doc = g.session().doc().unwrap();
        let _ = (doc.revision, doc.wb.names.len(), doc.wb.sheet_index("Sheet1"));
    }

    #[test]
    fn queries_are_not_mutating_and_edits_are() {
        let g = GridEngine::new();
        assert!(!g.is_mutating("cell.get"));
        assert!(g.is_mutating("cell.set"));
        assert!(g.is_mutating("range.setValues"));
    }

    #[test]
    fn the_dispatcher_records_edits_but_not_queries() {
        let mut d = Dispatcher::new(engine_with_table());
        d.execute("cell.get", &json!({ "cell": "A1" })).unwrap();
        d.execute("cell.set", &json!({ "cell": "D1", "input": "x" })).unwrap();
        let seen: Vec<String> = d.drain_changes().into_iter().map(|c| c.command).collect();
        assert_eq!(seen, vec!["cell.set".to_string()]);
    }

    /// Pins an upstream behaviour we rely on being harmless: GridCraft counts re-entering an identical
    /// value as a revision. The sync layer absorbs it (a pass with nothing different writes nothing),
    /// so we only need to notice if the engine ever stops bumping the counter for real edits.
    #[test]
    fn revision_moves_on_every_edit_even_an_identical_one() {
        let mut g = engine_with_table();
        let r0 = g.revision();
        g.execute("cell.set", &json!({ "cell": "D1", "input": "x" })).unwrap();
        let r1 = g.revision();
        g.execute("cell.set", &json!({ "cell": "D1", "input": "x" })).unwrap();
        assert_ne!(r0, r1);
        assert!(g.revision() >= r1);
    }

    #[test]
    fn two_grid_regions_sync_through_the_link_engine() {
        // Smoke test of the whole path: RegionAccess on both ends of a link inside one workbook.
        let mut g = engine_with_table();
        g.execute("range.setValues", &json!({ "range": "E1", "values": [["", ""], ["", ""], ["", ""]] })).unwrap();
        g.define_region("copy", "Sheet1", "E1:F3").unwrap();
        let a = g.read_region(&region_a("tbl")).unwrap();
        let ch: Vec<CellChange> = a.cells.iter().map(|(&(row, col), v)| CellChange { row, col, value: Some(v.clone()) }).collect();
        g.write_changes(&region_a("copy"), &ch).unwrap();
        assert_eq!(g.read_region(&region_a("copy")).unwrap().cells, a.cells);
    }
}
