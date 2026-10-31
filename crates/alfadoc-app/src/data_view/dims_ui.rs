//! Moving between dimensions: value chips (one click, no drop-down), a dimension bar above the
//! grid, and keys that step through the values.

use std::collections::BTreeSet;

use alfadoc_model::Maix;

use super::DataView;

/// The choices of a dimension in order: the plain value (`None`) first, then each value.
fn choices(values: &BTreeSet<String>) -> Vec<Option<&str>> {
    std::iter::once(None).chain(values.iter().map(|v| Some(v.as_str()))).collect()
}

/// The choice after (or before) `current`, wrapping round.
pub(super) fn step_choice(values: &BTreeSet<String>, current: Option<&str>, forward: bool) -> Option<String> {
    let all = choices(values);
    let at = all.iter().position(|c| *c == current).unwrap_or(0);
    let next = if forward { (at + 1) % all.len() } else { (at + all.len() - 1) % all.len() };
    all.get(next).copied().flatten().map(str::to_string)
}

/// The dimension the keys act on: the one last touched, else the first.
pub(super) fn focused_key<'a>(focus: &str, keys: impl Iterator<Item = &'a String>) -> Option<String> {
    let keys: Vec<&String> = keys.collect();
    if keys.iter().any(|k| k.as_str() == focus) { Some(focus.to_string()) } else { keys.first().map(|k| (*k).clone()) }
}

/// Every choice across all dimensions in order: plain, then each value of each dimension.
pub(super) fn flat_choices(catalog: &std::collections::BTreeMap<String, BTreeSet<String>>) -> Vec<Option<(&str, &str)>> {
    std::iter::once(None).chain(catalog.iter().flat_map(|(k, vs)| vs.iter().map(move |v| Some((k.as_str(), v.as_str()))))).collect()
}

impl DataView {
    /// One dimension as a row of chips. With combining on, a "plain" chip first (the dimension
    /// left unset); each value chip. A chip in italics has no data yet under the vector it would
    /// choose.
    pub(super) fn dimension_chips(&mut self, ui: &mut egui::Ui, maix: &mut Maix, key: &str, values: &BTreeSet<String>) {
        let combine = self.combine_dims;
        let current = maix.active_dims.get(key).cloned();
        let mut pick: Option<Option<String>> = None;
        if combine && ui.selectable_label(current.is_none(), "plain").on_hover_text("Leave this dimension unset").clicked() {
            pick = Some(None);
        }
        for value in values {
            let mut candidate = if combine { maix.active_dims.clone() } else { alfadoc_model::DimVector::new() };
            candidate.insert(key.to_string(), value.clone());
            let used = maix.vectors.find(&candidate).is_some();
            let text = if used { egui::RichText::new(value) } else { egui::RichText::new(value).italics().weak() };
            let tip = if used { "Show and write this value" } else { "No cell is stored under this yet" };
            if ui.selectable_label(current.as_deref() == Some(value.as_str()), text).on_hover_text(tip).clicked() {
                pick = Some(Some(value.clone()));
            }
        }
        if let Some(choice) = pick {
            if !combine {
                maix.active_dims.clear();
            }
            match choice {
                Some(v) => maix.active_dims.insert(key.to_string(), v),
                None => maix.active_dims.remove(key),
            };
            self.dim_focus = key.to_string();
        }
    }

    /// The strip above the grid: every dimension with its values as chips, so changing the view
    /// is one click. Hidden when there are no dimensions or the View menu turned it off.
    pub(super) fn dimension_bar(&mut self, ui: &mut egui::Ui, maix: &mut Maix) {
        let catalog = maix.dimension_catalog();
        if !self.dim_bar || catalog.is_empty() {
            return;
        }
        let focus = focused_key(&self.dim_focus, catalog.keys());
        let mut compare: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(4.0);
            if !self.combine_dims {
                // One choice across all dimensions, so there is a single "plain".
                if ui.selectable_label(maix.active_dims.is_empty(), "plain").on_hover_text("The values that belong to no dimension").clicked() {
                    maix.active_dims.clear();
                }
                ui.add_space(6.0);
            }
            for (key, values) in &catalog {
                let strong = focus.as_deref() == Some(key.as_str());
                let name = if strong { egui::RichText::new(format!("{key}:")).strong() } else { egui::RichText::new(format!("{key}:")).weak() };
                if ui.label(name).on_hover_text("Ctrl+[ and Ctrl+] step through the values").clicked() {
                    self.dim_focus = key.clone();
                }
                self.dimension_chips(ui, maix, key, values);
                if values.len() > 1 && ui.small_button("⇄").on_hover_text(format!("Compare the values of {key} side by side")).clicked() {
                    compare = Some(key.clone());
                }
                ui.add_space(10.0);
            }
        });
        if let Some(key) = compare {
            self.open_compare(maix);
            self.compare.picked = catalog.get(&key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            self.compare.key = key;
        }
    }

    /// Ctrl+] / Ctrl+[ : the next / previous choice. On its own each dimension's values are
    /// separate choices (plain, then every value of every dimension, wrapping round); when
    /// dimensions are combined the keys step the focused dimension, and with Shift pick which
    /// dimension that is.
    pub(super) fn cycle_dimension(&mut self, maix: &mut Maix, forward: bool, pick_dimension: bool) {
        let catalog = maix.dimension_catalog();
        if !self.combine_dims {
            let all = flat_choices(&catalog);
            let current = match maix.active_dims.iter().next() {
                Some((k, v)) if maix.active_dims.len() == 1 => Some((k.as_str(), v.as_str())),
                _ => None,
            };
            let at = all.iter().position(|c| *c == current).unwrap_or(0);
            let next = if forward { (at + 1) % all.len() } else { (at + all.len() - 1) % all.len() };
            let chosen = all.get(next).copied().flatten().map(|(k, v)| (k.to_string(), v.to_string()));
            maix.active_dims.clear();
            if let Some((k, v)) = chosen {
                self.dim_focus = k.clone();
                maix.active_dims.insert(k, v);
            }
            return;
        }
        let Some(key) = focused_key(&self.dim_focus, catalog.keys()) else { return };
        if pick_dimension {
            let keys: Vec<&String> = catalog.keys().collect();
            let at = keys.iter().position(|k| **k == key).unwrap_or(0);
            let next = if forward { (at + 1) % keys.len() } else { (at + keys.len() - 1) % keys.len() };
            self.dim_focus = keys.get(next).map(|k| (*k).clone()).unwrap_or(key);
            return;
        }
        let Some(values) = catalog.get(&key) else { return };
        match step_choice(values, maix.active_dims.get(&key).map(String::as_str), forward) {
            Some(v) => maix.active_dims.insert(key.clone(), v),
            None => maix.active_dims.remove(&key),
        };
        self.dim_focus = key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn stepping_goes_plain_then_each_value_and_wraps() {
        let v = set(&["actual", "budget"]);
        assert_eq!(step_choice(&v, None, true).as_deref(), Some("actual"));
        assert_eq!(step_choice(&v, Some("actual"), true).as_deref(), Some("budget"));
        assert_eq!(step_choice(&v, Some("budget"), true), None, "back to plain");
        assert_eq!(step_choice(&v, None, false).as_deref(), Some("budget"));
        assert_eq!(step_choice(&v, Some("actual"), false), None);
        assert_eq!(step_choice(&set(&[]), None, true), None, "nothing to step to");
    }

    #[test]
    fn the_keys_act_on_the_last_touched_dimension_else_the_first() {
        let keys = ["region".to_string(), "scenario".to_string()];
        assert_eq!(focused_key("scenario", keys.iter()).as_deref(), Some("scenario"));
        assert_eq!(focused_key("gone", keys.iter()).as_deref(), Some("region"));
        assert_eq!(focused_key("", Vec::<String>::new().iter()), None);
    }

    #[test]
    fn cycling_changes_the_chosen_value_and_remembers_the_dimension() {
        let mut m = Maix::default();
        m.declare_dimension("scenario", Some("actual"));
        m.declare_dimension("scenario", Some("budget"));
        m.declare_dimension("region", Some("TZ"));
        let mut view = DataView::default();
        view.combine_dims = true;
        view.cycle_dimension(&mut m, true, false);
        assert_eq!(m.active_dims.get("region").map(String::as_str), Some("TZ"), "the first dimension, first value");
        view.cycle_dimension(&mut m, true, true);
        view.cycle_dimension(&mut m, true, false);
        assert_eq!(m.active_dims.get("scenario").map(String::as_str), Some("actual"), "Shift moved to the next dimension");
        view.cycle_dimension(&mut m, false, false);
        assert!(!m.active_dims.contains_key("scenario"), "stepping back reaches plain");
    }

    #[test]
    fn on_their_own_the_dimensions_never_mix_and_there_is_one_plain() {
        let mut m = Maix::default();
        let sheet = m.add_sheet("S");
        for (k, v) in [("default", "d1"), ("default", "d2"), ("alfa", "alfa1"), ("alfa", "alfa2")] {
            m.declare_dimension(k, Some(v));
        }
        let by = |k: &str, v: &str| -> alfadoc_model::DimVector { [(k.to_string(), v.to_string())].into_iter().collect() };
        let stamp = alfadoc_model::Stamp::new("t", 1);
        let mut view = DataView::default();
        assert!(!view.combine_dims, "dimensions are separate unless combining is turned on");
        let visit = |m: &mut Maix, view: &mut DataView, k: &str, v: &str| {
            view.dim_focus = k.into();
            m.active_dims.clear();
            m.active_dims.insert(k.into(), v.into());
            let _ = m.set(sheet, 0, 0, &m.active_dims.clone(), alfadoc_model::Value::Text(format!("{k}={v}")), &stamp);
        };
        visit(&mut m, &mut view, "default", "d1");
        visit(&mut m, &mut view, "alfa", "alfa1");
        visit(&mut m, &mut view, "default", "d2");
        let read = |m: &Maix, k: &str, v: &str| m.get(sheet, 0, 0, &by(k, v)).unwrap().cloned();
        assert_eq!(read(&m, "default", "d1"), Some(alfadoc_model::Value::Text("default=d1".into())));
        assert_eq!(read(&m, "alfa", "alfa1"), Some(alfadoc_model::Value::Text("alfa=alfa1".into())));
        assert_eq!(read(&m, "default", "d2"), Some(alfadoc_model::Value::Text("default=d2".into())));
        // Stepping with the keys walks plain, then every value of every dimension, and back to plain.
        m.active_dims.clear();
        let mut seen = vec![];
        for _ in 0..5 {
            view.cycle_dimension(&mut m, true, false);
            seen.push(alfadoc_model::dims::canonical(&m.active_dims));
        }
        assert_eq!(seen, ["alfa=alfa1", "alfa=alfa2", "default=d1", "default=d2", ""]);
    }
}
