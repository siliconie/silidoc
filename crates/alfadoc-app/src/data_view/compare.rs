//! Compare dimension vectors: the same cells under several values of one dimension, side by
//! side (actual against budget, this year against last), with the difference when two are chosen.

use std::collections::{BTreeMap, BTreeSet};

use alfadoc_bridge::textcell;
use alfadoc_model::{Maix, Value};
use egui::Align2;

use super::{Cell, DataView, Range, address, effective, number};

/// Most rows the table shows.
const MAX_ROWS_SHOWN: usize = 500;

#[derive(Clone, Debug, Default)]
pub(super) struct CompareState {
    pub open: bool,
    pub key: String,
    /// The values compared; the empty text is "the plain value (dimension not set)".
    pub picked: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CmpRow {
    pub cell: Cell,
    pub values: Vec<Value>,
}

fn number_of(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// The cells of `range` that hold something under at least one of the chosen values of `key`
/// (the other active dimensions stay as they are), with the value under each.
pub(super) fn compare_table(maix: &Maix, sheet: u16, range: Range, key: &str, values: &[String], limit: usize) -> Vec<CmpRow> {
    let Ok(sh) = maix.sheet(sheet) else { return vec![] };
    let dims: Vec<Option<u64>> = values
        .iter()
        .map(|v| {
            let mut d = maix.active_dims.clone();
            if v.is_empty() {
                d.remove(key);
            } else {
                d.insert(key.to_string(), v.clone());
            }
            maix.vectors.find(&d)
        })
        .collect();
    let range = effective(maix, sheet, range);
    let mut out = vec![];
    'rows: for r in range.0..=range.2 {
        for c in range.1..=range.3 {
            let vals: Vec<Value> = dims.iter().map(|d| d.and_then(|d| sh.get(r, c, d)).map_or(Value::Null, |cell| cell.value.clone())).collect();
            if vals.iter().any(|v| *v != Value::Null) {
                out.push(CmpRow { cell: (r, c), values: vals });
                if out.len() >= limit {
                    break 'rows;
                }
            }
        }
    }
    out
}

/// `b - a` and the change in percent, when both are numbers.
pub(super) fn difference(a: &Value, b: &Value) -> Option<(f64, Option<f64>)> {
    let (x, y) = (number_of(a)?, number_of(b)?);
    Some((y - x, if x == 0.0 { None } else { Some((y - x) / x.abs() * 100.0) }))
}

fn label(v: &str) -> String {
    if v.is_empty() { "(plain)".to_string() } else { v.to_string() }
}

/// The table as tab-separated text.
fn table_text(key: &str, values: &[String], rows: &[CmpRow]) -> String {
    let mut out = String::from("cell");
    for v in values {
        out.push('\t');
        out.push_str(&format!("{key}={}", label(v)));
    }
    if values.len() == 2 {
        out.push_str("\tdifference\tchange %");
    }
    for row in rows {
        out.push('\n');
        out.push_str(&address(row.cell));
        for v in &row.values {
            out.push('\t');
            out.push_str(&textcell::to_text(v));
        }
        if values.len() == 2 {
            match difference(&row.values[0], &row.values[1]) {
                Some((d, p)) => out.push_str(&format!("\t{}\t{}", number(d), p.map_or(String::new(), number))),
                None => out.push_str("\t\t"),
            }
        }
    }
    out
}

impl DataView {
    pub(super) fn open_compare(&mut self, maix: &Maix) {
        self.compare.open = true;
        if self.compare.key.is_empty() {
            let first = maix.vectors.iter().flat_map(|(_, v)| v.keys().cloned().collect::<Vec<_>>()).next();
            self.compare.key = first.or_else(|| maix.active_dims.keys().next().cloned()).unwrap_or_default();
            self.compare.picked.clear();
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn compare_window(&mut self, ctx: &egui::Context, maix: &Maix, sheet: u16) {
        if !self.compare.open {
            return;
        }
        let mut st = self.compare.clone();
        let mut known: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (_, vector) in maix.vectors.iter() {
            for (k, v) in vector {
                known.entry(k.clone()).or_default().insert(v.clone());
            }
        }
        let range = self.sel();
        let mut open = true;
        let mut copy: Option<String> = None;
        egui::Window::new("Compare vectors").open(&mut open).collapsible(false).default_size([560.0, 380.0]).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if known.is_empty() {
                ui.label("No dimensions yet. Add one in the Dimensions menu (for example scenario = actual) and type values under it.");
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Dimension");
                egui::ComboBox::from_id_salt("compare_key").selected_text(&st.key).show_ui(ui, |ui| {
                    for k in known.keys() {
                        if ui.selectable_label(st.key == *k, k).clicked() {
                            st.key = k.clone();
                            st.picked.clear();
                        }
                    }
                });
            });
            let values: Vec<String> = known.get(&st.key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label("Compare");
                for v in std::iter::once(String::new()).chain(values) {
                    let mut on = st.picked.contains(&v);
                    if ui.checkbox(&mut on, label(&v)).changed() {
                        if on {
                            st.picked.insert(v.clone());
                        } else {
                            st.picked.remove(&v);
                        }
                    }
                }
            });
            let chosen: Vec<String> = st.picked.iter().cloned().collect();
            if chosen.len() < 2 {
                ui.weak("Tick two or more values to compare the selected cells under each.");
                return;
            }
            let rows = compare_table(maix, sheet, range, &st.key, &chosen, MAX_ROWS_SHOWN);
            ui.horizontal(|ui| {
                ui.weak(format!("{} cell(s) in the selection hold something", rows.len()));
                if ui.button("Copy table").clicked() {
                    copy = Some(table_text(&st.key, &chosen, &rows));
                }
            });
            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("compare_grid").striped(true).spacing([14.0, 3.0]).show(ui, |ui| {
                    ui.strong("Cell");
                    for v in &chosen {
                        ui.strong(label(v));
                    }
                    if chosen.len() == 2 {
                        ui.strong("Difference");
                        ui.strong("Change %");
                    }
                    ui.end_row();
                    for row in &rows {
                        ui.monospace(address(row.cell));
                        for v in &row.values {
                            ui.label(textcell::to_text(v));
                        }
                        if chosen.len() == 2 {
                            match difference(&row.values[0], &row.values[1]) {
                                Some((d, p)) => {
                                    ui.label(number(d));
                                    ui.label(p.map_or(String::new(), |p| format!("{}%", number(p))));
                                }
                                None => {
                                    ui.label("");
                                    ui.label("");
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        });
        st.open = open;
        self.compare = st;
        if let Some(text) = copy {
            ctx.copy_text(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::{DimVector, Stamp};

    fn dims(v: &str) -> DimVector {
        let mut d = DimVector::new();
        if !v.is_empty() {
            d.insert("scenario".into(), v.into());
        }
        d
    }

    #[test]
    fn the_same_cells_are_read_under_each_value() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        let by = Stamp::new("t", 1);
        m.set(s, 0, 0, &dims("actual"), Value::Int(80), &by).unwrap();
        m.set(s, 0, 0, &dims("budget"), Value::Int(100), &by).unwrap();
        m.set(s, 1, 0, &dims("actual"), Value::Int(5), &by).unwrap();
        m.set(s, 3, 0, &dims(""), Value::Int(9), &by).unwrap();
        let values = vec!["actual".to_string(), "budget".to_string()];
        let rows = compare_table(&m, s, (0, 0, 5, 0), "scenario", &values, 100);
        assert_eq!(rows, vec![CmpRow { cell: (0, 0), values: vec![Value::Int(80), Value::Int(100)] }, CmpRow { cell: (1, 0), values: vec![Value::Int(5), Value::Null] }]);
        let with_plain = vec![String::new(), "budget".to_string()];
        let rows = compare_table(&m, s, (0, 0, 5, 0), "scenario", &with_plain, 100);
        assert_eq!(rows.iter().map(|r| r.cell).collect::<Vec<_>>(), vec![(0, 0), (3, 0)]);
        assert_eq!(compare_table(&m, s, (0, 0, 5, 0), "scenario", &values, 1).len(), 1);
    }

    #[test]
    fn differences_need_numbers_and_a_nonzero_start() {
        assert_eq!(difference(&Value::Int(80), &Value::Int(100)), Some((20.0, Some(25.0))));
        assert_eq!(difference(&Value::Int(0), &Value::Int(4)), Some((4.0, None)));
        assert_eq!(difference(&Value::Text("a".into()), &Value::Int(1)), None);
        assert_eq!(difference(&Value::Null, &Value::Int(1)), None);
    }

    #[test]
    fn the_table_copies_as_text() {
        let rows = vec![CmpRow { cell: (0, 0), values: vec![Value::Int(80), Value::Int(100)] }];
        let t = table_text("scenario", &["actual".to_string(), "budget".to_string()], &rows);
        assert_eq!(t, "cell\tscenario=actual\tscenario=budget\tdifference\tchange %\nA1\t80\t100\t20\t25");
    }
}
