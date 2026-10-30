//! The **Data** tab: the package's shared maix store as an editable grid.
//!
//! Anything linked to a Data range (see `linking.rs`) appears here, and a value typed here flows
//! out to every sheet, document table and slide table linked to the same cells. Values are typed
//! the same way as in tables: `12` is a number, `007` stays text, `true` is a boolean.

use alfadoc_bridge::textcell;
use alfadoc_model::{DimVector, Maix, Stamp, Value};

/// Rows and columns shown at first; the buttons below the grid add more.
const START_ROWS: u32 = 40;
const START_COLS: u16 = 12;
const ROW_STEP: u32 = 40;
const MAX_ROWS: u32 = 10_000;
const MAX_COLS: u16 = 200;
/// Who the store records as the author of edits made in this tab.
const AUTHOR: &str = "alfadoc-user";

pub struct DataView {
    sheet: Option<u16>,
    rows: u32,
    cols: u16,
    /// The cell being typed into and the text so far.
    editing: Option<((u32, u16), String)>,
    new_name: String,
}

impl Default for DataView {
    fn default() -> Self {
        DataView { sheet: None, rows: START_ROWS, cols: START_COLS, editing: None, new_name: String::new() }
    }
}

/// `A`, `B`, … `Z`, `AA`, … for a zero-based column.
pub fn column_name(col: u16) -> String {
    let mut n = u32::from(col) + 1;
    let mut out = Vec::new();
    while n > 0 {
        let rem = u8::try_from((n - 1) % 26).unwrap_or(0);
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn shown(maix: &Maix, sheet: u16, row: u32, col: u16) -> String {
    match maix.get(sheet, row, col, &DimVector::new()) {
        Ok(Some(v)) if *v != Value::Null => textcell::to_text(v),
        _ => String::new(),
    }
}

impl DataView {
    /// Draws the tab. Returns true when the user changed a value.
    pub fn show(&mut self, ui: &mut egui::Ui, maix: &mut Maix, now_ms: u64) -> bool {
        let mut changed = false;
        if self.sheet.is_none_or(|id| maix.sheet(id).is_err()) {
            self.sheet = maix.sheets.first().map(|s| s.id);
        }
        ui.horizontal(|ui| {
            ui.label("Data sheet:");
            let current = self.sheet.and_then(|id| maix.sheet(id).ok()).map_or_else(|| "(none)".to_string(), |s| s.name.clone());
            egui::ComboBox::from_id_salt("data_sheet").selected_text(current).show_ui(ui, |ui| {
                for s in &maix.sheets {
                    ui.selectable_value(&mut self.sheet, Some(s.id), &s.name);
                }
            });
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.new_name).hint_text("new sheet name").desired_width(130.0));
            if ui.button("Add sheet").clicked() && !self.new_name.trim().is_empty() {
                let name = self.new_name.trim().to_string();
                self.sheet = Some(match maix.sheet_by_name(&name) {
                    Some(s) => s.id,
                    None => maix.add_sheet(&name),
                });
                self.new_name.clear();
            }
        });
        ui.separator();
        let Some(sheet) = self.sheet else {
            ui.centered_and_justified(|ui| {
                ui.label("The data store is empty. Add a sheet, or link a range to Data from the Links window.");
            });
            return false;
        };
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("data_grid").spacing([2.0, 2.0]).min_col_width(60.0).show(ui, |ui| {
                ui.label("");
                for c in 0..self.cols {
                    ui.strong(column_name(c));
                }
                ui.end_row();
                for r in 0..self.rows {
                    ui.weak((r + 1).to_string());
                    for c in 0..self.cols {
                        changed |= self.cell(ui, maix, sheet, r, c, now_ms);
                    }
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                if ui.button("More rows").clicked() {
                    self.rows = (self.rows + ROW_STEP).min(MAX_ROWS);
                }
                if ui.button("More columns").clicked() {
                    self.cols = self.cols.saturating_add(6).min(MAX_COLS);
                }
            });
        });
        changed
    }

    fn cell(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, row: u32, col: u16, now_ms: u64) -> bool {
        let key = (row, col);
        let current = shown(maix, sheet, row, col);
        let mut text = match &self.editing {
            Some((k, t)) if *k == key => t.clone(),
            _ => current.clone(),
        };
        let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(60.0));
        if response.has_focus() || response.changed() {
            self.editing = Some((key, text.clone()));
        }
        if !response.lost_focus() {
            return false;
        }
        if matches!(&self.editing, Some((k, _)) if *k == key) {
            self.editing = None;
        }
        if text == current {
            return false;
        }
        let value = if text.is_empty() { Value::Null } else { textcell::from_text(&text) };
        maix.set(sheet, row, col, &DimVector::new(), value, &Stamp::new(AUTHOR, now_ms)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_count_like_a_spreadsheet() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn shown_text_follows_the_store() {
        let mut m = Maix::default();
        let s = m.add_sheet("Costs");
        assert_eq!(shown(&m, s, 0, 0), "");
        m.set(s, 0, 0, &DimVector::new(), Value::Int(12), &Stamp::new("t", 1)).unwrap();
        m.set(s, 1, 0, &DimVector::new(), Value::Null, &Stamp::new("t", 2)).unwrap();
        assert_eq!(shown(&m, s, 0, 0), "12");
        assert_eq!(shown(&m, s, 1, 0), "");
    }
}
