//! Find and replace in the Data tab (Ctrl+F, Ctrl+H).

use alfadoc_bridge::textcell;
use alfadoc_model::{Maix, Value};
use egui::{Align2, Key};

use super::{Cell, Change, DataView, Range, cell_count, effective, put_input};

/// The Find and Replace window and what it searches for.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FindState {
    pub open: bool,
    pub find: String,
    pub replace: String,
    pub case: bool,
    /// The whole cell must equal the text.
    pub whole: bool,
    /// Search formulas (and replace inside them) instead of shown values.
    pub formulas: bool,
    pub in_selection: bool,
    pub message: String,
    /// The find box wants the keyboard on the next frame.
    pub focus_box: bool,
}

impl Default for FindState {
    fn default() -> Self {
        FindState { open: false, find: String::new(), replace: String::new(), case: false, whole: false, formulas: false, in_selection: false, message: String::new(), focus_box: false }
    }
}

enum Action {
    Next,
    Previous,
    Replace,
    ReplaceAll,
}

/// Does `text` contain `query` (or equal it, with `whole`)?
pub(super) fn matches(text: &str, query: &str, case: bool, whole: bool) -> bool {
    if query.is_empty() {
        return false;
    }
    if case {
        if whole { text == query } else { text.contains(query) }
    } else {
        let (t, q) = (text.to_lowercase(), query.to_lowercase());
        if whole { t == q } else { t.contains(&q) }
    }
}

/// `text` with every occurrence of `query` replaced (the whole text, with `whole`); `None` when
/// there is nothing to replace.
pub(super) fn replace_text(text: &str, query: &str, with: &str, case: bool, whole: bool) -> Option<String> {
    if !matches(text, query, case, whole) {
        return None;
    }
    if whole {
        return Some(with.to_string());
    }
    if case {
        return Some(text.replace(query, with));
    }
    let n = query.chars().count();
    let wanted: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut hit = false;
    while i < chars.len() {
        if i + n <= chars.len() && chars[i..i + n].iter().flat_map(|c| c.to_lowercase()).eq(wanted.iter().copied()) {
            out.push_str(with);
            i += n;
            hit = true;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    hit.then_some(out)
}

/// The text of a cell as the search sees it, and whether it is a formula.
fn searchable(cell: &alfadoc_model::maix::Cell, formulas: bool) -> Option<(String, bool)> {
    if formulas && let Some(f) = &cell.formula {
        return Some((f.clone(), true));
    }
    if cell.value == Value::Null {
        return None;
    }
    Some((textcell::to_text(&cell.value), cell.formula.is_some()))
}

/// Every cell that matches, in reading order (rows first).
pub(super) fn matching_cells(maix: &Maix, sheet: u16, st: &FindState, within: Option<Range>) -> Vec<Cell> {
    let Ok(sh) = maix.sheet(sheet) else { return vec![] };
    let Some(dim) = maix.vectors.find(&maix.active_dims) else { return vec![] };
    sh.slice(dim)
        .filter(|(r, c, _)| within.is_none_or(|w| (w.0..=w.2).contains(r) && (w.1..=w.3).contains(c)))
        .filter(|(_, _, cell)| searchable(cell, st.formulas).is_some_and(|(t, _)| matches(&t, &st.find, st.case, st.whole)))
        .map(|(r, c, _)| (r, c))
        .collect()
}

/// The match after (or, going back, before) `from`, wrapping round.
pub(super) fn next_match(cells: &[Cell], from: Cell, forward: bool) -> Option<Cell> {
    if forward {
        cells.iter().copied().find(|c| *c > from).or_else(|| cells.first().copied())
    } else {
        cells.iter().copied().rev().find(|c| *c < from).or_else(|| cells.last().copied())
    }
}

impl DataView {
    pub(super) fn open_find(&mut self) {
        self.find.open = true;
        self.find.focus_box = true;
        self.find.in_selection = cell_count(self.sel()) > 1;
        self.find.message.clear();
    }

    fn find_scope(&self, maix: &Maix, sheet: u16) -> Option<Range> {
        self.find.in_selection.then(|| effective(maix, sheet, self.sel()))
    }

    /// Replaces in the cell at `at`; true when it changed.
    fn replace_in_cell(&self, maix: &mut Maix, sheet: u16, at: Cell, now: u64, batch: &mut Vec<Change>) -> bool {
        let st = &self.find;
        let Ok(sh) = maix.sheet(sheet) else { return false };
        let Some(dim) = maix.vectors.find(&maix.active_dims) else { return false };
        let Some(cell) = sh.get(at.0, at.1, dim) else { return false };
        let Some((text, is_formula)) = searchable(cell, st.formulas) else { return false };
        if is_formula && !st.formulas {
            return false; // a computed value cannot be replaced; search formulas to change the formula
        }
        let Some(new) = replace_text(&text, &st.find, &st.replace, st.case, st.whole) else { return false };
        put_input(maix, sheet, at, &new, now, batch);
        true
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn find_window(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        if !self.find.open {
            return;
        }
        let mut st = self.find.clone();
        let mut action: Option<Action> = None;
        let mut open = true;
        egui::Window::new("Find and replace").open(&mut open).collapsible(false).resizable(false).anchor(Align2::RIGHT_TOP, [-20.0, 120.0]).show(ctx, |ui| {
            egui::Grid::new("find_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Find");
                let r = ui.add(egui::TextEdit::singleline(&mut st.find).desired_width(220.0));
                if st.focus_box {
                    r.request_focus();
                    st.focus_box = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    action = Some(if ui.input(|i| i.modifiers.shift) { Action::Previous } else { Action::Next });
                    r.request_focus();
                }
                ui.end_row();
                ui.label("Replace with");
                ui.add(egui::TextEdit::singleline(&mut st.replace).desired_width(220.0));
                ui.end_row();
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut st.case, "Match case");
                ui.checkbox(&mut st.whole, "Whole cell");
                ui.checkbox(&mut st.formulas, "Look in formulas");
                ui.checkbox(&mut st.in_selection, "Selection only");
            });
            ui.horizontal(|ui| {
                if ui.button("Previous").clicked() {
                    action = Some(Action::Previous);
                }
                if ui.button("Next").clicked() {
                    action = Some(Action::Next);
                }
                if ui.button("Replace").clicked() {
                    action = Some(Action::Replace);
                }
                if ui.button("Replace all").clicked() {
                    action = Some(Action::ReplaceAll);
                }
            });
            if !st.message.is_empty() {
                ui.weak(&st.message);
            }
        });
        st.open = open && !ctx.input(|i| i.key_pressed(Key::Escape));
        self.find = st;
        let Some(action) = action else { return };
        let within = self.find_scope(maix, sheet);
        match action {
            Action::Next | Action::Previous => self.go_to_match(maix, sheet, within, matches!(action, Action::Next)),
            Action::Replace => {
                if self.replace_in_cell(maix, sheet, self.focus, now, batch) {
                    self.find.message = "Replaced one.".into();
                }
                self.go_to_match(maix, sheet, within, true);
            }
            Action::ReplaceAll => {
                let cells = matching_cells(maix, sheet, &self.find, within);
                let mut done = 0;
                for at in cells {
                    if self.replace_in_cell(maix, sheet, at, now, batch) {
                        done += 1;
                    }
                }
                self.find.message = if done == 0 { "Nothing to replace.".into() } else { format!("Replaced {done} cell(s).") };
            }
        }
    }

    fn go_to_match(&mut self, maix: &Maix, sheet: u16, within: Option<Range>, forward: bool) {
        let cells = matching_cells(maix, sheet, &self.find, within);
        match next_match(&cells, self.focus, forward) {
            Some(cell) => {
                self.anchor = cell;
                self.focus = cell;
                self.scroll_to_focus = true;
                self.find.message = format!("{} match(es)", cells.len());
            }
            None => self.find.message = "No match.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_follows_the_options() {
        assert!(matches("Hello World", "world", false, false));
        assert!(!matches("Hello World", "world", true, false));
        assert!(matches("Hello", "hello", false, true));
        assert!(!matches("Hello World", "hello", false, true));
        assert!(!matches("anything", "", false, false));
    }

    #[test]
    fn replacing_works_with_and_without_case() {
        assert_eq!(replace_text("a-b-A", "a", "x", true, false), Some("x-b-A".into()));
        assert_eq!(replace_text("a-b-A", "a", "x", false, false), Some("x-b-x".into()));
        assert_eq!(replace_text("abc", "ABC", "z", false, true), Some("z".into()));
        assert_eq!(replace_text("abcd", "ABC", "z", false, true), None);
        assert_eq!(replace_text("nothing", "q", "z", false, false), None);
        assert_eq!(replace_text("=A1+A2", "A", "B", true, false), Some("=B1+B2".into()));
    }

    #[test]
    fn the_next_match_wraps_round() {
        let cells = [(0, 0), (2, 1), (5, 0)];
        assert_eq!(next_match(&cells, (0, 0), true), Some((2, 1)));
        assert_eq!(next_match(&cells, (5, 0), true), Some((0, 0)));
        assert_eq!(next_match(&cells, (2, 1), false), Some((0, 0)));
        assert_eq!(next_match(&cells, (0, 0), false), Some((5, 0)));
        assert_eq!(next_match(&[], (0, 0), true), None);
    }
}
