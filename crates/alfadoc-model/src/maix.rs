//! The maix store, **JSON-records profile** (MAIX-1.1 §3).
//!
//! Version 1 of Alfadoc stores maix data as JSON records inside the `.alfadoc` zip. The cell
//! address, dimension vectors, hashing, metadata cascade and delta semantics are exactly those of
//! the maix 1.0 draft; only the byte layout (4 KB pages, B+ trees) is deferred.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dims::{DimTable, DimVector};
use crate::error::{Error, Result};
use crate::format::SheetFormat;

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
    /// Column widths, row heights, hidden lines, frozen panes and cell styles.
    pub format: SheetFormat,
    pub(crate) cells: BTreeMap<Addr, Cell>,
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
    #[serde(default, skip_serializing_if = "SheetFormat::is_default")]
    format: SheetFormat,
    cells: Vec<CellRecord>,
}

impl From<SheetRepr> for Sheet {
    fn from(r: SheetRepr) -> Self {
        Sheet {
            id: r.id,
            name: r.name,
            columns: r.columns,
            meta: r.meta,
            format: r.format,
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
            format: s.format,
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
    /// The values declared for this dimension (so they are offered even before a cell uses them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// Longest dimension name or value.
pub const MAX_DIM_TEXT: usize = 64;
/// Most dimensions, and most values per dimension, that can be declared.
pub const MAX_DIMS: usize = 64;
pub const MAX_DIM_VALUES: usize = 500;

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
    /// Every dimension with every value known: the declared ones, the ones some cell is stored
    /// under, and the ones currently chosen.
    pub fn dimension_catalog(&self) -> BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut out: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for axis in &self.axes {
            out.entry(axis.id.clone()).or_default().extend(axis.values.iter().cloned());
        }
        for (_, vector) in self.vectors.iter() {
            for (k, v) in vector {
                out.entry(k.clone()).or_default().insert(v.clone());
            }
        }
        for (k, v) in &self.active_dims {
            out.entry(k.clone()).or_default().insert(v.clone());
        }
        out
    }

    /// Declares a dimension and, if given, one of its values. Returns false when the names are
    /// empty or too long, or there would be too many.
    pub fn declare_dimension(&mut self, key: &str, value: Option<&str>) -> bool {
        let key = key.trim();
        let value = value.map(str::trim);
        let ok = |t: &str| !t.is_empty() && t.chars().count() <= MAX_DIM_TEXT && !t.contains(['|', '=']);
        if !ok(key) || value.is_some_and(|v| !ok(v)) {
            return false;
        }
        let at = match self.axes.iter().position(|a| a.id == key) {
            Some(i) => i,
            None => {
                if self.axes.len() >= MAX_DIMS {
                    return false;
                }
                self.axes.push(DimAxis { id: key.to_string(), kind: "custom".into(), required: false, values: vec![] });
                self.axes.len() - 1
            }
        };
        if let (Some(v), Some(axis)) = (value, self.axes.get_mut(at))
            && !axis.values.iter().any(|x| x == v)
        {
            if axis.values.len() >= MAX_DIM_VALUES {
                return false;
            }
            axis.values.push(v.to_string());
        }
        true
    }

    /// Whether some stored vector uses `key = value` (or any value of `key` when `value` is None).
    pub fn dimension_in_use(&self, key: &str, value: Option<&str>) -> bool {
        self.vectors.iter().any(|(_, v)| v.get(key).is_some_and(|x| value.is_none_or(|w| w == x)))
    }

    /// Forgets a declared value (or a whole dimension when `value` is None) unless a stored
    /// vector uses it. A chosen value is un-chosen. Returns whether anything was forgotten.
    pub fn forget_dimension(&mut self, key: &str, value: Option<&str>) -> bool {
        if self.dimension_in_use(key, value) {
            return false;
        }
        match value {
            Some(v) => {
                if let Some(axis) = self.axes.iter_mut().find(|a| a.id == key) {
                    axis.values.retain(|x| x != v);
                }
                if self.active_dims.get(key).is_some_and(|x| x == v) {
                    self.active_dims.remove(key);
                }
            }
            None => {
                self.axes.retain(|a| a.id != key);
                self.active_dims.remove(key);
            }
        }
        true
    }

    /// How many cells (every dimension vector counted) the sheets hold together.
    pub fn cell_total(&self) -> usize {
        self.sheets.iter().map(Sheet::cell_count).sum()
    }

    /// The store without its cells: everything else, marked with the paged profile. The cells
    /// travel separately in the data pages ([`crate::paged`]).
    pub(crate) fn skeleton(&self) -> Maix {
        let sheets = self.sheets.iter().map(|s| Sheet { id: s.id, name: s.name.clone(), columns: s.columns.clone(), meta: s.meta.clone(), format: s.format.clone(), cells: BTreeMap::new() }).collect();
        Maix { profile: crate::paged::PROFILE_PAGED.into(), axes: self.axes.clone(), vectors: self.vectors.clone(), sheets, history: self.history.clone(), active_dims: self.active_dims.clone(), app_registry: self.app_registry.clone() }
    }

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

    /// The look (sizes, styles, frozen panes) of a sheet, to change it.
    pub fn format_mut(&mut self, id: u16) -> Result<&mut SheetFormat> {
        Ok(&mut self.sheet_mut(id)?.format)
    }

    pub(crate) fn sheet_mut(&mut self, id: u16) -> Result<&mut Sheet> {
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
    fn declared_dimensions_are_listed_even_before_a_cell_uses_them() {
        let mut m = Maix::default();
        assert!(m.declare_dimension("scenario", Some("actual")));
        assert!(m.declare_dimension("scenario", Some("budget")));
        assert!(m.declare_dimension("scenario", Some("budget")), "declaring twice is fine");
        assert!(m.declare_dimension("region", None));
        let catalog = m.dimension_catalog();
        assert_eq!(catalog["scenario"].iter().cloned().collect::<Vec<_>>(), ["actual", "budget"]);
        assert!(catalog["region"].is_empty());
        // Choosing one does not hide the others.
        m.active_dims.insert("scenario".into(), "actual".into());
        assert_eq!(m.dimension_catalog()["scenario"].len(), 2);
        // The declarations are saved.
        let back: Maix = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.dimension_catalog(), m.dimension_catalog());
    }

    #[test]
    fn odd_names_are_refused_and_used_values_cannot_be_forgotten() {
        let mut m = Maix::default();
        assert!(!m.declare_dimension("", Some("x")));
        assert!(!m.declare_dimension("a|b", None));
        assert!(!m.declare_dimension("a", Some("x=y")));
        assert!(!m.declare_dimension("a", Some(&"v".repeat(MAX_DIM_TEXT + 1))));
        let s = m.add_sheet("S");
        m.declare_dimension("scenario", Some("actual"));
        m.declare_dimension("scenario", Some("budget"));
        let dims: DimVector = [("scenario".to_string(), "actual".to_string())].into_iter().collect();
        m.set(s, 0, 0, &dims, Value::Int(1), &Stamp::new("t", 1)).unwrap();
        assert!(m.dimension_in_use("scenario", Some("actual")));
        assert!(!m.forget_dimension("scenario", Some("actual")), "a cell is stored under it");
        assert!(!m.forget_dimension("scenario", None));
        m.active_dims.insert("scenario".into(), "budget".into());
        assert!(m.forget_dimension("scenario", Some("budget")));
        assert!(m.active_dims.is_empty(), "a forgotten value is no longer chosen");
        assert!(!m.dimension_catalog()["scenario"].contains("budget"));
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
