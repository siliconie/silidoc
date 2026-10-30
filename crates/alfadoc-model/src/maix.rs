//! The maix store, **JSON-records profile** (MAIX-1.1 §3).
//!
//! Version 1 of Alfadoc stores maix data as JSON records inside the `.alfadoc` zip. The cell
//! address, dimension vectors, hashing, metadata cascade and delta semantics are exactly those of
//! the maix 1.0 draft; only the byte layout (4 KB pages, B+ trees) is deferred.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dims::{DimTable, DimVector};
use crate::error::{Error, Result};

/// A cell value. Formulas keep their text in [`Cell::formula`]; `Value` is the stored/cached result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Value {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    /// Milliseconds since the Unix epoch (UTC).
    Date(i64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    pub version: u32,
    pub updated_at: u64,
    /// Cell-level metadata, namespaced: `{"pos": {"tax_rate": 0.0}}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, Meta>,
}

pub type Meta = BTreeMap<String, serde_json::Value>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, Meta>,
}

type Addr = (u32, u16, u64); // row, col, dim_hash

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "SheetRepr", into = "SheetRepr")]
pub struct Sheet {
    pub id: u16,
    pub name: String,
    pub columns: BTreeMap<u16, Column>,
    pub meta: BTreeMap<String, Meta>,
    cells: BTreeMap<Addr, Cell>,
}

#[derive(Serialize, Deserialize)]
struct CellRecord {
    row: u32,
    col: u16,
    #[serde(default)]
    dim: u64,
    #[serde(flatten)]
    cell: Cell,
}

#[derive(Serialize, Deserialize)]
struct SheetRepr {
    id: u16,
    name: String,
    #[serde(default)]
    columns: BTreeMap<u16, Column>,
    #[serde(default)]
    meta: BTreeMap<String, Meta>,
    cells: Vec<CellRecord>,
}

impl From<SheetRepr> for Sheet {
    fn from(r: SheetRepr) -> Self {
        Sheet {
            id: r.id,
            name: r.name,
            columns: r.columns,
            meta: r.meta,
            cells: r.cells.into_iter().map(|c| ((c.row, c.col, c.dim), c.cell)).collect(),
        }
    }
}

impl From<Sheet> for SheetRepr {
    fn from(s: Sheet) -> Self {
        SheetRepr {
            id: s.id,
            name: s.name,
            columns: s.columns,
            meta: s.meta,
            cells: s.cells.into_iter().map(|((row, col, dim), cell)| CellRecord { row, col, dim, cell }).collect(),
        }
    }
}

impl Sheet {
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn get(&self, row: u32, col: u16, dim: u64) -> Option<&Cell> {
        self.cells.get(&(row, col, dim))
    }

    /// One past the last row and column that hold a value (so `(0, 0)` for an empty sheet).
    pub fn extent(&self) -> (u32, u16) {
        let mut out = (0u32, 0u16);
        for ((row, col, _), cell) in &self.cells {
            if cell.value != Value::Null {
                out.0 = out.0.max(row.saturating_add(1));
                out.1 = out.1.max(col.saturating_add(1));
            }
        }
        out
    }

    /// The cell at `(row, col)` under every dimension vector that has one, as `(dim hash, cell)`.
    pub fn across(&self, row: u32, col: u16) -> impl Iterator<Item = (u64, &Cell)> {
        self.cells.range((row, col, 0)..=(row, col, u64::MAX)).map(|((_, _, d), c)| (*d, c))
    }

    /// Every cell that holds a formula: `(row, col, dim hash, formula)`.
    pub fn formulas(&self) -> Vec<(u32, u16, u64, String)> {
        self.cells.iter().filter_map(|((r, c, d), cell)| cell.formula.as_ref().map(|f| (*r, *c, *d, f.clone()))).collect()
    }

    /// All cells at `dim`, in row-major order.
    pub fn slice(&self, dim: u64) -> impl Iterator<Item = (u32, u16, &Cell)> {
        self.cells.iter().filter(move |((_, _, d), _)| *d == dim).map(|((r, c, _), cell)| (*r, *c, cell))
    }
}

/// Who changed something and when (milliseconds since the Unix epoch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub author: String,
    pub at: u64,
}

impl Stamp {
    pub fn new(author: &str, at: u64) -> Self {
        Stamp { author: author.to_string(), at }
    }
}

/// One append-only history entry (maix §8.9, value changes only in this profile).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub sheet: u16,
    pub row: u32,
    pub col: u16,
    pub dim: u64,
    pub version: u32,
    pub old: Value,
    pub new: Value,
    pub author: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DimAxis {
    pub id: String,
    /// `value`, `time`, `scenario`, `role` or `custom` (spec §6.2).
    pub kind: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Maix {
    /// Profile marker so readers can reject what they do not understand.
    pub profile: String,
    pub axes: Vec<DimAxis>,
    pub vectors: DimTable,
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub history: Vec<Delta>,
    /// The dimension vector the editors show and write (empty: the plain, undimensioned values).
    #[serde(default, skip_serializing_if = "DimVector::is_empty")]
    pub active_dims: DimVector,
    /// Workbook-level app registry and defaults (the manifest JSON payload of maix §8.1.2).
    #[serde(default)]
    pub app_registry: BTreeMap<String, serde_json::Value>,
}

pub const PROFILE: &str = "maix/1.1-json";

impl Default for Maix {
    fn default() -> Self {
        Maix { profile: PROFILE.into(), axes: vec![], vectors: DimTable::default(), sheets: vec![], history: vec![], active_dims: DimVector::new(), app_registry: BTreeMap::new() }
    }
}

impl Maix {
    pub fn add_sheet(&mut self, name: &str) -> u16 {
        let id = self.sheets.iter().map(|s| s.id).max().map_or(1, |m| m.saturating_add(1));
        self.sheets.push(Sheet { id, name: name.to_string(), ..Sheet::default() });
        id
    }

    pub fn sheet(&self, id: u16) -> Result<&Sheet> {
        self.sheets.iter().find(|s| s.id == id).ok_or(Error::UnknownSheet(id))
    }

    /// Renames a sheet. False when the name is empty or another sheet already has it.
    pub fn rename_sheet(&mut self, id: u16, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.sheets.iter().any(|s| s.id != id && s.name == name) {
            return false;
        }
        match self.sheet_mut(id) {
            Ok(sheet) => {
                sheet.name = name.to_string();
                true
            }
            Err(_) => false,
        }
    }

    pub fn sheet_by_name(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }

    fn sheet_mut(&mut self, id: u16) -> Result<&mut Sheet> {
        self.sheets.iter_mut().find(|s| s.id == id).ok_or(Error::UnknownSheet(id))
    }

    /// Writes a value, bumps the cell version and records a delta. Writing an equal value is a
    /// no-op (no version bump, no delta), which keeps sync loops from echoing.
    /// Returns whether anything changed.
    pub fn set(&mut self, sheet: u16, row: u32, col: u16, dims: &DimVector, value: Value, by: &Stamp) -> Result<bool> {
        let dim = self.vectors.intern(dims)?;
        let sh = self.sheet_mut(sheet)?;
        let key = (row, col, dim);
        let (old, version) = match sh.cells.get(&key) {
            Some(c) if c.value == value => return Ok(false),
            Some(c) => (c.value.clone(), c.version.saturating_add(1)),
            None => (Value::Null, 1),
        };
        let cell = sh.cells.entry(key).or_insert_with(|| Cell { value: Value::Null, formula: None, version: 0, updated_at: 0, meta: BTreeMap::new() });
        cell.value = value.clone();
        cell.version = version;
        cell.updated_at = by.at;
        self.history.push(Delta { sheet, row, col, dim, version, old, new: value, author: by.author.clone(), at: by.at });
        Ok(true)
    }

    /// The formula of a cell, if it has one.
    pub fn formula(&self, sheet: u16, row: u32, col: u16, dims: &DimVector) -> Option<&str> {
        let dim = self.vectors.find(dims)?;
        self.sheet(sheet).ok()?.get(row, col, dim)?.formula.as_deref()
    }

    /// Sets (`Some`) or removes (`None`) a cell's formula text. The value is left alone: run
    /// [`Maix::recalc`] afterwards. Returns whether the formula changed.
    pub fn set_formula(&mut self, sheet: u16, row: u32, col: u16, dims: &DimVector, formula: Option<&str>) -> Result<bool> {
        let dim = if formula.is_some() {
            self.vectors.intern(dims)?
        } else {
            match self.vectors.find(dims) {
                Some(d) => d,
                None => return Ok(false), // no such vector, so no such cell
            }
        };
        let sh = self.sheet_mut(sheet)?;
        let key = (row, col, dim);
        let new = formula.map(str::to_string);
        match sh.cells.get_mut(&key) {
            Some(cell) if cell.formula == new => Ok(false),
            Some(cell) => {
                cell.formula = new;
                Ok(true)
            }
            None if new.is_none() => Ok(false),
            None => {
                sh.cells.insert(key, Cell { value: Value::Null, formula: new, version: 0, updated_at: 0, meta: BTreeMap::new() });
                Ok(true)
            }
        }
    }

    pub fn get(&self, sheet: u16, row: u32, col: u16, dims: &DimVector) -> Result<Option<&Value>> {
        let sh = self.sheet(sheet)?;
        Ok(match self.vectors.find(dims) {
            Some(dim) => sh.get(row, col, dim).map(|c| &c.value),
            None => None,
        })
    }
}

/// Metadata cascade (maix §9): `levels` are ordered **lowest to highest priority**. A level that
/// contains `"__cascade": "replace"` discards everything below it (§9.4).
pub fn resolve_cascade(levels: &[&Meta]) -> Meta {
    let mut out = Meta::new();
    for level in levels {
        if level.get("__cascade").and_then(|v| v.as_str()) == Some("replace") {
            out.clear();
        }
        for (k, v) in level.iter() {
            if k != "__cascade" {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dv(pairs: &[(&str, &str)]) -> DimVector {
        pairs.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
    }

    #[test]
    fn same_cell_different_dimensions_are_distinct() {
        let mut m = Maix::default();
        let s = m.add_sheet("BOQ");
        let est = dv(&[("role", "estimator")]);
        let site = dv(&[("role", "site_manager")]);
        assert!(m.set(s, 1, 3, &est, Value::Int(4500), &Stamp::new("t", 1)).unwrap());
        assert!(m.set(s, 1, 3, &site, Value::Text("50 bags".into()), &Stamp::new("t", 2)).unwrap());
        assert_eq!(m.get(s, 1, 3, &est).unwrap(), Some(&Value::Int(4500)));
        assert_eq!(m.get(s, 1, 3, &site).unwrap(), Some(&Value::Text("50 bags".into())));
        assert_eq!(m.get(s, 1, 3, &DimVector::new()).unwrap(), None);
    }

    #[test]
    fn equal_write_is_a_noop() {
        let mut m = Maix::default();
        let s = m.add_sheet("A");
        assert!(m.set(s, 0, 0, &DimVector::new(), Value::Int(1), &Stamp::new("t", 1)).unwrap());
        assert!(!m.set(s, 0, 0, &DimVector::new(), Value::Int(1), &Stamp::new("t", 2)).unwrap());
        assert_eq!(m.history.len(), 1);
        assert!(m.set(s, 0, 0, &DimVector::new(), Value::Int(2), &Stamp::new("t", 3)).unwrap());
        assert_eq!(m.history.len(), 2);
        assert_eq!(m.history[1].old, Value::Int(1));
        assert_eq!(m.sheet(s).unwrap().get(0, 0, 0).unwrap().version, 2);
    }

    #[test]
    fn extent_ignores_cleared_cells_and_renaming_keeps_names_unique() {
        let mut m = Maix::default();
        let a = m.add_sheet("A");
        let b = m.add_sheet("B");
        assert_eq!(m.sheet(a).unwrap().extent(), (0, 0));
        m.set(a, 4, 2, &DimVector::new(), Value::Int(1), &Stamp::new("t", 1)).unwrap();
        m.set(a, 9, 7, &DimVector::new(), Value::Int(1), &Stamp::new("t", 2)).unwrap();
        assert_eq!(m.sheet(a).unwrap().extent(), (10, 8));
        m.set(a, 9, 7, &DimVector::new(), Value::Null, &Stamp::new("t", 3)).unwrap();
        assert_eq!(m.sheet(a).unwrap().extent(), (5, 3));
        assert!(!m.rename_sheet(b, "A"));
        assert!(!m.rename_sheet(b, "  "));
        assert!(m.rename_sheet(b, " Costs "));
        assert_eq!(m.sheet(b).unwrap().name, "Costs");
    }

    #[test]
    fn unknown_sheet_is_an_error_not_a_panic() {
        let mut m = Maix::default();
        assert!(m.set(9, 0, 0, &DimVector::new(), Value::Null, &Stamp::new("t", 1)).is_err());
    }

    #[test]
    fn json_round_trip_keeps_cells_and_dims() {
        let mut m = Maix::default();
        let s = m.add_sheet("A");
        m.set(s, 2, 1, &dv(&[("time", "Q1")]), Value::Float(1.5), &Stamp::new("t", 1)).unwrap();
        let text = serde_json::to_string(&m).unwrap();
        let back: Maix = serde_json::from_str(&text).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn cascade_matches_the_spec_example() {
        let schema: Meta = serde_json::from_value(json!({"editable": true, "tax_rate": 0.18, "render": "text"})).unwrap();
        let workbook: Meta = serde_json::from_value(json!({"currency": "TZS"})).unwrap();
        let sheet: Meta = serde_json::from_value(json!({"editable": false})).unwrap();
        let column: Meta = serde_json::from_value(json!({"field_role": "price", "display": "currency", "render": "price_tag"})).unwrap();
        let cell: Meta = serde_json::from_value(json!({"tax_rate": 0.0})).unwrap();
        let r = resolve_cascade(&[&schema, &workbook, &sheet, &column, &cell]);
        assert_eq!(r["editable"], json!(false));
        assert_eq!(r["tax_rate"], json!(0.0));
        assert_eq!(r["render"], json!("price_tag"));
        assert_eq!(r["currency"], json!("TZS"));
        assert_eq!(r["field_role"], json!("price"));
    }

    #[test]
    fn cascade_replace_discards_lower_levels() {
        let low: Meta = serde_json::from_value(json!({"a": 1, "b": 2})).unwrap();
        let top: Meta = serde_json::from_value(json!({"__cascade": "replace", "c": 3})).unwrap();
        let r = resolve_cascade(&[&low, &top]);
        assert_eq!(r.len(), 1);
        assert_eq!(r["c"], json!(3));
    }
}
