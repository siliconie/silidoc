//! Changing the *shape* of a maix sheet: inserting and deleting rows and columns, sorting rows,
//! and keeping formulas right when cells move.
//!
//! Formula references (`B2`, `$B$2`, `A1:C9`) are rewritten as text, so what the user typed keeps
//! its look. Text inside quotes and inside `@{…}` filters is never touched. A reference to a line
//! that was deleted becomes `#REF!`, which the calculator shows as an error.

use std::cmp::Ordering;

use crate::dims::DimVector;
use crate::error::{Error, Result};
use crate::format::{Axis, MAX_COLS, MAX_ROWS, Rect, Shift};
use crate::maix::{Cell, Maix, Sheet, Value};

/// Most rows one sort moves.
const MAX_SORT_ROWS: u32 = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellRef {
    col: u32,
    row: u32,
    col_abs: bool,
    row_abs: bool,
}

enum Piece {
    Text(String),
    Cell(CellRef),
    Range(CellRef, CellRef),
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// A cell reference starting at `i`: its value and the index after it.
fn parse_ref(chars: &[char], i: usize) -> Option<(CellRef, usize)> {
    let mut j = i;
    let col_abs = chars.get(j) == Some(&'$');
    if col_abs {
        j += 1;
    }
    let letters_start = j;
    while chars.get(j).is_some_and(char::is_ascii_alphabetic) {
        j += 1;
    }
    let letters = j - letters_start;
    if letters == 0 || letters > 3 {
        return None;
    }
    let row_abs = chars.get(j) == Some(&'$');
    if row_abs {
        j += 1;
    }
    let digits_start = j;
    while chars.get(j).is_some_and(char::is_ascii_digit) {
        j += 1;
    }
    if j == digits_start {
        return None;
    }
    if chars.get(j).is_some_and(|c| is_ident(*c) || *c == '(') {
        return None;
    }
    let col = chars[letters_start..letters_start + letters].iter().fold(0u32, |n, c| n * 26 + (u32::from(c.to_ascii_uppercase() as u8) - u32::from(b'A') + 1)) - 1;
    let row: u32 = chars[digits_start..j].iter().collect::<String>().parse().ok()?;
    if col >= u32::from(MAX_COLS) || row == 0 || row > MAX_ROWS {
        return None;
    }
    Some((CellRef { col, row: row - 1, col_abs, row_abs }, j))
}

fn split(formula: &str) -> Vec<Piece> {
    let chars: Vec<char> = formula.chars().collect();
    let mut out: Vec<Piece> = vec![];
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            text.push(c);
            i += 1;
            while i < chars.len() {
                text.push(chars[i]);
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        text.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '@' && chars.get(i + 1) == Some(&'{') {
            while i < chars.len() {
                text.push(chars[i]);
                i += 1;
                if chars[i - 1] == '}' {
                    break;
                }
            }
            continue;
        }
        let inside_word = i > 0 && is_ident(chars[i - 1]);
        if !inside_word
            && (c == '$' || c.is_ascii_alphabetic())
            && let Some((a, end)) = parse_ref(&chars, i)
        {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            if chars.get(end) == Some(&':')
                && let Some((b, end2)) = parse_ref(&chars, end + 1)
            {
                out.push(Piece::Range(a, b));
                i = end2;
            } else {
                out.push(Piece::Cell(a));
                i = end;
            }
            continue;
        }
        text.push(c);
        i += 1;
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// `A`, `B`, … `AA` for a zero-based column.
fn col_letters(col: u32) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        out.push(b'A' + u8::try_from((n - 1) % 26).unwrap_or(0));
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn write_ref(r: CellRef, out: &mut String) {
    if r.col_abs {
        out.push('$');
    }
    out.push_str(&col_letters(r.col));
    if r.row_abs {
        out.push('$');
    }
    out.push_str(&(r.row + 1).to_string());
}

fn join(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Cell(r) => write_ref(*r, &mut out),
            Piece::Range(a, b) => {
                write_ref(*a, &mut out);
                out.push(':');
                write_ref(*b, &mut out);
            }
        }
    }
    out
}

const REF_ERROR: &str = "#REF!";

fn index_of(r: CellRef, axis: Axis) -> u32 {
    match axis {
        Axis::Rows => r.row,
        Axis::Cols => r.col,
    }
}

fn with_index(mut r: CellRef, axis: Axis, i: u32) -> CellRef {
    match axis {
        Axis::Rows => r.row = i,
        Axis::Cols => r.col = i,
    }
    r
}

/// Rewrites the references of `formula` for rows or columns being inserted or deleted.
pub fn shift_formula(formula: &str, axis: Axis, op: Shift) -> String {
    let pieces: Vec<Piece> = split(formula)
        .into_iter()
        .map(|p| match p {
            Piece::Cell(r) => match op.map(index_of(r, axis)) {
                Some(i) if in_bounds(axis, i) => Piece::Cell(with_index(r, axis, i)),
                _ => Piece::Text(REF_ERROR.into()),
            },
            Piece::Range(a, b) => {
                let (ia, ib) = (index_of(a, axis), index_of(b, axis));
                match op.map_range(ia.min(ib), ia.max(ib)) {
                    Some((lo, hi)) if in_bounds(axis, hi) => {
                        let (na, nb) = if ia <= ib { (lo, hi) } else { (hi, lo) };
                        Piece::Range(with_index(a, axis, na), with_index(b, axis, nb))
                    }
                    _ => Piece::Text(REF_ERROR.into()),
                }
            }
            other => other,
        })
        .collect();
    join(&pieces)
}

fn in_bounds(axis: Axis, i: u32) -> bool {
    match axis {
        Axis::Rows => i < MAX_ROWS,
        Axis::Cols => i < u32::from(MAX_COLS),
    }
}

/// Moves the relative references of `formula` by `rows` and `cols` (as when it is copied to
/// another cell); `$`-fixed parts stay. A reference that would leave the sheet becomes `#REF!`.
pub fn translate_formula(formula: &str, rows: i64, cols: i64) -> String {
    if rows == 0 && cols == 0 {
        return formula.to_string();
    }
    let moved = |r: CellRef| -> Option<CellRef> {
        let row = if r.row_abs { i64::from(r.row) } else { i64::from(r.row) + rows };
        let col = if r.col_abs { i64::from(r.col) } else { i64::from(r.col) + cols };
        if row < 0 || row >= i64::from(MAX_ROWS) || col < 0 || col >= i64::from(MAX_COLS) {
            return None;
        }
        Some(CellRef { row: u32::try_from(row).ok()?, col: u32::try_from(col).ok()?, ..r })
    };
    let pieces: Vec<Piece> = split(formula)
        .into_iter()
        .map(|p| match p {
            Piece::Cell(r) => moved(r).map_or_else(|| Piece::Text(REF_ERROR.into()), Piece::Cell),
            Piece::Range(a, b) => match (moved(a), moved(b)) {
                (Some(x), Some(y)) => Piece::Range(x, y),
                _ => Piece::Text(REF_ERROR.into()),
            },
            other => other,
        })
        .collect();
    join(&pieces)
}

/// Spreadsheet order: numbers, then text (ignoring case), then booleans; empty cells last.
fn rank(v: &Value) -> u8 {
    match v {
        Value::Int(_) | Value::Float(_) | Value::Date(_) => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        Value::Null => 3,
    }
}

fn as_number(v: &Value) -> f64 {
    match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Date(d) => *d as f64,
        _ => 0.0,
    }
}

/// How two values sort (ascending). Empty cells compare last in both directions: see `sort_rows`.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    match rank(a).cmp(&rank(b)) {
        Ordering::Equal => match (a, b) {
            (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()).then_with(|| x.cmp(y)),
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Null, Value::Null) => Ordering::Equal,
            _ => as_number(a).partial_cmp(&as_number(b)).unwrap_or(Ordering::Equal),
        },
        other => other,
    }
}

impl Maix {
    /// Inserts or deletes whole rows or columns of a sheet: cells, formulas (here and in the
    /// cells that point at them), styles, sizes and hidden lines all move. Inserting is refused
    /// when it would push a filled cell off the end of the sheet.
    pub fn shift_lines(&mut self, sheet: u16, axis: Axis, op: Shift) -> Result<()> {
        let (Shift::Insert { count, .. } | Shift::Delete { count, .. }) = op;
        if count == 0 {
            return Ok(());
        }
        let limit = match axis {
            Axis::Rows => u64::from(MAX_ROWS),
            Axis::Cols => u64::from(MAX_COLS),
        };
        let sh = self.sheet_mut(sheet)?;
        let idx = |r: u32, c: u16| match axis {
            Axis::Rows => r,
            Axis::Cols => u32::from(c),
        };
        if let Shift::Insert { at, count } = op {
            let last = sh.cells.iter().filter(|(_, cell)| cell.value != Value::Null || cell.formula.is_some()).map(|((r, c, _), _)| idx(*r, *c)).filter(|i| *i >= at).max();
            if last.is_some_and(|l| u64::from(l) + u64::from(count) >= limit) {
                return Err(Error::Limit("the sheet has no room for that many more rows or columns".into()));
            }
        }
        let old = std::mem::take(&mut sh.cells);
        for ((r, c, d), mut cell) in old {
            let Some(n) = op.map(idx(r, c)) else { continue };
            let key = match axis {
                Axis::Rows => (n, c, d),
                Axis::Cols => match u16::try_from(n) {
                    Ok(nc) => (r, nc, d),
                    Err(_) => continue,
                },
            };
            if let Some(f) = &cell.formula {
                cell.formula = Some(shift_formula(f, axis, op));
            }
            sh.cells.insert(key, cell);
        }
        sh.format.shift(axis, op);
        if axis == Axis::Cols {
            let columns = std::mem::take(&mut sh.columns);
            for (c, meta) in columns {
                if let Some(n) = op.map(u32::from(c)).and_then(|n| u16::try_from(n).ok()) {
                    sh.columns.insert(n, meta);
                }
            }
        }
        Ok(())
    }

    /// Sorts the rows of `range` by the values in column `key_col` (as seen at `dims`). Whole
    /// rows of the range move: every dimension's cell, its formula and its style. Empty keys stay
    /// at the bottom whichever way the sort goes; equal keys keep their order. Returns how many
    /// rows moved.
    pub fn sort_rows(&mut self, sheet: u16, range: Rect, key_col: u16, ascending: bool, dims: &DimVector) -> Result<usize> {
        let (top, left, bottom, right) = range;
        if bottom < top || right < left {
            return Ok(0);
        }
        if bottom - top >= MAX_SORT_ROWS {
            return Err(Error::Limit("too many rows to sort at once".into()));
        }
        let dim = self.vectors.find(dims);
        let sh = self.sheet_mut(sheet)?;
        let n = usize::try_from(bottom - top).unwrap_or(0) + 1;
        let key_of = |sh: &Sheet, i: usize| -> Value {
            let row = top + u32::try_from(i).unwrap_or(0);
            dim.and_then(|d| sh.get(row, key_col, d)).map_or(Value::Null, |c| c.value.clone())
        };
        let keys: Vec<Value> = (0..n).map(|i| key_of(sh, i)).collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&keys[a], &keys[b]);
            match (x == &Value::Null, y == &Value::Null) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    let o = compare_values(x, y);
                    if ascending { o } else { o.reverse() }
                }
            }
        });
        let mut new_row_of = vec![0u32; n];
        for (new_pos, &old) in order.iter().enumerate() {
            new_row_of[old] = top + u32::try_from(new_pos).unwrap_or(0);
        }
        let moved = order.iter().enumerate().filter(|(new_pos, old)| new_pos != *old).count();
        if moved == 0 {
            return Ok(0);
        }
        let in_range: Vec<(u32, u16, u64)> = sh.cells.range((top, 0, 0)..=(bottom, u16::MAX, u64::MAX)).filter(|((_, c, _), _)| *c >= left && *c <= right).map(|(k, _)| *k).collect();
        let mut taken: Vec<((u32, u16, u64), Cell)> = vec![];
        for k in in_range {
            if let Some(cell) = sh.cells.remove(&k) {
                taken.push((k, cell));
            }
        }
        for ((r, c, d), mut cell) in taken {
            let new_row = new_row_of[usize::try_from(r - top).unwrap_or(0)];
            if let Some(f) = &cell.formula {
                cell.formula = Some(translate_formula(f, i64::from(new_row) - i64::from(r), 0));
            }
            sh.cells.insert((new_row, c, d), cell);
        }
        sh.format.permute_rows(top, left, right, &new_row_of);
        Ok(moved)
    }

    /// Takes a sheet out of the store (its id is gone).
    pub fn delete_sheet(&mut self, id: u16) -> bool {
        let before = self.sheets.len();
        self.sheets.retain(|s| s.id != id);
        self.sheets.len() != before
    }

    /// Moves a sheet tab left (`-1`) or right (`1`).
    pub fn move_sheet(&mut self, id: u16, delta: i32) -> bool {
        let Some(i) = self.sheets.iter().position(|s| s.id == id) else {
            return false;
        };
        let to = i64::try_from(i).unwrap_or(0) + i64::from(delta);
        let Ok(to) = usize::try_from(to) else {
            return false;
        };
        if to >= self.sheets.len() || to == i {
            return false;
        }
        let sheet = self.sheets.remove(i);
        self.sheets.insert(to, sheet);
        true
    }

    /// A copy of a sheet under a new id and a name like `Costs (2)`. The change history is not copied.
    pub fn duplicate_sheet(&mut self, id: u16) -> Option<u16> {
        let mut copy = self.sheet(id).ok()?.clone();
        let new_id = self.sheets.iter().map(|s| s.id).max().map_or(1, |m| m.saturating_add(1));
        let base = copy.name.clone();
        let mut n = 2;
        while self.sheet_by_name(&format!("{base} ({n})")).is_some() {
            n += 1;
        }
        copy.id = new_id;
        copy.name = format!("{base} ({n})");
        let at = self.sheets.iter().position(|s| s.id == id).map_or(self.sheets.len(), |i| i + 1);
        self.sheets.insert(at, copy);
        Some(new_id)
    }

    /// Swaps a sheet for another with the same id and returns the old one (for undo).
    pub fn replace_sheet(&mut self, sheet: Sheet) -> Option<Sheet> {
        let slot = self.sheets.iter_mut().find(|s| s.id == sheet.id)?;
        Some(std::mem::replace(slot, sheet))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maix::Stamp;

    fn put(m: &mut Maix, s: u16, r: u32, c: u16, v: Value) {
        m.set(s, r, c, &DimVector::new(), v, &Stamp::new("t", 1)).unwrap();
    }

    fn get(m: &Maix, s: u16, r: u32, c: u16) -> Value {
        m.get(s, r, c, &DimVector::new()).unwrap().cloned().unwrap_or(Value::Null)
    }

    #[test]
    fn insert_moves_references_below_it() {
        let ins = Shift::Insert { at: 2, count: 1 };
        assert_eq!(shift_formula("=A1+A3", Axis::Rows, ins), "=A1+A4");
        assert_eq!(shift_formula("=SUM(A1:A3)", Axis::Rows, ins), "=SUM(A1:A4)");
        assert_eq!(shift_formula("=$B$5*2", Axis::Rows, ins), "=$B$6*2");
        assert_eq!(shift_formula("=C3", Axis::Cols, Shift::Insert { at: 0, count: 2 }), "=E3");
    }

    #[test]
    fn delete_shrinks_ranges_and_breaks_lost_cells() {
        let del = Shift::Delete { at: 1, count: 1 };
        assert_eq!(shift_formula("=SUM(A1:A4)", Axis::Rows, del), "=SUM(A1:A3)");
        assert_eq!(shift_formula("=A2+A3", Axis::Rows, del), "=#REF!+A2");
        assert_eq!(shift_formula("=SUM(A2:A2)", Axis::Rows, del), "=SUM(#REF!)");
    }

    #[test]
    fn text_filters_and_function_names_are_left_alone() {
        let ins = Shift::Insert { at: 0, count: 1 };
        assert_eq!(shift_formula("=\"A1\"&B1", Axis::Rows, ins), "=\"A1\"&B2");
        assert_eq!(shift_formula("=B1@{scenario=A1}", Axis::Rows, ins), "=B2@{scenario=A1}");
        assert_eq!(shift_formula("=LOG10(A1)+DIMSUM(B1:B3,\"x=C1\")", Axis::Rows, ins), "=LOG10(A2)+DIMSUM(B2:B4,\"x=C1\")");
    }

    #[test]
    fn copying_moves_only_the_relative_parts() {
        assert_eq!(translate_formula("=A1+$B$2+C$3+$D4", 1, 1), "=B2+$B$2+D$3+$D5");
        assert_eq!(translate_formula("=SUM(A1:B2)", 2, 0), "=SUM(A3:B4)");
        assert_eq!(translate_formula("=A1", -1, 0), "=#REF!");
        assert_eq!(translate_formula("=A1", 0, 0), "=A1");
    }

    #[test]
    fn rows_shift_with_their_formulas_and_styles() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        put(&mut m, s, 0, 0, Value::Int(1));
        put(&mut m, s, 1, 0, Value::Int(2));
        m.set_formula(s, 2, 0, &DimVector::new(), Some("=A1+A2")).unwrap();
        m.recalc(&Stamp::new("t", 2));
        assert_eq!(get(&m, s, 2, 0), Value::Int(3));
        m.shift_lines(s, Axis::Rows, Shift::Insert { at: 1, count: 2 }).unwrap();
        assert_eq!(get(&m, s, 0, 0), Value::Int(1));
        assert_eq!(get(&m, s, 1, 0), Value::Null);
        assert_eq!(get(&m, s, 3, 0), Value::Int(2));
        assert_eq!(m.formula(s, 4, 0, &DimVector::new()), Some("=A1+A4"));
        m.recalc(&Stamp::new("t", 3));
        assert_eq!(get(&m, s, 4, 0), Value::Int(3));
    }

    #[test]
    fn columns_delete_and_their_cells_go() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        put(&mut m, s, 0, 0, Value::Int(1));
        put(&mut m, s, 0, 1, Value::Int(2));
        put(&mut m, s, 0, 2, Value::Int(3));
        m.shift_lines(s, Axis::Cols, Shift::Delete { at: 1, count: 1 }).unwrap();
        assert_eq!(get(&m, s, 0, 1), Value::Int(3));
        assert_eq!(get(&m, s, 0, 2), Value::Null);
    }

    #[test]
    fn inserting_is_refused_when_it_would_lose_data() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        put(&mut m, s, MAX_ROWS - 1, 0, Value::Int(1));
        assert!(m.shift_lines(s, Axis::Rows, Shift::Insert { at: 0, count: 1 }).is_err());
        assert_eq!(get(&m, s, MAX_ROWS - 1, 0), Value::Int(1));
        // Empty cells at the end do not count.
        let t = m.add_sheet("T");
        put(&mut m, t, 5, 0, Value::Int(1));
        assert!(m.shift_lines(t, Axis::Rows, Shift::Insert { at: 0, count: 1000 }).is_ok());
    }

    #[test]
    fn sorting_moves_whole_rows_and_keeps_empty_keys_last() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        for (r, (name, qty)) in [("pear", 3), ("apple", 9), ("Fig", 1)].into_iter().enumerate() {
            put(&mut m, s, r as u32, 0, Value::Text(name.into()));
            put(&mut m, s, r as u32, 1, Value::Int(qty));
        }
        put(&mut m, s, 3, 1, Value::Int(7)); // a row without a name
        let moved = m.sort_rows(s, (0, 0, 3, 1), 0, true, &DimVector::new()).unwrap();
        assert!(moved > 0);
        let names: Vec<Value> = (0..4).map(|r| get(&m, s, r, 0)).collect();
        assert_eq!(names, vec![Value::Text("apple".into()), Value::Text("Fig".into()), Value::Text("pear".into()), Value::Null]);
        assert_eq!(get(&m, s, 0, 1), Value::Int(9)); // apple's quantity travelled with it
        assert_eq!(get(&m, s, 3, 1), Value::Int(7)); // the nameless row is last
        m.sort_rows(s, (0, 0, 3, 1), 1, false, &DimVector::new()).unwrap();
        assert_eq!(get(&m, s, 0, 1), Value::Int(9));
        assert_eq!(get(&m, s, 1, 1), Value::Int(7));
        assert_eq!(m.sort_rows(s, (0, 0, 3, 1), 1, false, &DimVector::new()).unwrap(), 0);
    }

    #[test]
    fn sorting_keeps_row_relative_formulas_pointing_at_their_own_row() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        for (r, n) in [3, 1, 2].into_iter().enumerate() {
            put(&mut m, s, r as u32, 0, Value::Int(n));
            m.set_formula(s, r as u32, 1, &DimVector::new(), Some(&format!("=A{}*10", r + 1))).unwrap();
        }
        m.sort_rows(s, (0, 0, 2, 1), 0, true, &DimVector::new()).unwrap();
        m.recalc(&Stamp::new("t", 2));
        assert_eq!((get(&m, s, 0, 0), get(&m, s, 0, 1)), (Value::Int(1), Value::Int(10)));
        assert_eq!((get(&m, s, 2, 0), get(&m, s, 2, 1)), (Value::Int(3), Value::Int(30)));
    }

    #[test]
    fn values_of_different_kinds_sort_numbers_then_text_then_booleans() {
        let mut v = [Value::Bool(true), Value::Text("b".into()), Value::Int(5), Value::Float(1.5), Value::Text("A".into())];
        v.sort_by(compare_values);
        assert_eq!(v.to_vec(), vec![Value::Float(1.5), Value::Int(5), Value::Text("A".into()), Value::Text("b".into()), Value::Bool(true)]);
    }

    #[test]
    fn sheets_can_be_copied_moved_and_removed() {
        let mut m = Maix::default();
        let a = m.add_sheet("A");
        let b = m.add_sheet("B");
        put(&mut m, a, 0, 0, Value::Int(1));
        let c = m.duplicate_sheet(a).unwrap();
        assert_eq!(m.sheet(c).unwrap().name, "A (2)");
        assert_eq!(m.sheets.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, c, b]);
        assert_eq!(get(&m, c, 0, 0), Value::Int(1));
        assert!(m.move_sheet(c, 1));
        assert_eq!(m.sheets.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b, c]);
        assert!(!m.move_sheet(c, 1));
        assert!(m.delete_sheet(b));
        assert!(!m.delete_sheet(b));
        let copy = m.sheet(a).unwrap().clone();
        assert!(m.replace_sheet(copy).is_some());
    }
}
