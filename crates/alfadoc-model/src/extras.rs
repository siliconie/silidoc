//! The rest of a sheet's look and rules: merged cells, conditional formatting, data validation,
//! comments and row/column groups. They live beside the styles in the sheet's `format` record,
//! move with inserted and deleted lines, and are part of the same undo step as any other format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::format::{Axis, MAX_COLS, MAX_ROWS, Rect, Shift};
use crate::maix::Value;

/// Deepest group nesting.
pub const MAX_LEVEL: u8 = 7;
/// Most merged areas, conditional rules and validations one sheet keeps.
pub const MAX_MERGES: usize = 5_000;
pub const MAX_RULES: usize = 200;
pub const MAX_VALIDATIONS: usize = 500;
pub const MAX_COMMENT: usize = 4_000;

fn meet(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

fn within(r: Rect, row: u32, col: u16) -> bool {
    row >= r.0 && row <= r.2 && col >= r.1 && col <= r.3
}

fn norm(r: Rect) -> Rect {
    (r.0.min(r.2), r.1.min(r.3), r.0.max(r.2), r.1.max(r.3))
}

fn number_of(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Date(d) => d.to_string(),
    }
}

// ---- conditional formatting

/// When a conditional rule applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CondKind {
    Greater { value: f64 },
    Less { value: f64 },
    Equal { value: f64 },
    Between { low: f64, high: f64 },
    TextContains { text: String },
    Blank,
    NotBlank,
    /// Blends from `low` (the smallest number in the range) to `high` (the biggest) as fill.
    ColorScale { low: u32, high: u32 },
}

/// A rule: cells of `range` that fit `kind` get this fill, text colour and weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CondRule {
    pub range: Rect,
    pub kind: CondKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
}

impl CondRule {
    /// Whether a (non-scale) rule applies to a value. A colour scale never "matches"; see
    /// [`Extras::look`].
    pub fn matches(&self, v: &Value) -> bool {
        match &self.kind {
            CondKind::Blank => *v == Value::Null || text_of(v).is_empty(),
            CondKind::NotBlank => *v != Value::Null && !text_of(v).is_empty(),
            CondKind::Greater { value } => number_of(v).is_some_and(|x| x > *value),
            CondKind::Less { value } => number_of(v).is_some_and(|x| x < *value),
            CondKind::Equal { value } => number_of(v).is_some_and(|x| (x - *value).abs() < f64::EPSILON * value.abs().max(1.0)),
            CondKind::Between { low, high } => number_of(v).is_some_and(|x| x >= low.min(*high) && x <= low.max(*high)),
            CondKind::TextContains { text } => !text.is_empty() && matches!(v, Value::Text(_)) && text_of(v).to_lowercase().contains(&text.to_lowercase()),
            CondKind::ColorScale { .. } => false,
        }
    }

    pub fn is_scale(&self) -> bool {
        matches!(self.kind, CondKind::ColorScale { .. })
    }
}

/// `low` to `high` at `t` (0 to 1), as `0xRRGGBB`.
pub fn blend(low: u32, high: u32, t: f64) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |shift: u32| {
        let (a, b) = (f64::from((low >> shift) & 0xff), f64::from((high >> shift) & 0xff));
        ((a + (b - a) * t).round() as u32) & 0xff
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// What the conditional rules make of one cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub fill: Option<u32>,
    pub color: Option<u32>,
    pub bold: bool,
}

// ---- data validation

/// What a validated cell accepts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValidKind {
    /// One of these texts (also offered in a drop-down).
    List { items: Vec<String> },
    /// A whole number, within the limits that are set.
    Whole { min: Option<i64>, max: Option<i64> },
    /// Any number, within the limits that are set.
    Decimal { min: Option<f64>, max: Option<f64> },
    /// Text of this many characters.
    TextLength { min: Option<u32>, max: Option<u32> },
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    pub range: Rect,
    pub kind: ValidKind,
    /// Shown instead of the standard sentence when a value is refused.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default = "yes")]
    pub allow_blank: bool,
}

fn limits<T: PartialOrd + std::fmt::Display + Copy>(min: Option<T>, max: Option<T>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!("between {a} and {b}"),
        (Some(a), None) => format!("{a} or more"),
        (None, Some(b)) => format!("{b} or less"),
        (None, None) => "anything".to_string(),
    }
}

impl Validation {
    /// The sentence shown when a value is refused.
    pub fn describe(&self) -> String {
        if !self.message.is_empty() {
            return self.message.clone();
        }
        match &self.kind {
            ValidKind::List { items } => format!("Choose one of: {}.", items.join(", ")),
            ValidKind::Whole { min, max } => format!("Enter a whole number {}.", limits(*min, *max)),
            ValidKind::Decimal { min, max } => format!("Enter a number {}.", limits(*min, *max)),
            ValidKind::TextLength { min, max } => format!("Enter text of {} characters.", limits(*min, *max)),
        }
    }

    /// `Ok` when the cell may hold `v`; otherwise the sentence to show.
    pub fn check(&self, v: &Value) -> Result<(), String> {
        if *v == Value::Null || matches!(v, Value::Text(t) if t.is_empty()) {
            return if self.allow_blank { Ok(()) } else { Err(self.describe()) };
        }
        let ok = match &self.kind {
            ValidKind::List { items } => {
                let t = text_of(v);
                items.iter().any(|i| *i == t)
            }
            ValidKind::Whole { min, max } => number_of(v).is_some_and(|x| x.fract() == 0.0 && min.is_none_or(|m| x >= m as f64) && max.is_none_or(|m| x <= m as f64)),
            ValidKind::Decimal { min, max } => number_of(v).is_some_and(|x| min.is_none_or(|m| x >= m) && max.is_none_or(|m| x <= m)),
            ValidKind::TextLength { min, max } => {
                let n = u32::try_from(text_of(v).chars().count()).unwrap_or(u32::MAX);
                min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m)
            }
        };
        if ok { Ok(()) } else { Err(self.describe()) }
    }
}

// ---- comments

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub row: u32,
    pub col: u16,
    pub text: String,
}

// ---- everything together

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Extras {
    /// Merged areas; the top-left cell holds the content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<CondRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validations: Vec<Validation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// Group depth per row and column (absent: not grouped).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub row_levels: BTreeMap<u32, u8>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub col_levels: BTreeMap<u16, u8>,
}

impl Extras {
    pub fn is_empty(&self) -> bool {
        *self == Extras::default()
    }

    // merged cells

    /// Merges `range` (two cells or more). Merged areas it touches are replaced. False when the
    /// range is a single cell or there are too many merges.
    pub fn merge(&mut self, range: Rect) -> bool {
        let range = norm(range);
        if range.0 == range.2 && range.1 == range.3 {
            return false;
        }
        self.merges.retain(|m| !meet(*m, range));
        if self.merges.len() >= MAX_MERGES {
            return false;
        }
        self.merges.push(range);
        self.merges.sort_unstable();
        true
    }

    /// Splits the merged areas that touch `range`; returns how many.
    pub fn unmerge(&mut self, range: Rect) -> usize {
        let range = norm(range);
        let before = self.merges.len();
        self.merges.retain(|m| !meet(*m, range));
        before - self.merges.len()
    }

    /// The merged area containing the cell.
    pub fn merge_at(&self, row: u32, col: u16) -> Option<Rect> {
        self.merges.iter().copied().find(|m| within(*m, row, col))
    }

    /// True for a cell hidden under a merge (not its top-left one).
    pub fn covered(&self, row: u32, col: u16) -> bool {
        self.merge_at(row, col).is_some_and(|m| (m.0, m.1) != (row, col))
    }

    /// Whether any merge touches `range`.
    pub fn merges_touch(&self, range: Rect) -> bool {
        let range = norm(range);
        self.merges.iter().any(|m| meet(*m, range))
    }

    // conditional formatting

    /// Adds a rule; false when there are too many.
    pub fn add_rule(&mut self, rule: CondRule) -> bool {
        if self.rules.len() >= MAX_RULES {
            return false;
        }
        self.rules.push(CondRule { range: norm(rule.range), ..rule });
        true
    }

    pub fn remove_rule(&mut self, index: usize) -> bool {
        if index < self.rules.len() {
            self.rules.remove(index);
            true
        } else {
            false
        }
    }

    /// What the rules make of a cell. Earlier rules win where two set the same thing.
    /// `scale(i)` is the (smallest, biggest) number of rule `i`'s range, for colour scales.
    pub fn look(&self, row: u32, col: u16, v: &Value, scale: &dyn Fn(usize) -> Option<(f64, f64)>) -> Look {
        let mut out = Look::default();
        for (i, rule) in self.rules.iter().enumerate() {
            if !within(rule.range, row, col) {
                continue;
            }
            let (fill, color, bold) = if let CondKind::ColorScale { low, high } = rule.kind {
                match (number_of(v), scale(i)) {
                    (Some(x), Some((lo, hi))) => (Some(blend(low, high, if hi > lo { (x - lo) / (hi - lo) } else { 0.0 })), None, false),
                    _ => continue,
                }
            } else if rule.matches(v) {
                (rule.fill, rule.color, rule.bold)
            } else {
                continue;
            };
            out.fill = out.fill.or(fill);
            out.color = out.color.or(color);
            out.bold |= bold;
        }
        out
    }

    pub fn has_scale(&self) -> bool {
        self.rules.iter().any(CondRule::is_scale)
    }

    // data validation

    pub fn validation_at(&self, row: u32, col: u16) -> Option<&Validation> {
        self.validations.iter().rev().find(|v| within(v.range, row, col))
    }

    /// Replaces the validation of `range` (what it overlapped goes); `None` only removes.
    pub fn set_validation(&mut self, range: Rect, rule: Option<Validation>) -> bool {
        let range = norm(range);
        self.validations.retain(|v| !meet(v.range, range));
        match rule {
            Some(v) => {
                if self.validations.len() >= MAX_VALIDATIONS {
                    return false;
                }
                self.validations.push(Validation { range, ..v });
                true
            }
            None => true,
        }
    }

    // comments

    pub fn comment_at(&self, row: u32, col: u16) -> Option<&str> {
        self.comments.iter().find(|c| c.row == row && c.col == col).map(|c| c.text.as_str())
    }

    /// Sets (or, with `None` or empty text, removes) a cell's comment.
    pub fn set_comment(&mut self, row: u32, col: u16, text: Option<&str>) {
        self.comments.retain(|c| !(c.row == row && c.col == col));
        if let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) {
            self.comments.push(Comment { row, col, text: t.chars().take(MAX_COMMENT).collect() });
            self.comments.sort_by_key(|c| (c.row, c.col));
        }
    }

    // groups

    pub fn level(&self, axis: Axis, i: u32) -> u8 {
        match axis {
            Axis::Rows => self.row_levels.get(&i).copied().unwrap_or(0),
            Axis::Cols => u16::try_from(i).ok().and_then(|c| self.col_levels.get(&c).copied()).unwrap_or(0),
        }
    }

    fn set_level(&mut self, axis: Axis, i: u32, level: u8) {
        match axis {
            Axis::Rows => {
                if level == 0 { self.row_levels.remove(&i) } else { self.row_levels.insert(i, level) };
            }
            Axis::Cols => {
                if let Ok(c) = u16::try_from(i) {
                    if level == 0 { self.col_levels.remove(&c) } else { self.col_levels.insert(c, level) };
                }
            }
        }
    }

    /// Groups the lines `lo..=hi` one level deeper. False when one is already at the deepest level.
    pub fn group(&mut self, axis: Axis, lo: u32, hi: u32) -> bool {
        let (lo, hi) = (lo.min(hi), lo.max(hi).min(match axis { Axis::Rows => MAX_ROWS - 1, Axis::Cols => u32::from(MAX_COLS) - 1 }));
        if hi - lo >= 100_000 || (lo..=hi).any(|i| self.level(axis, i) >= MAX_LEVEL) {
            return false;
        }
        for i in lo..=hi {
            let l = self.level(axis, i);
            self.set_level(axis, i, l + 1);
        }
        true
    }

    /// Takes `lo..=hi` out of their innermost group.
    pub fn ungroup(&mut self, axis: Axis, lo: u32, hi: u32) {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let existing: Vec<u32> = match axis {
            Axis::Rows => self.row_levels.range(lo..=hi).map(|(k, _)| *k).collect(),
            Axis::Cols => self.col_levels.range(u16::try_from(lo).unwrap_or(u16::MAX)..=u16::try_from(hi).unwrap_or(u16::MAX)).map(|(k, _)| u32::from(*k)).collect(),
        };
        for i in existing {
            let l = self.level(axis, i);
            self.set_level(axis, i, l.saturating_sub(1));
        }
    }

    /// The first and last line of the group the line is in: the run around it of lines at its
    /// depth or deeper. `None` when the line is not grouped.
    pub fn run(&self, axis: Axis, line: u32) -> Option<(u32, u32)> {
        let l = self.level(axis, line);
        if l == 0 {
            return None;
        }
        let (mut lo, mut hi) = (line, line);
        while lo > 0 && self.level(axis, lo - 1) >= l {
            lo -= 1;
        }
        while self.level(axis, hi + 1) >= l {
            hi += 1;
        }
        Some((lo, hi))
    }

    pub fn is_grouped(&self, axis: Axis) -> bool {
        match axis {
            Axis::Rows => !self.row_levels.is_empty(),
            Axis::Cols => !self.col_levels.is_empty(),
        }
    }

    // moving with inserted and deleted lines

    fn shift_rect(r: Rect, axis: Axis, op: Shift) -> Option<Rect> {
        match axis {
            Axis::Rows => op.map_range(r.0, r.2).map(|(a, b)| (a, r.1, b, r.3)),
            Axis::Cols => {
                let (a, b) = op.map_range(u32::from(r.1), u32::from(r.3))?;
                Some((r.0, u16::try_from(a).ok()?, r.2, u16::try_from(b).ok()?))
            }
        }
    }

    pub fn shift(&mut self, axis: Axis, op: Shift) {
        self.merges = std::mem::take(&mut self.merges).into_iter().filter_map(|m| Self::shift_rect(m, axis, op)).filter(|m| m.0 != m.2 || m.1 != m.3).collect();
        for rule in &mut self.rules {
            if let Some(r) = Self::shift_rect(rule.range, axis, op) {
                rule.range = r;
            } else {
                rule.range = (1, 0, 0, 0); // empty: dropped below
            }
        }
        self.rules.retain(|r| r.range.0 <= r.range.2);
        for v in &mut self.validations {
            v.range = Self::shift_rect(v.range, axis, op).unwrap_or((1, 0, 0, 0));
        }
        self.validations.retain(|v| v.range.0 <= v.range.2);
        let old = std::mem::take(&mut self.comments);
        self.comments = old
            .into_iter()
            .filter_map(|mut c| match axis {
                Axis::Rows => op.map(c.row).map(|r| {
                    c.row = r;
                    c
                }),
                Axis::Cols => op.map(u32::from(c.col)).and_then(|x| u16::try_from(x).ok()).map(|x| {
                    c.col = x;
                    c
                }),
            })
            .collect();
        match axis {
            Axis::Rows => {
                let old = std::mem::take(&mut self.row_levels);
                self.row_levels = old.into_iter().filter_map(|(k, v)| op.map(k).map(|k| (k, v))).collect();
            }
            Axis::Cols => {
                let old = std::mem::take(&mut self.col_levels);
                self.col_levels = old.into_iter().filter_map(|(k, v)| op.map(u32::from(k)).and_then(|x| u16::try_from(x).ok()).map(|k| (k, v))).collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_replaces_what_it_touches_and_finds_covered_cells() {
        let mut e = Extras::default();
        assert!(!e.merge((1, 1, 1, 1)));
        assert!(e.merge((2, 2, 1, 1)));
        assert_eq!(e.merge_at(2, 2), Some((1, 1, 2, 2)));
        assert!(e.covered(2, 2) && !e.covered(1, 1) && !e.covered(3, 3));
        assert!(e.merge((2, 2, 4, 4)));
        assert_eq!(e.merges, vec![(2, 2, 4, 4)]);
        assert!(e.merges_touch((0, 0, 2, 2)));
        assert_eq!(e.unmerge((3, 3, 3, 3)), 1);
        assert!(e.merges.is_empty());
    }

    #[test]
    fn rules_combine_and_scales_blend() {
        let mut e = Extras::default();
        e.add_rule(CondRule { range: (0, 0, 9, 0), kind: CondKind::Greater { value: 10.0 }, fill: Some(0xFF0000), color: None, bold: false });
        e.add_rule(CondRule { range: (0, 0, 9, 0), kind: CondKind::Greater { value: 5.0 }, fill: Some(0x00FF00), color: Some(0x111111), bold: true });
        e.add_rule(CondRule { range: (0, 0, 9, 0), kind: CondKind::TextContains { text: "late".into() }, fill: Some(0xFFFF00), color: None, bold: false });
        let none = |_: usize| None;
        let big = e.look(0, 0, &Value::Int(12), &none);
        assert_eq!(big, Look { fill: Some(0xFF0000), color: Some(0x111111), bold: true });
        assert_eq!(e.look(1, 0, &Value::Int(7), &none).fill, Some(0x00FF00));
        assert_eq!(e.look(1, 0, &Value::Int(1), &none), Look::default());
        assert_eq!(e.look(1, 0, &Value::Text("Running LATE".into()), &none).fill, Some(0xFFFF00));
        assert_eq!(e.look(1, 1, &Value::Int(99), &none), Look::default(), "outside the range");
        let mut s = Extras::default();
        s.add_rule(CondRule { range: (0, 0, 9, 0), kind: CondKind::ColorScale { low: 0x000000, high: 0xFF0000 }, fill: None, color: None, bold: false });
        assert_eq!(s.look(0, 0, &Value::Int(5), &|_| Some((0.0, 10.0))).fill, Some(0x800000));
        assert_eq!(blend(0x000000, 0xFFFFFF, 2.0), 0xFFFFFF);
    }

    #[test]
    fn validation_checks_lists_numbers_and_lengths() {
        let v = |kind| Validation { range: (0, 0, 9, 0), kind, message: String::new(), allow_blank: true };
        let list = v(ValidKind::List { items: vec!["yes".into(), "no".into()] });
        assert!(list.check(&Value::Text("yes".into())).is_ok());
        assert!(list.check(&Value::Text("maybe".into())).is_err());
        assert!(list.check(&Value::Null).is_ok());
        let whole = v(ValidKind::Whole { min: Some(1), max: Some(10) });
        assert!(whole.check(&Value::Int(10)).is_ok());
        assert!(whole.check(&Value::Int(11)).is_err());
        assert!(whole.check(&Value::Float(2.5)).is_err());
        assert!(whole.check(&Value::Float(3.0)).is_ok());
        assert!(whole.check(&Value::Text("3".into())).is_err());
        let dec = v(ValidKind::Decimal { min: Some(0.0), max: None });
        assert!(dec.check(&Value::Float(0.5)).is_ok() && dec.check(&Value::Int(-1)).is_err());
        let len = v(ValidKind::TextLength { min: None, max: Some(3) });
        assert!(len.check(&Value::Text("abc".into())).is_ok() && len.check(&Value::Text("abcd".into())).is_err());
        let strict = Validation { allow_blank: false, ..list.clone() };
        assert!(strict.check(&Value::Null).is_err());
        assert_eq!(whole.describe(), "Enter a whole number between 1 and 10.");
        assert_eq!(Validation { message: "Pick a size".into(), ..whole }.describe(), "Pick a size");
    }

    #[test]
    fn comments_are_set_replaced_and_removed() {
        let mut e = Extras::default();
        e.set_comment(2, 1, Some(" check this "));
        assert_eq!(e.comment_at(2, 1), Some("check this"));
        e.set_comment(2, 1, Some("new"));
        assert_eq!(e.comments.len(), 1);
        e.set_comment(2, 1, Some("   "));
        assert!(e.comments.is_empty());
    }

    #[test]
    fn groups_nest_and_runs_are_found() {
        let mut e = Extras::default();
        assert!(e.group(Axis::Rows, 2, 6));
        assert!(e.group(Axis::Rows, 3, 4));
        assert_eq!(e.level(Axis::Rows, 3), 2);
        assert_eq!(e.run(Axis::Rows, 3), Some((3, 4)));
        assert_eq!(e.run(Axis::Rows, 5), Some((2, 6)));
        assert_eq!(e.run(Axis::Rows, 8), None);
        e.ungroup(Axis::Rows, 3, 4);
        assert_eq!(e.run(Axis::Rows, 3), Some((2, 6)));
        e.ungroup(Axis::Rows, 2, 6);
        assert!(!e.is_grouped(Axis::Rows));
        for _ in 0..MAX_LEVEL {
            assert!(e.group(Axis::Cols, 0, 0));
        }
        assert!(!e.group(Axis::Cols, 0, 0));
    }

    #[test]
    fn everything_moves_with_inserted_and_deleted_lines() {
        let mut e = Extras::default();
        e.merge((2, 1, 4, 2));
        e.add_rule(CondRule { range: (3, 0, 8, 0), kind: CondKind::Blank, fill: Some(1), color: None, bold: false });
        e.set_validation((3, 0, 3, 0), Some(Validation { range: (3, 0, 3, 0), kind: ValidKind::Whole { min: None, max: None }, message: String::new(), allow_blank: true }));
        e.set_comment(5, 3, Some("note"));
        e.group(Axis::Rows, 5, 6);
        e.shift(Axis::Rows, Shift::Insert { at: 3, count: 2 });
        assert_eq!(e.merges, vec![(2, 1, 6, 2)], "an insert inside grows the merge");
        assert_eq!(e.rules[0].range, (5, 0, 10, 0));
        assert_eq!(e.validations[0].range, (5, 0, 5, 0));
        assert_eq!(e.comment_at(7, 3), Some("note"));
        assert_eq!(e.level(Axis::Rows, 7), 1);
        e.shift(Axis::Rows, Shift::Delete { at: 5, count: 1 });
        assert!(e.validations.is_empty(), "its only row went");
        e.shift(Axis::Cols, Shift::Delete { at: 3, count: 1 });
        assert!(e.comments.is_empty());
        e.shift(Axis::Cols, Shift::Delete { at: 1, count: 2 });
        assert!(e.merges.is_empty());
    }

    #[test]
    fn extras_save_and_load() {
        let mut e = Extras::default();
        e.merge((0, 0, 1, 1));
        e.set_comment(1, 1, Some("hi"));
        e.group(Axis::Cols, 2, 3);
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Extras>(&json).unwrap(), e);
        assert_eq!(serde_json::to_string(&Extras::default()).unwrap(), "{}");
    }
}
