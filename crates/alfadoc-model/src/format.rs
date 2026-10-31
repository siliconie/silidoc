//! How a maix sheet *looks*, apart from what it holds: row heights and column widths, hidden rows
//! and columns, frozen panes, and cell styles (bold, colours, alignment, number format…).
//!
//! None of this is part of the maix 1.0 data model; it is stored beside the cells in the sheet's
//! JSON record under `format` and older files simply do not have it. Styles are position-based
//! (they do not depend on the active dimension vector).
//!
//! * A style is interned: equal styles share one id. Id `0` is the plain style.
//! * A cell's style is its own entry if it has one, else its column's, else its row's.
//! * A format applied to whole columns (or whole rows) is stored once per column (row), not once
//!   per cell, so formatting column `B` of a million-row sheet costs one entry.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::extras::Extras;

/// Most rows and columns a maix sheet can have (the same as Excel and GridCraft).
pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u16 = 16_384;
/// Row height and column width when none was set, in points.
pub const DEFAULT_ROW_H: f32 = 24.0;
pub const DEFAULT_COL_W: f32 = 96.0;
/// Smallest and largest size a row or column can be given.
pub const MIN_SIZE: f32 = 6.0;
pub const MAX_ROW_H: f32 = 800.0;
pub const MAX_COL_W: f32 = 1_600.0;
/// Most cells one style change touches one by one (whole rows and columns are stored apart).
pub const MAX_STYLE_CELLS: u64 = 250_000;

/// A rectangle of cells: (top, left, bottom, right), both ends included.
pub type Rect = (u32, u16, u32, u16);

/// Sizes, hidden lines and frozen panes of one sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub col_widths: BTreeMap<u16, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub row_heights: BTreeMap<u32, f32>,
    /// Hidden columns keep their width in `col_widths`, so showing them again restores it.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden_cols: BTreeSet<u16>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden_rows: BTreeSet<u32>,
    /// Rows at the top and columns at the left that stay in view while the rest scrolls.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub freeze_rows: u32,
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub freeze_cols: u16,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

fn is_zero_u16(n: &u16) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Layout {
    pub fn is_default(&self) -> bool {
        *self == Layout::default()
    }

    /// The width of column `col` (0 when hidden).
    pub fn col_width(&self, col: u16) -> f32 {
        if self.hidden_cols.contains(&col) { 0.0 } else { self.col_widths.get(&col).copied().unwrap_or(DEFAULT_COL_W) }
    }

    pub fn row_height(&self, row: u32) -> f32 {
        if self.hidden_rows.contains(&row) { 0.0 } else { self.row_heights.get(&row).copied().unwrap_or(DEFAULT_ROW_H) }
    }

    /// Sets the width of the columns in `cols`; `None` goes back to the default.
    pub fn set_col_width(&mut self, cols: std::ops::RangeInclusive<u16>, width: Option<f32>) {
        for c in cols {
            match width {
                Some(w) => {
                    self.col_widths.insert(c, w.clamp(MIN_SIZE, MAX_COL_W));
                }
                None => {
                    self.col_widths.remove(&c);
                }
            }
        }
    }

    pub fn set_row_height(&mut self, rows: std::ops::RangeInclusive<u32>, height: Option<f32>) {
        let rows = *rows.start()..=(*rows.end()).min(MAX_ROWS - 1);
        let count = u64::from(*rows.end()).saturating_sub(u64::from(*rows.start())) + 1;
        if count > 100_000 {
            return; // a whole-sheet height is meant to change the default, which is not stored
        }
        for r in rows {
            match height {
                Some(h) => {
                    self.row_heights.insert(r, h.clamp(MIN_SIZE, MAX_ROW_H));
                }
                None => {
                    self.row_heights.remove(&r);
                }
            }
        }
    }

    pub fn set_cols_hidden(&mut self, cols: std::ops::RangeInclusive<u16>, hidden: bool) {
        for c in cols {
            if hidden {
                self.hidden_cols.insert(c);
            } else {
                self.hidden_cols.remove(&c);
            }
        }
    }

    pub fn set_rows_hidden(&mut self, rows: std::ops::RangeInclusive<u32>, hidden: bool) {
        let rows = *rows.start()..=(*rows.end()).min(MAX_ROWS - 1);
        let count = u64::from(*rows.end()).saturating_sub(u64::from(*rows.start())) + 1;
        if count > 100_000 {
            return;
        }
        for r in rows {
            if hidden {
                self.hidden_rows.insert(r);
            } else {
                self.hidden_rows.remove(&r);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HAlign {
    /// Numbers to the right, text to the left.
    #[default]
    General,
    Left,
    Center,
    Right,
}

impl HAlign {
    fn is_general(&self) -> bool {
        *self == HAlign::General
    }
}

/// What a cell looks like. `numfmt` is an Excel number-format code (`#,##0.00`, `0%`,
/// `yyyy-mm-dd`…); empty means General.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellStyle {
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    /// Text colour as `0xRRGGBB`; `None` is the theme's text colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Background colour as `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<u32>,
    #[serde(default, skip_serializing_if = "HAlign::is_general")]
    pub align: HAlign,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wrap: bool,
    /// Font size in points; 0 is the default size.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub size: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub numfmt: String,
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

impl CellStyle {
    pub fn is_plain(&self) -> bool {
        *self == CellStyle::default()
    }
}

/// The look of a sheet: layout plus the style table and where styles apply.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "FormatRepr", into = "FormatRepr")]
pub struct SheetFormat {
    pub layout: Layout,
    /// Merged cells, conditional formatting, validation, comments and groups.
    pub extras: Extras,
    /// Interned styles; the id of `styles[i]` is `i + 1`.
    styles: Vec<CellStyle>,
    cells: BTreeMap<(u32, u16), u32>,
    cols: BTreeMap<u16, u32>,
    rows: BTreeMap<u32, u32>,
}

#[derive(Serialize, Deserialize)]
struct StyleCell {
    row: u32,
    col: u16,
    style: u32,
}

#[derive(Serialize, Deserialize, Default)]
struct FormatRepr {
    #[serde(default)]
    layout: Layout,
    #[serde(default, skip_serializing_if = "Extras::is_empty")]
    extras: Extras,
    #[serde(default)]
    styles: Vec<CellStyle>,
    #[serde(default)]
    cells: Vec<StyleCell>,
    #[serde(default)]
    cols: BTreeMap<u16, u32>,
    #[serde(default)]
    rows: BTreeMap<u32, u32>,
}

impl From<FormatRepr> for SheetFormat {
    fn from(r: FormatRepr) -> Self {
        let n = u32::try_from(r.styles.len()).unwrap_or(u32::MAX);
        // An id that points outside the table (a damaged file) means plain.
        let ok = |id: u32| if id <= n { id } else { 0 };
        SheetFormat {
            layout: r.layout,
            extras: r.extras,
            cells: r.cells.into_iter().map(|c| ((c.row, c.col), ok(c.style))).collect(),
            cols: r.cols.into_iter().map(|(k, v)| (k, ok(v))).collect(),
            rows: r.rows.into_iter().map(|(k, v)| (k, ok(v))).collect(),
            styles: r.styles,
        }
    }
}

impl From<SheetFormat> for FormatRepr {
    fn from(f: SheetFormat) -> Self {
        FormatRepr {
            layout: f.layout,
            extras: f.extras,
            styles: f.styles,
            cells: f.cells.into_iter().map(|((row, col), style)| StyleCell { row, col, style }).collect(),
            cols: f.cols,
            rows: f.rows,
        }
    }
}

/// Which way a range of rows or columns is being counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Cols,
}

/// Inserting or deleting `count` lines at index `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Insert { at: u32, count: u32 },
    Delete { at: u32, count: u32 },
}

impl Shift {
    /// Where the line `i` ends up; `None` when it was deleted.
    pub fn map(self, i: u32) -> Option<u32> {
        match self {
            Shift::Insert { at, count } => Some(if i >= at { i.saturating_add(count) } else { i }),
            Shift::Delete { at, count } => {
                if i < at {
                    Some(i)
                } else if i - at < count {
                    None
                } else {
                    Some(i - count)
                }
            }
        }
    }

    /// Where the lines `lo..=hi` end up. An insertion inside grows the range; a deletion shrinks
    /// it; `None` when every line of it was deleted.
    pub fn map_range(self, lo: u32, hi: u32) -> Option<(u32, u32)> {
        match self {
            Shift::Insert { at, count } => {
                let nl = if lo >= at { lo.saturating_add(count) } else { lo };
                let nh = if hi >= at { hi.saturating_add(count) } else { hi };
                Some((nl, nh))
            }
            Shift::Delete { at, count } => {
                let end = u64::from(at) + u64::from(count); // first line after the deleted ones
                let (l, h) = (u64::from(lo), u64::from(hi));
                if l >= u64::from(at) && h < end {
                    return None;
                }
                let nl = if l >= end { l - u64::from(count) } else if l >= u64::from(at) { u64::from(at) } else { l };
                let nh = if h >= end { h - u64::from(count) } else if h >= u64::from(at) { u64::from(at).checked_sub(1)? } else { h };
                if nl > nh {
                    return None;
                }
                Some((u32::try_from(nl).ok()?, u32::try_from(nh).ok()?))
            }
        }
    }
}

fn remap_keys<K: Ord + Copy, V>(map: &mut BTreeMap<K, V>, f: impl Fn(K) -> Option<K>) {
    let old = std::mem::take(map);
    for (k, v) in old {
        if let Some(nk) = f(k) {
            map.insert(nk, v);
        }
    }
}

fn remap_set<K: Ord + Copy>(set: &mut BTreeSet<K>, f: impl Fn(K) -> Option<K>) {
    let old = std::mem::take(set);
    set.extend(old.into_iter().filter_map(f));
}

impl SheetFormat {
    pub fn is_default(&self) -> bool {
        self.layout.is_default() && self.extras.is_empty() && self.cells.is_empty() && self.cols.is_empty() && self.rows.is_empty()
    }

    /// The style with this id (`None` for the plain style).
    pub fn get(&self, id: u32) -> Option<&CellStyle> {
        usize::try_from(id).ok()?.checked_sub(1).and_then(|i| self.styles.get(i))
    }

    /// The id of an equal style, added to the table when there is none. Plain is always `0`.
    pub fn intern(&mut self, style: &CellStyle) -> u32 {
        if style.is_plain() {
            return 0;
        }
        if let Some(i) = self.styles.iter().position(|s| s == style) {
            return u32::try_from(i + 1).unwrap_or(0);
        }
        self.styles.push(style.clone());
        u32::try_from(self.styles.len()).unwrap_or(0)
    }

    /// The id of the style a cell has: its own, else its column's, else its row's.
    pub fn id_at(&self, row: u32, col: u16) -> u32 {
        self.cells.get(&(row, col)).or_else(|| self.cols.get(&col)).or_else(|| self.rows.get(&row)).copied().unwrap_or(0)
    }

    pub fn style_at(&self, row: u32, col: u16) -> CellStyle {
        self.get(self.id_at(row, col)).cloned().unwrap_or_default()
    }

    /// True when any cell, column or row has a style.
    pub fn has_styles(&self) -> bool {
        !self.cells.is_empty() || !self.cols.is_empty() || !self.rows.is_empty()
    }

    /// Gives every cell of `range` the style `f` makes from its current one. Whole columns and
    /// whole rows are stored as such. Returns how many cells, columns or rows were touched (0 when
    /// the range was too big to do cell by cell).
    pub fn apply(&mut self, range: Rect, f: &dyn Fn(&CellStyle) -> CellStyle) -> usize {
        let (top, left, bottom, right) = range;
        let whole_cols = top == 0 && bottom >= MAX_ROWS - 1;
        let whole_rows = left == 0 && right >= MAX_COLS - 1;
        if whole_cols {
            let keys: Vec<(u32, u16)> = self.cells.keys().filter(|(_, c)| *c >= left && *c <= right).copied().collect();
            for key in keys {
                let new = f(&self.style_at(key.0, key.1));
                let id = self.intern(&new);
                self.cells.insert(key, id);
            }
            for c in left..=right.min(MAX_COLS - 1) {
                let current = self.cols.get(&c).and_then(|id| self.get(*id)).cloned().unwrap_or_default();
                let id = self.intern(&f(&current));
                if id == 0 {
                    self.cols.remove(&c);
                } else {
                    self.cols.insert(c, id);
                }
            }
            return usize::from(right.min(MAX_COLS - 1) - left) + 1;
        }
        if whole_rows {
            let keys: Vec<(u32, u16)> = self.cells.keys().filter(|(r, _)| *r >= top && *r <= bottom).copied().collect();
            for key in keys {
                let new = f(&self.style_at(key.0, key.1));
                let id = self.intern(&new);
                self.cells.insert(key, id);
            }
            let last = bottom.min(MAX_ROWS - 1);
            if u64::from(last) - u64::from(top) + 1 > MAX_STYLE_CELLS {
                return 0;
            }
            for r in top..=last {
                let current = self.rows.get(&r).and_then(|id| self.get(*id)).cloned().unwrap_or_default();
                let id = self.intern(&f(&current));
                if id == 0 {
                    self.rows.remove(&r);
                } else {
                    self.rows.insert(r, id);
                }
            }
            return usize::try_from(last - top).unwrap_or(0) + 1;
        }
        let count = (u64::from(bottom) - u64::from(top) + 1) * (u64::from(right) - u64::from(left) + 1);
        if count > MAX_STYLE_CELLS {
            return 0;
        }
        for r in top..=bottom {
            for c in left..=right {
                let new = f(&self.style_at(r, c));
                self.set_cell(r, c, &new);
            }
        }
        usize::try_from(count).unwrap_or(0)
    }

    /// Gives one cell exactly this style.
    pub fn set_cell(&mut self, row: u32, col: u16, style: &CellStyle) {
        let id = self.intern(style);
        let inherited = self.cols.get(&col).or_else(|| self.rows.get(&row)).copied().unwrap_or(0);
        if id == inherited {
            self.cells.remove(&(row, col));
        } else {
            self.cells.insert((row, col), id);
        }
    }

    /// Takes every cell, column and row style off the cells in `range`.
    pub fn clear_range(&mut self, range: Rect) -> usize {
        self.apply(range, &|_| CellStyle::default())
    }

    /// Moves what the shift moves: styles, sizes, hidden lines and the frozen pane.
    pub fn shift(&mut self, axis: Axis, op: Shift) {
        self.extras.shift(axis, op);
        match axis {
            Axis::Rows => {
                remap_keys(&mut self.cells, |(r, c)| op.map(r).map(|r| (r, c)));
                remap_keys(&mut self.rows, |r| op.map(r));
                remap_keys(&mut self.layout.row_heights, |r| op.map(r));
                remap_set(&mut self.layout.hidden_rows, |r| op.map(r));
                if let Shift::Delete { at, count } = op {
                    let inside = self.layout.freeze_rows.min(at.saturating_add(count)).saturating_sub(at);
                    self.layout.freeze_rows = self.layout.freeze_rows.saturating_sub(inside);
                }
            }
            Axis::Cols => {
                let m = |c: u16| op.map(u32::from(c)).and_then(|c| u16::try_from(c).ok());
                remap_keys(&mut self.cells, |(r, c)| m(c).map(|c| (r, c)));
                remap_keys(&mut self.cols, m);
                remap_keys(&mut self.layout.col_widths, m);
                remap_set(&mut self.layout.hidden_cols, m);
                if let Shift::Delete { at, count } = op {
                    let frozen = u32::from(self.layout.freeze_cols);
                    let inside = frozen.min(at.saturating_add(count)).saturating_sub(at);
                    self.layout.freeze_cols = u16::try_from(frozen - inside).unwrap_or(0);
                }
            }
        }
    }

    /// Moves the per-cell styles of rows `top..=bottom` (columns `left..=right`) the way
    /// `new_row_of` says, for sorting. `new_row_of[i]` is where the row `top + i` goes.
    pub fn permute_rows(&mut self, top: u32, left: u16, right: u16, new_row_of: &[u32]) {
        let mut taken: Vec<((u32, u16), u32)> = vec![];
        let keys: Vec<(u32, u16)> = self.cells.range((top, left)..).take_while(|((r, _), _)| *r < top.saturating_add(u32::try_from(new_row_of.len()).unwrap_or(u32::MAX))).filter(|((_, c), _)| *c >= left && *c <= right).map(|(k, _)| *k).collect();
        for k in keys {
            if let Some(id) = self.cells.remove(&k) {
                taken.push((k, id));
            }
        }
        for ((r, c), id) in taken {
            let i = usize::try_from(r - top).unwrap_or(usize::MAX);
            if let Some(nr) = new_row_of.get(i) {
                self.cells.insert((*nr, c), id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> CellStyle {
        CellStyle { bold: true, ..CellStyle::default() }
    }

    #[test]
    fn equal_styles_share_an_id_and_plain_is_zero() {
        let mut f = SheetFormat::default();
        let a = f.intern(&bold());
        let b = f.intern(&bold());
        assert_eq!(a, b);
        assert_ne!(a, 0);
        assert_eq!(f.intern(&CellStyle::default()), 0);
        assert_eq!(f.get(a), Some(&bold()));
        assert_eq!(f.get(0), None);
    }

    #[test]
    fn a_cell_style_wins_over_its_column_and_row() {
        let mut f = SheetFormat::default();
        f.apply((0, 1, MAX_ROWS - 1, 1), &|s| CellStyle { italic: true, ..s.clone() });
        assert!(f.style_at(5, 1).italic);
        assert!(!f.style_at(5, 2).italic);
        f.apply((5, 1, 5, 1), &|s| CellStyle { bold: true, ..s.clone() });
        let both = f.style_at(5, 1);
        assert!(both.bold && both.italic);
        // Taking the style off a cell in a formatted column leaves an explicit plain cell.
        f.clear_range((5, 1, 5, 1));
        assert!(f.style_at(5, 1).is_plain());
        assert!(f.style_at(6, 1).italic);
    }

    #[test]
    fn whole_columns_cost_one_entry_each() {
        let mut f = SheetFormat::default();
        assert_eq!(f.apply((0, 2, MAX_ROWS - 1, 3), &|s| CellStyle { bold: true, ..s.clone() }), 2);
        assert!(f.cells.is_empty());
        assert!(f.style_at(900_000, 3).bold);
    }

    #[test]
    fn too_big_a_block_is_refused_not_half_done() {
        let mut f = SheetFormat::default();
        assert_eq!(f.apply((0, 0, 5_000, 100), &|s| CellStyle { bold: true, ..s.clone() }), 0);
        assert!(!f.has_styles());
    }

    #[test]
    fn inserting_and_deleting_rows_moves_styles_sizes_and_hidden_lines() {
        let mut f = SheetFormat::default();
        f.apply((3, 0, 3, 0), &|s| CellStyle { bold: true, ..s.clone() });
        f.layout.set_row_height(3..=3, Some(40.0));
        f.layout.set_rows_hidden(5..=5, true);
        f.shift(Axis::Rows, Shift::Insert { at: 2, count: 2 });
        assert!(f.style_at(5, 0).bold);
        assert_eq!(f.layout.row_height(5), 40.0);
        assert!(f.layout.hidden_rows.contains(&7));
        f.shift(Axis::Rows, Shift::Delete { at: 5, count: 1 });
        assert!(f.style_at(5, 0).is_plain());
        assert!(!f.layout.row_heights.contains_key(&5));
        assert!(f.layout.hidden_rows.contains(&6));
    }

    #[test]
    fn columns_shift_like_rows() {
        let mut f = SheetFormat::default();
        f.layout.set_col_width(2..=2, Some(200.0));
        f.layout.freeze_cols = 3;
        f.shift(Axis::Cols, Shift::Insert { at: 0, count: 1 });
        assert_eq!(f.layout.col_width(3), 200.0);
        // Columns 1 and 2 are two of the three frozen ones.
        f.shift(Axis::Cols, Shift::Delete { at: 1, count: 2 });
        assert_eq!(f.layout.freeze_cols, 1);
    }

    #[test]
    fn ranges_grow_and_shrink_with_the_lines_around_them() {
        let ins = Shift::Insert { at: 5, count: 2 };
        assert_eq!(ins.map_range(2, 4), Some((2, 4)));
        assert_eq!(ins.map_range(2, 6), Some((2, 8)));
        assert_eq!(ins.map_range(5, 6), Some((7, 8)));
        let del = Shift::Delete { at: 5, count: 3 };
        assert_eq!(del.map_range(2, 4), Some((2, 4)));
        assert_eq!(del.map_range(4, 6), Some((4, 4)));
        assert_eq!(del.map_range(6, 9), Some((5, 6)));
        assert_eq!(del.map_range(5, 7), None);
        assert_eq!(del.map(4), Some(4));
        assert_eq!(del.map(6), None);
        assert_eq!(del.map(8), Some(5));
    }

    #[test]
    fn a_damaged_style_id_means_plain() {
        let text = r#"{"layout":{},"styles":[{"bold":true}],"cells":[{"row":0,"col":0,"style":9},{"row":1,"col":0,"style":1}]}"#;
        let f: SheetFormat = serde_json::from_str(text).unwrap();
        assert!(f.style_at(0, 0).is_plain());
        assert!(f.style_at(1, 0).bold);
    }

    #[test]
    fn format_round_trips_through_json() {
        let mut f = SheetFormat::default();
        f.apply((1, 1, 2, 2), &|s| CellStyle { fill: Some(0xFFEE00), numfmt: "0.00".into(), align: HAlign::Right, ..s.clone() });
        f.layout.set_col_width(0..=0, Some(150.0));
        f.layout.freeze_rows = 1;
        let back: SheetFormat = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(f, back);
    }
}
