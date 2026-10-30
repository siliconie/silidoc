//! What the menus, the toolbar and the keys do to the sheet: formatting, copying and pasting,
//! inserting and deleting rows and columns, sizes, hiding, freezing, sorting and filling.

use alfadoc_model::format::{DEFAULT_COL_W, DEFAULT_ROW_H, MAX_COLS, MAX_ROWS, MIN_SIZE};
use alfadoc_model::{Axis, CellStyle, Maix, Shift, SheetFormat, Value};
use alfadoc_model::shift::translate_formula;
use egui::FontId;

use super::{Change, Clip, Content, DataView, MAX_BATCH, Range, Spot, Step, cell_count, clear, effective, paste, to_tsv, value_at, whole_cols, whole_rows, write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Toggle {
    Bold,
    Italic,
    Underline,
    Strike,
    Wrap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PasteKind {
    All,
    Values,
    Formulas,
    Formats,
}

fn content_at(maix: &Maix, sheet: u16, r: u32, c: u16) -> Content {
    let dims = &maix.active_dims;
    match maix.formula(sheet, r, c, dims) {
        Some(f) => Content::Formula(f.to_string()),
        None => Content::Value(value_at(maix, sheet, r, c)),
    }
}

impl DataView {
    pub(super) fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), f64::NAN));
    }

    /// Changes the sheet's format and keeps an undo step when something changed.
    pub(super) fn edit_format(&mut self, maix: &mut Maix, sheet: u16, f: impl FnOnce(&mut SheetFormat)) -> bool {
        let Ok(fmt) = maix.format_mut(sheet) else { return false };
        let before = fmt.clone();
        f(fmt);
        if *fmt == before {
            return false;
        }
        let after = Box::new(fmt.clone());
        self.steps.push(Step::Format { sheet, before: Box::new(before), after });
        true
    }

    /// Gives every cell of `range` the style `f` makes of its current one.
    pub(super) fn restyle(&mut self, maix: &mut Maix, sheet: u16, range: Range, f: &dyn Fn(&CellStyle) -> CellStyle) {
        let mut refused = false;
        self.edit_format(maix, sheet, |fmt| refused = fmt.apply(range, f) == 0);
        if refused {
            self.say("That range is too big to format cell by cell: select whole columns or rows instead.");
        }
    }

    pub(super) fn restyle_selection(&mut self, maix: &mut Maix, sheet: u16, f: &dyn Fn(&CellStyle) -> CellStyle) {
        let sel = self.sel();
        self.restyle(maix, sheet, sel, f);
    }

    /// Bold, italic… on or off, following the active cell.
    pub(super) fn toggle(&mut self, maix: &mut Maix, sheet: u16, which: Toggle) {
        let current = maix.sheet(sheet).map(|s| s.format.style_at(self.focus.0, self.focus.1)).unwrap_or_default();
        let on = !match which {
            Toggle::Bold => current.bold,
            Toggle::Italic => current.italic,
            Toggle::Underline => current.underline,
            Toggle::Strike => current.strike,
            Toggle::Wrap => current.wrap,
        };
        self.restyle_selection(maix, sheet, &move |s| {
            let mut n = s.clone();
            match which {
                Toggle::Bold => n.bold = on,
                Toggle::Italic => n.italic = on,
                Toggle::Underline => n.underline = on,
                Toggle::Strike => n.strike = on,
                Toggle::Wrap => n.wrap = on,
            }
            n
        });
    }

    // ---- clipboard

    /// Copies (or cuts) the selection: text for other programs, and the cells with their
    /// formulas and styles for pasting inside the app.
    pub(super) fn copy(&mut self, ctx: &egui::Context, maix: &Maix, sheet: u16, cut: bool) {
        let range = effective(maix, sheet, self.sel());
        if cell_count(range) > MAX_BATCH {
            self.say("That is too much to copy at once.");
            return;
        }
        let text = to_tsv(maix, sheet, range);
        let n = cell_count(range);
        let (mut contents, mut values, mut styles) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for r in range.0..=range.2 {
            for c in range.1..=range.3 {
                contents.push(content_at(maix, sheet, r, c));
                values.push(value_at(maix, sheet, r, c));
                styles.push(maix.sheet(sheet).map(|s| s.format.style_at(r, c)).unwrap_or_default());
            }
        }
        ctx.copy_text(text.clone());
        self.clip = Some(Clip { sheet, range, text, contents, values, styles, cut });
    }

    /// Pastes text that arrived from the system clipboard: our own copy keeps formulas and
    /// styles; anything else is read as tab-separated values.
    pub(super) fn paste_text(&mut self, maix: &mut Maix, sheet: u16, text: &str, now: u64, batch: &mut Vec<Change>) {
        let norm = |t: &str| t.replace('\r', "").trim_end_matches('\n').to_string();
        if let Some(clip) = self.clip.clone()
            && norm(&clip.text) == norm(text)
        {
            self.paste_clip(maix, sheet, &clip, PasteKind::All, now, batch);
            return;
        }
        let sel = self.sel();
        paste(maix, sheet, (sel.0, sel.1), sel, text, now, batch);
        let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(text).split('\n').collect();
        let width = lines.iter().map(|l| l.split('\t').count()).max().unwrap_or(1);
        if lines.len() > 1 || width > 1 {
            let bottom = sel.0.saturating_add(u32::try_from(lines.len()).unwrap_or(1) - 1).min(MAX_ROWS - 1);
            let right = sel.1.saturating_add(u16::try_from(width).unwrap_or(1) - 1).min(MAX_COLS - 1);
            self.anchor = (sel.0, sel.1);
            self.focus = (bottom, right);
        }
    }

    /// Pastes the cells copied inside the app at the selection's top-left corner.
    pub(super) fn paste_clip(&mut self, maix: &mut Maix, sheet: u16, clip: &Clip, kind: PasteKind, now: u64, batch: &mut Vec<Change>) {
        let sel = self.sel();
        let (top, left) = (sel.0, sel.1);
        let rows = clip.range.2 - clip.range.0 + 1;
        let cols = u32::from(clip.range.3 - clip.range.1) + 1;
        let (dr, dc) = (i64::from(top) - i64::from(clip.range.0), i64::from(left) - i64::from(clip.range.1));
        if clip.cut && kind == PasteKind::All {
            clear(maix, clip.sheet, clip.range, now, batch);
            let range = clip.range;
            self.edit_format(maix, clip.sheet, |f| {
                f.clear_range(range);
            });
        }
        let mut targets: Vec<(u32, u16, usize)> = vec![];
        for i in 0..rows {
            for j in 0..cols {
                let (r, c) = (top.saturating_add(i), u32::from(left) + j);
                if r < MAX_ROWS && c < u32::from(MAX_COLS) {
                    targets.push((r, u16::try_from(c).unwrap_or(0), (i * cols + j) as usize));
                }
            }
        }
        if kind != PasteKind::Formats {
            for (r, c, k) in &targets {
                let content = match (kind, clip.contents.get(*k)) {
                    (PasteKind::Values, _) => Content::Value(clip.values.get(*k).cloned().unwrap_or(Value::Null)),
                    (_, Some(Content::Formula(f))) => Content::Formula(translate_formula(f, dr, dc)),
                    (_, Some(other)) => other.clone(),
                    (_, None) => Content::Value(Value::Null),
                };
                let spot = Spot { sheet, cell: (*r, *c), dims: maix.active_dims.clone() };
                write(maix, &spot, content, now, batch);
            }
        }
        if kind == PasteKind::All || kind == PasteKind::Formats {
            self.edit_format(maix, sheet, |f| {
                for (r, c, k) in &targets {
                    if let Some(style) = clip.styles.get(*k) {
                        f.set_cell(*r, *c, style);
                    }
                }
            });
        }
        let (bottom, right) = (top.saturating_add(rows - 1).min(MAX_ROWS - 1), u16::try_from((u32::from(left) + cols - 1).min(u32::from(MAX_COLS) - 1)).unwrap_or(0));
        self.anchor = (top, left);
        self.focus = (bottom, right);
        if clip.cut && kind == PasteKind::All {
            self.clip = None;
        }
    }

    /// The Paste entries of the menus: our own clipboard, or a hint to use the keyboard for
    /// text from other programs (the system clipboard can only be read from a key press).
    pub(super) fn paste_special(&mut self, maix: &mut Maix, sheet: u16, kind: PasteKind, now: u64, batch: &mut Vec<Change>) {
        match self.clip.clone() {
            Some(clip) => self.paste_clip(maix, sheet, &clip, kind, now, batch),
            None => self.say("Nothing copied in this window. To paste from another program, press Ctrl+V."),
        }
    }

    pub(super) fn clear_formats(&mut self, maix: &mut Maix, sheet: u16) {
        let sel = self.sel();
        self.restyle(maix, sheet, sel, &|_| CellStyle::default());
    }

    // ---- rows and columns

    /// Runs a change of the sheet's shape and keeps a step that undoes it.
    fn structural<T>(&mut self, maix: &mut Maix, sheet: u16, shift: Option<(Axis, Shift)>, op: impl FnOnce(&mut Maix) -> alfadoc_model::Result<T>) -> Option<T> {
        let before = maix.sheet(sheet).ok()?.clone();
        match op(maix) {
            Ok(out) => {
                if let Ok(after) = maix.sheet(sheet)
                    && *after != before
                {
                    let after = after.clone();
                    self.steps.push(Step::Sheet { before: Box::new(before), after: Box::new(after), shift });
                    if let Some((axis, op)) = shift {
                        self.shifts.push((sheet, axis, op));
                    }
                    self.needs_calc = true;
                }
                Some(out)
            }
            Err(e) => {
                self.say(e.to_string());
                None
            }
        }
    }

    pub(super) fn insert_lines(&mut self, maix: &mut Maix, sheet: u16, axis: Axis, at: u32, count: u32) {
        let op = Shift::Insert { at, count };
        self.structural(maix, sheet, Some((axis, op)), |m| m.shift_lines(sheet, axis, op));
    }

    pub(super) fn delete_lines(&mut self, maix: &mut Maix, sheet: u16, axis: Axis, at: u32, count: u32) {
        let op = Shift::Delete { at, count };
        self.structural(maix, sheet, Some((axis, op)), |m| m.shift_lines(sheet, axis, op));
    }

    /// Insert rows (above the selection) or columns (left of it): whole columns selected insert
    /// columns, anything else rows.
    pub(super) fn insert_for_selection(&mut self, maix: &mut Maix, sheet: u16, _now: u64) {
        let sel = self.sel();
        if whole_cols(sel) && !whole_rows(sel) {
            self.insert_lines(maix, sheet, Axis::Cols, u32::from(sel.1), u32::from(sel.3 - sel.1) + 1);
        } else {
            self.insert_lines(maix, sheet, Axis::Rows, sel.0, sel.2 - sel.0 + 1);
        }
    }

    pub(super) fn delete_for_selection(&mut self, maix: &mut Maix, sheet: u16, _now: u64) {
        let sel = self.sel();
        if whole_cols(sel) && !whole_rows(sel) {
            self.delete_lines(maix, sheet, Axis::Cols, u32::from(sel.1), u32::from(sel.3 - sel.1) + 1);
        } else {
            self.delete_lines(maix, sheet, Axis::Rows, sel.0, sel.2 - sel.0 + 1);
        }
    }

    /// The columns (`true`) or rows the selection covers, as an inclusive range of indices.
    pub(super) fn selected_lines(&self, cols: bool) -> (u32, u32) {
        let sel = self.sel();
        if cols { (u32::from(sel.1), u32::from(sel.3)) } else { (sel.0, sel.2) }
    }

    pub(super) fn set_size(&mut self, maix: &mut Maix, sheet: u16, cols: bool, first: u32, last: u32, size: Option<f32>) {
        let mut refused = false;
        self.edit_format(maix, sheet, |f| {
            if cols {
                f.layout.set_col_width(u16::try_from(first).unwrap_or(0)..=u16::try_from(last).unwrap_or(0), size);
            } else if last - first >= 100_000 {
                refused = true;
            } else {
                f.layout.set_row_height(first..=last, size);
            }
        });
        if refused {
            self.say("Select fewer rows to change their height.");
        }
    }

    /// The current size of the first selected column or row (for the size dialogs).
    pub(super) fn current_size(&self, maix: &Maix, sheet: u16, cols: bool) -> f32 {
        let (first, _) = self.selected_lines(cols);
        maix.sheet(sheet).map_or(if cols { DEFAULT_COL_W } else { DEFAULT_ROW_H }, |s| {
            if cols { s.format.layout.col_widths.get(&u16::try_from(first).unwrap_or(0)).copied().unwrap_or(DEFAULT_COL_W) } else { s.format.layout.row_heights.get(&first).copied().unwrap_or(DEFAULT_ROW_H) }
        })
    }

    pub(super) fn hide_lines(&mut self, maix: &mut Maix, sheet: u16, cols: bool, first: u32, last: u32, hidden: bool) {
        self.edit_format(maix, sheet, |f| {
            if cols {
                let (a, b) = (u16::try_from(first).unwrap_or(0), u16::try_from(last).unwrap_or(0));
                if hidden {
                    f.layout.set_cols_hidden(a..=b, true);
                } else {
                    f.layout.hidden_cols.retain(|c| !(a..=b).contains(c));
                }
            } else if hidden {
                f.layout.set_rows_hidden(first..=last, true);
            } else {
                f.layout.hidden_rows.retain(|r| !(first..=last).contains(r));
            }
        });
    }

    pub(super) fn freeze(&mut self, maix: &mut Maix, sheet: u16, rows: u32, cols: u16) {
        self.edit_format(maix, sheet, |f| {
            f.layout.freeze_rows = rows.min(1000);
            f.layout.freeze_cols = cols.min(100);
        });
        self.scroll = (0.0, 0.0);
    }

    /// Fits the column or row `idx` (or all selected ones, when it is one of them) to its content.
    pub(super) fn autofit(&mut self, ui: &egui::Ui, maix: &mut Maix, sheet: u16, cols: bool, idx: u32) {
        let sel = self.sel();
        let (first, last) = if cols && whole_cols(sel) && (u32::from(sel.1)..=u32::from(sel.3)).contains(&idx) {
            (u32::from(sel.1), u32::from(sel.3))
        } else if !cols && whole_rows(sel) && (sel.0..=sel.2).contains(&idx) {
            (sel.0, sel.2)
        } else {
            (idx, idx)
        };
        let mut best: std::collections::HashMap<u32, f32> = std::collections::HashMap::new();
        if let Ok(sh) = maix.sheet(sheet) {
            let dim = maix.vectors.find(&maix.active_dims);
            let col_width = |c: u16| sh.format.layout.col_widths.get(&c).copied().unwrap_or(DEFAULT_COL_W);
            if let Some(dim) = dim {
                for (r, c, cell) in sh.slice(dim).take(200_000) {
                    let line = if cols { u32::from(c) } else { r };
                    if line < first || line > last || cell.value == Value::Null {
                        continue;
                    }
                    let style = sh.format.get(sh.format.id_at(r, c)).cloned().unwrap_or_default();
                    let text = match super::numfmt::display(&cell.value, &style.numfmt) {
                        Some(s) => s.text,
                        None => alfadoc_bridge::textcell::to_text(&cell.value),
                    };
                    let size = if style.size == 0 { 13.5 } else { f32::from(style.size) * 13.5 / 11.0 };
                    let font = FontId::proportional(size);
                    let bold = if style.bold { 1.08 } else { 1.0 };
                    let measured = if cols {
                        ui.painter().layout_no_wrap(text, font, egui::Color32::WHITE).size().x * bold + 14.0
                    } else if style.wrap {
                        ui.painter().layout(text, font, egui::Color32::WHITE, (col_width(c) - 12.0).max(10.0)).size().y + 8.0
                    } else {
                        size * 1.7 + 4.0
                    };
                    let e = best.entry(line).or_insert(0.0);
                    *e = e.max(measured);
                }
            }
        }
        let best = best;
        self.edit_format(maix, sheet, |f| {
            for line in first..=last.min(first + 20_000) {
                let size = best.get(&line).copied();
                if cols {
                    let Ok(c) = u16::try_from(line) else { continue };
                    f.layout.set_col_width(c..=c, size.map(|w| w.max(MIN_SIZE + 10.0)));
                } else {
                    f.layout.set_row_height(line..=line, size.filter(|h| *h > DEFAULT_ROW_H + 0.5));
                }
            }
        });
    }

    // ---- sorting and filling

    /// Does the first row of `range` look like column titles (text over numbers)?
    fn guess_header(maix: &Maix, sheet: u16, range: Range) -> bool {
        if range.0 >= range.2 {
            return false;
        }
        let kinds = |row: u32| (range.1..=range.3).map(move |c| value_at(maix, sheet, row, c));
        let first_all_text = kinds(range.0).filter(|v| *v != Value::Null).all(|v| matches!(v, Value::Text(_))) && kinds(range.0).any(|v| matches!(v, Value::Text(_)));
        let next_has_number = kinds(range.0 + 1).any(|v| matches!(v, Value::Int(_) | Value::Float(_) | Value::Date(_)));
        first_all_text && next_has_number
    }

    /// Sorts the rows of the selection (or of the whole used area when one cell is selected) by
    /// a column. `key` is a sheet column; `None` is the active cell's column.
    pub(super) fn sort_selection(&mut self, maix: &mut Maix, sheet: u16, ascending: bool, key: Option<u16>, header: Option<bool>) {
        let sel = self.sel();
        let used = self.used(maix, sheet);
        if used.0 == 0 {
            return;
        }
        let mut range = if cell_count(sel) == 1 { (0, 0, used.0 - 1, used.1.saturating_sub(1)) } else { (sel.0, sel.1, sel.2.min(used.0 - 1), sel.3.min(used.1.saturating_sub(1))) };
        if range.2 < range.0 || range.3 < range.1 {
            return;
        }
        let key = key.unwrap_or(self.focus.1).clamp(range.1, range.3);
        if header.unwrap_or_else(|| Self::guess_header(maix, sheet, range)) {
            range.0 += 1;
        }
        if range.0 >= range.2 {
            return;
        }
        let dims = maix.active_dims.clone();
        if let Some(moved) = self.structural(maix, sheet, None, |m| m.sort_rows(sheet, range, key, ascending, &dims)) {
            self.say(if moved == 0 { "Already in order.".to_string() } else { format!("Sorted {} rows.", range.2 - range.0 + 1) });
        }
    }

    /// Ctrl+D (`down`) and Ctrl+R: copies the first row (column) of the selection over the rest,
    /// or, for a single row (column), the one above (to the left). Relative references move.
    pub(super) fn fill(&mut self, maix: &mut Maix, sheet: u16, down: bool, now: u64, batch: &mut Vec<Change>) {
        let sel = effective(maix, sheet, self.sel());
        if cell_count(sel) > MAX_BATCH {
            return;
        }
        let (src_line, from, to) = if down {
            if sel.0 < sel.2 {
                (sel.0, sel.0 + 1, sel.2)
            } else if sel.0 > 0 {
                (sel.0 - 1, sel.0, sel.0)
            } else {
                return;
            }
        } else if sel.1 < sel.3 {
            (u32::from(sel.1), u32::from(sel.1) + 1, u32::from(sel.3))
        } else if sel.1 > 0 {
            (u32::from(sel.1) - 1, u32::from(sel.1), u32::from(sel.1))
        } else {
            return;
        };
        let mut styled: Vec<(u32, u16, CellStyle)> = vec![];
        let (outer, inner) = if down { (u32::from(sel.1)..=u32::from(sel.3), from..=to) } else { (sel.0..=sel.2, from..=to) };
        for o in outer {
            let (sr, sc) = if down { (src_line, u16::try_from(o).unwrap_or(0)) } else { (o, u16::try_from(src_line).unwrap_or(0)) };
            let content = content_at(maix, sheet, sr, sc);
            let style = maix.sheet(sheet).map(|s| s.format.style_at(sr, sc)).unwrap_or_default();
            for i in inner.clone() {
                let (r, c) = if down { (i, sc) } else { (sr, u16::try_from(i).unwrap_or(0)) };
                let (dr, dc) = (i64::from(r) - i64::from(sr), i64::from(c) - i64::from(sc));
                let c2 = match &content {
                    Content::Formula(f) => Content::Formula(translate_formula(f, dr, dc)),
                    other => other.clone(),
                };
                let spot = Spot { sheet, cell: (r, c), dims: maix.active_dims.clone() };
                write(maix, &spot, c2, now, batch);
                styled.push((r, c, style.clone()));
            }
        }
        self.edit_format(maix, sheet, |f| {
            for (r, c, s) in &styled {
                f.set_cell(*r, *c, s);
            }
        });
    }
}
