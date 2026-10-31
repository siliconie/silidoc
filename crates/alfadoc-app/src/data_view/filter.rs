//! Filters: a drop-down on each title cell that hides the rows whose value in that column is not
//! ticked. Hiding uses the sheet's hidden rows (so it saves and undoes with them); the filter's
//! own state lives in this window only. Rows hidden by hand are left alone by a filter, but are
//! shown again when the filter is removed if the filter had hidden them too.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use alfadoc_model::Maix;
use egui::{Align2, Key};

use super::{DataView, Range, cell_count, shown};

/// Most data rows a filter looks at.
const MAX_FILTER_ROWS: u32 = 200_000;
/// Most distinct values listed in a drop-down.
const MAX_LISTED: usize = 1_000;

/// A filter on one sheet: the range (its first row holds the titles) and the ticked values per column.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Filter {
    pub range: Range,
    pub allowed: BTreeMap<u16, BTreeSet<String>>,
    /// The rows this filter has hidden.
    pub hidden: BTreeSet<u32>,
}

/// The open drop-down.
#[derive(Clone, Debug)]
pub(super) struct Popup {
    pub col: u16,
    pub search: String,
    pub picked: BTreeSet<String>,
    /// Every distinct text in the column, with how many rows have it.
    pub all: Vec<(String, usize)>,
}

/// The distinct shown texts of a column below the titles (blank cells as an empty text), sorted
/// without regard to case.
pub(super) fn column_values(maix: &Maix, sheet: u16, range: Range, col: u16) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let last = range.2.min(range.0.saturating_add(MAX_FILTER_ROWS));
    for r in range.0 + 1..=last {
        *counts.entry(shown(maix, sheet, r, col)).or_insert(0) += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    out
}

/// The data rows a filter hides: those whose text in some filtered column is not ticked.
pub(super) fn rows_to_hide(maix: &Maix, sheet: u16, filter: &Filter) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let range = filter.range;
    let last = range.2.min(range.0.saturating_add(MAX_FILTER_ROWS));
    for r in range.0 + 1..=last {
        if filter.allowed.iter().any(|(col, ok)| !ok.contains(&shown(maix, sheet, r, *col))) {
            out.insert(r);
        }
    }
    out
}

impl DataView {
    pub(super) fn has_filter(&self, sheet: u16) -> bool {
        self.filters.contains_key(&sheet)
    }

    /// Puts a filter on the selection (or the used area from the active row), or takes it off.
    pub(super) fn toggle_filter(&mut self, maix: &mut Maix, sheet: u16) {
        if self.has_filter(sheet) {
            self.remove_filter(maix, sheet);
            return;
        }
        let used = self.used(maix, sheet);
        let sel = self.sel();
        let range = if cell_count(sel) == 1 { (sel.0, 0, used.0.saturating_sub(1), used.1.saturating_sub(1)) } else { (sel.0, sel.1, sel.2.min(used.0.saturating_sub(1)), sel.3.min(used.1.saturating_sub(1))) };
        if range.2 <= range.0 || range.3 < range.1 {
            self.say("Select the titles and the rows under them (at least two rows) to filter.");
            return;
        }
        if range.2 - range.0 > MAX_FILTER_ROWS {
            self.say("Too many rows to filter.");
            return;
        }
        self.filters.insert(sheet, Filter { range, ..Filter::default() });
    }

    pub(super) fn remove_filter(&mut self, maix: &mut Maix, sheet: u16) {
        if let Some(f) = self.filters.remove(&sheet) {
            self.edit_format(maix, sheet, |fmt| {
                for r in &f.hidden {
                    fmt.layout.hidden_rows.remove(r);
                }
            });
        }
        self.filter_popup = None;
    }

    /// Hides and shows rows to match the filter's ticked values.
    pub(super) fn apply_filter(&mut self, maix: &mut Maix, sheet: u16) {
        let Some(filter) = self.filters.get(&sheet).cloned() else { return };
        let new = rows_to_hide(maix, sheet, &filter);
        let old = filter.hidden;
        self.edit_format(maix, sheet, |fmt| {
            for r in &old {
                fmt.layout.hidden_rows.remove(r);
            }
            fmt.layout.hidden_rows.extend(new.iter().copied());
        });
        if let Some(f) = self.filters.get_mut(&sheet) {
            f.hidden = new;
        }
    }

    pub(super) fn open_filter_popup(&mut self, maix: &Maix, sheet: u16, col: u16) {
        let Some(f) = self.filters.get(&sheet) else { return };
        let all = column_values(maix, sheet, f.range, col);
        let picked = match f.allowed.get(&col) {
            Some(ok) => ok.clone(),
            None => all.iter().map(|(t, _)| t.clone()).collect(),
        };
        self.filter_popup = Some(Popup { col, search: String::new(), picked, all });
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn filter_window(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16) {
        let Some(mut popup) = self.filter_popup.take() else { return };
        let Some(range) = self.filters.get(&sheet).map(|f| f.range) else { return };
        let (mut ok, mut cancel, mut clear, mut sort) = (false, false, false, None::<bool>);
        egui::Window::new(format!("Filter column {}", super::column_name(popup.col))).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Sort A → Z").clicked() {
                    sort = Some(true);
                }
                if ui.button("Sort Z → A").clicked() {
                    sort = Some(false);
                }
            });
            ui.separator();
            let r = ui.add(egui::TextEdit::singleline(&mut popup.search).hint_text("Search").desired_width(260.0));
            let _ = r;
            let needle = popup.search.to_lowercase();
            let listed: Vec<(String, usize)> = popup.all.iter().filter(|(t, _)| needle.is_empty() || t.to_lowercase().contains(&needle)).take(MAX_LISTED).cloned().collect();
            ui.horizontal(|ui| {
                if ui.small_button("Select all shown").clicked() {
                    popup.picked.extend(listed.iter().map(|(t, _)| t.clone()));
                }
                if ui.small_button("Clear shown").clicked() {
                    for (t, _) in &listed {
                        popup.picked.remove(t);
                    }
                }
            });
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for (text, n) in &listed {
                    let mut on = popup.picked.contains(text);
                    let label = if text.is_empty() { format!("(Blanks)  {n}") } else { format!("{text}  {n}") };
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            popup.picked.insert(text.clone());
                        } else {
                            popup.picked.remove(text);
                        }
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    ok = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
                if ui.button("Clear filter on this column").clicked() {
                    clear = true;
                }
            });
        });
        if cancel {
            return;
        }
        if let Some(ascending) = sort {
            self.sort_rows_in(maix, sheet, (range.0 + 1, range.1, range.2, range.3), popup.col, ascending);
            self.apply_filter(maix, sheet);
            self.filter_popup = Some(popup);
            return;
        }
        if clear || ok {
            let everything = popup.picked.len() == popup.all.len();
            if let Some(f) = self.filters.get_mut(&sheet) {
                if clear || everything {
                    f.allowed.remove(&popup.col);
                } else {
                    f.allowed.insert(popup.col, popup.picked);
                }
            }
            self.apply_filter(maix, sheet);
            return;
        }
        self.filter_popup = Some(popup);
    }

    /// The columns of the sheet's filter that have something ticked off.
    pub(super) fn filter_view(&self, sheet: u16) -> Option<(Range, BTreeSet<u16>)> {
        self.filters.get(&sheet).map(|f| (f.range, f.allowed.keys().copied().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::{DimVector, Stamp, Value};

    fn sheet() -> (Maix, u16) {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        let none = DimVector::new();
        let by = Stamp::new("t", 1);
        for (r, (name, qty)) in [("name", 0), ("apple", 3), ("pear", 5), ("apple", 7)].into_iter().enumerate() {
            let r = r as u32;
            m.set(s, r, 0, &none, Value::Text(name.into()), &by).unwrap();
            if r > 0 {
                m.set(s, r, 1, &none, Value::Int(qty), &by).unwrap();
            }
        }
        (m, s)
    }

    #[test]
    fn a_column_lists_its_distinct_values_with_counts() {
        let (m, s) = sheet();
        assert_eq!(column_values(&m, s, (0, 0, 3, 1), 0), vec![("apple".to_string(), 2), ("pear".to_string(), 1)]);
    }

    #[test]
    fn unticked_values_hide_their_rows_and_titles_stay() {
        let (m, s) = sheet();
        let mut f = Filter { range: (0, 0, 3, 1), ..Filter::default() };
        f.allowed.insert(0, ["pear".to_string()].into_iter().collect());
        assert_eq!(rows_to_hide(&m, s, &f).into_iter().collect::<Vec<_>>(), vec![1, 3]);
        f.allowed.insert(1, ["5".to_string()].into_iter().collect());
        assert_eq!(rows_to_hide(&m, s, &f).into_iter().collect::<Vec<_>>(), vec![1, 3]);
        f.allowed.insert(1, ["3".to_string()].into_iter().collect());
        assert_eq!(rows_to_hide(&m, s, &f).len(), 3);
    }

    #[test]
    fn applying_and_removing_a_filter_changes_hidden_rows_and_undoes() {
        let (mut m, s) = sheet();
        let mut v = DataView::default();
        v.sheet = Some(s);
        v.anchor = (0, 0);
        v.focus = (0, 0);
        v.toggle_filter(&mut m, s);
        assert!(v.has_filter(s));
        v.filters.get_mut(&s).unwrap().allowed.insert(0, ["pear".to_string()].into_iter().collect());
        v.apply_filter(&mut m, s);
        let hidden = |m: &Maix| m.sheet(s).unwrap().format.layout.hidden_rows.iter().copied().collect::<Vec<_>>();
        assert_eq!(hidden(&m), vec![1, 3]);
        v.remove_filter(&mut m, s);
        assert!(hidden(&m).is_empty());
        assert!(!v.has_filter(s));
        // Each change is an undo step.
        let step = v.steps.remove(0);
        v.run_step(&mut m, &step, true, 1);
        assert!(hidden(&m).is_empty());
    }
}
