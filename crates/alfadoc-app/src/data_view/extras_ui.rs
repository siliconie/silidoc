//! Merged cells, conditional formatting, data validation, comments and groups in the Data tab:
//! the operations the menus trigger, their dialogs, and the colour-scale cache the grid uses.

use alfadoc_model::{Axis, CondKind, CondRule, DimVector, Maix, ValidKind, Validation, Value};
use egui::{Align2, Key};

use super::{Cell, Change, Content, DataView, Step, cell_count, column_name, content_from, put, put_input, whole_cols, whole_rows};

/// Most cells a colour scale looks at to find its smallest and biggest number.
const SCALE_SCAN: usize = 300_000;

// ---- drafts of the dialogs

pub(super) enum Dialog {
    Comment { cell: Cell, text: String },
    Validation(ValidDraft),
    Rules(RuleDraft),
}

#[derive(Clone, Debug, Default)]
pub(super) struct ValidDraft {
    /// 0 list, 1 whole number, 2 number, 3 text length.
    pub kind: usize,
    /// One item per line (commas also separate).
    pub items: String,
    pub min: String,
    pub max: String,
    pub message: String,
    pub allow_blank: bool,
    pub error: String,
}

const RULE_KINDS: [&str; 8] = ["Greater than", "Less than", "Equal to", "Between", "Text contains", "Is blank", "Is not blank", "Colour scale"];

#[derive(Clone, Debug)]
pub(super) struct RuleDraft {
    pub kind: usize,
    pub a: String,
    pub b: String,
    pub text: String,
    pub fill: [u8; 3],
    pub use_fill: bool,
    pub color: [u8; 3],
    pub use_color: bool,
    pub bold: bool,
    pub low: [u8; 3],
    pub high: [u8; 3],
    pub error: String,
}

impl Default for RuleDraft {
    fn default() -> Self {
        RuleDraft { kind: 0, a: String::new(), b: String::new(), text: String::new(), fill: [0xF4, 0xCC, 0xCC], use_fill: true, color: [0x99, 0x00, 0x00], use_color: false, bold: false, low: [0xFF, 0xFF, 0xFF], high: [0x63, 0xBE, 0x7B], error: String::new() }
    }
}

fn to_u32(c: [u8; 3]) -> u32 {
    (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2])
}

fn num(text: &str) -> Result<Option<f64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>().map(Some).map_err(|_| format!("“{t}” is not a number."))
}

/// The rule a draft describes, or why it is not one yet.
pub(super) fn rule_from(draft: &RuleDraft, range: alfadoc_model::Rect) -> Result<CondRule, String> {
    let need = |text: &str| num(text)?.ok_or_else(|| "Enter a number.".to_string());
    let kind = match draft.kind {
        0 => CondKind::Greater { value: need(&draft.a)? },
        1 => CondKind::Less { value: need(&draft.a)? },
        2 => CondKind::Equal { value: need(&draft.a)? },
        3 => CondKind::Between { low: need(&draft.a)?, high: need(&draft.b)? },
        4 => {
            if draft.text.trim().is_empty() {
                return Err("Enter the text to look for.".into());
            }
            CondKind::TextContains { text: draft.text.trim().to_string() }
        }
        5 => CondKind::Blank,
        6 => CondKind::NotBlank,
        _ => CondKind::ColorScale { low: to_u32(draft.low), high: to_u32(draft.high) },
    };
    let scale = matches!(kind, CondKind::ColorScale { .. });
    if !scale && !draft.use_fill && !draft.use_color && !draft.bold {
        return Err("Choose a fill, a text colour or bold.".into());
    }
    Ok(CondRule { range, kind, fill: (!scale && draft.use_fill).then(|| to_u32(draft.fill)), color: (!scale && draft.use_color).then(|| to_u32(draft.color)), bold: !scale && draft.bold })
}

/// The validation a draft describes, or why it is not one yet.
pub(super) fn validation_from(draft: &ValidDraft, range: alfadoc_model::Rect) -> Result<Validation, String> {
    let kind = match draft.kind {
        0 => {
            let items: Vec<String> = draft.items.split(['\n', ',']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if items.is_empty() {
                return Err("Type the allowed items, one per line.".into());
            }
            ValidKind::List { items }
        }
        1 => ValidKind::Whole { min: num(&draft.min)?.map(|x| x.round() as i64), max: num(&draft.max)?.map(|x| x.round() as i64) },
        2 => ValidKind::Decimal { min: num(&draft.min)?, max: num(&draft.max)? },
        _ => ValidKind::TextLength { min: num(&draft.min)?.map(|x| x.max(0.0) as u32), max: num(&draft.max)?.map(|x| x.max(0.0) as u32) },
    };
    Ok(Validation { range, kind, message: draft.message.trim().to_string(), allow_blank: draft.allow_blank })
}

fn rule_text(rule: &CondRule) -> String {
    let what = match &rule.kind {
        CondKind::Greater { value } => format!("greater than {value}"),
        CondKind::Less { value } => format!("less than {value}"),
        CondKind::Equal { value } => format!("equal to {value}"),
        CondKind::Between { low, high } => format!("between {low} and {high}"),
        CondKind::TextContains { text } => format!("text contains “{text}”"),
        CondKind::Blank => "is blank".to_string(),
        CondKind::NotBlank => "is not blank".to_string(),
        CondKind::ColorScale { .. } => "colour scale".to_string(),
    };
    let r = rule.range;
    format!("{}{}:{}{}  {what}", column_name(r.1), r.0 + 1, column_name(r.3), r.2 + 1)
}

/// What a group command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GroupAction {
    CollapseHere,
    ExpandHere,
    CollapseAll,
    ExpandAll,
}

/// Smallest and biggest number of each colour-scale rule's range (`None` for other rules and for
/// ranges without numbers).
pub(super) fn scale_ranges(maix: &Maix, sheet: u16) -> Vec<Option<(f64, f64)>> {
    let Ok(sh) = maix.sheet(sheet) else { return vec![] };
    let rules = &sh.format.extras.rules;
    let mut out: Vec<Option<(f64, f64)>> = vec![None; rules.len()];
    let Some(dim) = maix.vectors.find(&maix.active_dims) else { return out };
    for (r, c, cell) in sh.slice(dim).take(SCALE_SCAN) {
        let x = match &cell.value {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            _ => continue,
        };
        for (i, rule) in rules.iter().enumerate() {
            if rule.is_scale() && r >= rule.range.0 && r <= rule.range.2 && c >= rule.range.1 && c <= rule.range.3 {
                out[i] = Some(match out[i] {
                    Some((lo, hi)) => (lo.min(x), hi.max(x)),
                    None => (x, x),
                });
            }
        }
    }
    out
}

/// The scale ranges, kept until the cells, the rules or the shown dimensions change.
pub(super) struct ScaleCache {
    sheet: u16,
    history: usize,
    dims: DimVector,
    rules: Vec<CondRule>,
    out: Vec<Option<(f64, f64)>>,
}

impl DataView {
    pub(super) fn scales(&mut self, maix: &Maix, sheet: u16) -> Vec<Option<(f64, f64)>> {
        let Ok(sh) = maix.sheet(sheet) else { return vec![] };
        if !sh.format.extras.has_scale() {
            return vec![];
        }
        let rules = &sh.format.extras.rules;
        if let Some(c) = &self.scale_cache
            && c.sheet == sheet
            && c.history == maix.history.len()
            && c.dims == maix.active_dims
            && c.rules == *rules
        {
            return c.out.clone();
        }
        let out = scale_ranges(maix, sheet);
        self.scale_cache = Some(ScaleCache { sheet, history: maix.history.len(), dims: maix.active_dims.clone(), rules: rules.clone(), out: out.clone() });
        out
    }

    /// Where a click or a step on `cell` lands: the top-left cell of the merged area it is in.
    pub(super) fn merge_origin(maix: &Maix, sheet: u16, cell: Cell) -> Cell {
        maix.sheet(sheet).ok().and_then(|s| s.format.extras.merge_at(cell.0, cell.1)).map_or(cell, |m| (m.0, m.1))
    }

    /// Like `put_input`, but refuses text that the cell's validation does not allow (and says why).
    pub(super) fn checked_put(&mut self, maix: &mut Maix, sheet: u16, cell: Cell, text: &str, now: u64, batch: &mut Vec<Change>) -> bool {
        if let Content::Value(v) = content_from(text)
            && let Ok(sh) = maix.sheet(sheet)
            && let Some(rule) = sh.format.extras.validation_at(cell.0, cell.1)
            && let Err(message) = rule.check(&v)
        {
            self.say(message);
            return false;
        }
        put_input(maix, sheet, cell, text, now, batch);
        true
    }

    // ---- merged cells

    pub(super) fn merge_selection(&mut self, maix: &mut Maix, sheet: u16) {
        let sel = self.sel();
        if whole_cols(sel) || whole_rows(sel) {
            self.say("Select a block of cells to merge, not whole rows or columns.");
            return;
        }
        if cell_count(sel) < 2 {
            self.say("Select two or more cells to merge.");
            return;
        }
        // Only the top-left cell keeps its content; clearing the rest is its own undo step.
        let mut cleared: Vec<Change> = vec![];
        for r in sel.0..=sel.2 {
            for c in sel.1..=sel.3 {
                if (r, c) != (sel.0, sel.1) {
                    put(maix, sheet, (r, c), Value::Null, self.now_ms, &mut cleared);
                }
            }
        }
        if !cleared.is_empty() {
            self.steps.push(Step::Cells(cleared));
        }
        let mut done = false;
        self.edit_format(maix, sheet, |f| done = f.extras.merge(sel));
        if done {
            self.anchor = (sel.0, sel.1);
            self.focus = (sel.0, sel.1);
        } else {
            self.say("There are too many merged areas on this sheet.");
        }
    }

    pub(super) fn unmerge_selection(&mut self, maix: &mut Maix, sheet: u16) {
        let sel = self.sel();
        let mut n = 0;
        self.edit_format(maix, sheet, |f| n = f.extras.unmerge(sel));
        if n == 0 {
            self.say("There is no merged cell in the selection.");
        }
    }

    // ---- groups

    fn group_axis(&self) -> Axis {
        let sel = self.sel();
        if whole_cols(sel) && !whole_rows(sel) { Axis::Cols } else { Axis::Rows }
    }

    pub(super) fn group_selection(&mut self, maix: &mut Maix, sheet: u16) {
        let axis = self.group_axis();
        let (lo, hi) = self.selected_lines(axis == Axis::Cols);
        let mut ok = false;
        self.edit_format(maix, sheet, |f| ok = f.extras.group(axis, lo, hi));
        if !ok {
            self.say("Those lines cannot be grouped any deeper.");
        }
    }

    pub(super) fn ungroup_selection(&mut self, maix: &mut Maix, sheet: u16) {
        let axis = self.group_axis();
        let (lo, hi) = self.selected_lines(axis == Axis::Cols);
        self.edit_format(maix, sheet, |f| f.extras.ungroup(axis, lo, hi));
    }

    /// Hides or shows the group at the selection, or every group.
    pub(super) fn group_action(&mut self, maix: &mut Maix, sheet: u16, action: GroupAction) {
        let axis = self.group_axis();
        let (lo, hi) = self.selected_lines(axis == Axis::Cols);
        let mut found = false;
        self.edit_format(maix, sheet, |f| {
            let mut spans: Vec<(Axis, u32, u32)> = vec![];
            match action {
                GroupAction::CollapseHere => {
                    // The groups of the selected lines.
                    for i in lo..=hi.min(lo.saturating_add(10_000)) {
                        if let Some((a, b)) = f.extras.run(axis, i) {
                            spans.push((axis, a, b));
                        }
                    }
                }
                GroupAction::ExpandHere => {
                    // Hidden lines next to the selection belong to a collapsed group.
                    for i in lo.saturating_sub(1)..=hi.saturating_add(1).min(lo.saturating_add(10_000)) {
                        if let Some((a, b)) = f.extras.run(axis, i) {
                            spans.push((axis, a, b));
                        }
                    }
                }
                GroupAction::CollapseAll | GroupAction::ExpandAll => {
                    spans.extend(f.extras.row_levels.keys().map(|r| (Axis::Rows, *r, *r)));
                    spans.extend(f.extras.col_levels.keys().map(|c| (Axis::Cols, u32::from(*c), u32::from(*c))));
                }
            }
            let hide = matches!(action, GroupAction::CollapseHere | GroupAction::CollapseAll);
            for (ax, a, b) in spans {
                found = true;
                for i in a..=b {
                    match ax {
                        Axis::Rows => {
                            if hide { f.layout.hidden_rows.insert(i) } else { f.layout.hidden_rows.remove(&i) };
                        }
                        Axis::Cols => {
                            if let Ok(c) = u16::try_from(i) {
                                if hide { f.layout.hidden_cols.insert(c) } else { f.layout.hidden_cols.remove(&c) };
                            }
                        }
                    }
                }
            }
        });
        if !found {
            self.say("There is no group here. Select rows or columns and choose Group first.");
        }
    }

    // ---- dialogs

    pub(super) fn open_comment(&mut self, maix: &Maix, sheet: u16) {
        let cell = self.focus;
        let text = maix.sheet(sheet).ok().and_then(|s| s.format.extras.comment_at(cell.0, cell.1).map(str::to_string)).unwrap_or_default();
        self.dialog = Some(Dialog::Comment { cell, text });
    }

    pub(super) fn delete_comment(&mut self, maix: &mut Maix, sheet: u16) {
        let cell = self.focus;
        self.edit_format(maix, sheet, |f| f.extras.set_comment(cell.0, cell.1, None));
    }

    pub(super) fn open_validation(&mut self, maix: &Maix, sheet: u16) {
        let mut d = ValidDraft { allow_blank: true, ..ValidDraft::default() };
        if let Some(v) = maix.sheet(sheet).ok().and_then(|s| s.format.extras.validation_at(self.focus.0, self.focus.1)) {
            d.allow_blank = v.allow_blank;
            d.message = v.message.clone();
            match &v.kind {
                ValidKind::List { items } => d.items = items.join("\n"),
                ValidKind::Whole { min, max } => {
                    d.kind = 1;
                    d.min = min.map(|x| x.to_string()).unwrap_or_default();
                    d.max = max.map(|x| x.to_string()).unwrap_or_default();
                }
                ValidKind::Decimal { min, max } => {
                    d.kind = 2;
                    d.min = min.map(|x| x.to_string()).unwrap_or_default();
                    d.max = max.map(|x| x.to_string()).unwrap_or_default();
                }
                ValidKind::TextLength { min, max } => {
                    d.kind = 3;
                    d.min = min.map(|x| x.to_string()).unwrap_or_default();
                    d.max = max.map(|x| x.to_string()).unwrap_or_default();
                }
            }
        }
        self.dialog = Some(Dialog::Validation(d));
    }

    pub(super) fn open_rules(&mut self) {
        self.dialog = Some(Dialog::Rules(RuleDraft::default()));
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn extras_window(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let sel = self.sel();
        let (mut ok, mut cancel, mut clear) = (false, false, false);
        let mut delete_rule: Option<usize> = None;
        let title = match &dialog {
            Dialog::Comment { .. } => "Comment",
            Dialog::Validation(_) => "Data validation",
            Dialog::Rules(_) => "Conditional formatting",
        };
        let rules: Vec<CondRule> = maix.sheet(sheet).map(|s| s.format.extras.rules.clone()).unwrap_or_default();
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| match &mut dialog {
            Dialog::Comment { cell, text } => {
                ui.label(format!("Note on {}{}", column_name(cell.1), cell.0 + 1));
                let r = ui.add(egui::TextEdit::multiline(text).desired_rows(4).desired_width(300.0));
                r.request_focus();
                ui.horizontal(|ui| {
                    ok |= ui.button("OK").clicked();
                    cancel |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    clear |= ui.button("Delete comment").clicked();
                });
            }
            Dialog::Validation(d) => {
                ui.label(format!("Applies to {}{}:{}{}", column_name(sel.1), sel.0 + 1, column_name(sel.3), sel.2 + 1));
                ui.horizontal(|ui| {
                    ui.label("Allow");
                    egui::ComboBox::from_id_salt("valid_kind").selected_text(["A list of items", "Whole numbers", "Numbers", "Text of a length"][d.kind.min(3)]).show_ui(ui, |ui| {
                        for (i, name) in ["A list of items", "Whole numbers", "Numbers", "Text of a length"].iter().enumerate() {
                            ui.selectable_value(&mut d.kind, i, *name);
                        }
                    });
                });
                if d.kind == 0 {
                    ui.label("Items, one per line:");
                    ui.add(egui::TextEdit::multiline(&mut d.items).desired_rows(4).desired_width(300.0));
                } else {
                    ui.horizontal(|ui| {
                        ui.label("From");
                        ui.add(egui::TextEdit::singleline(&mut d.min).desired_width(80.0).hint_text("no limit"));
                        ui.label("to");
                        ui.add(egui::TextEdit::singleline(&mut d.max).desired_width(80.0).hint_text("no limit"));
                    });
                }
                ui.checkbox(&mut d.allow_blank, "Empty cells are fine");
                ui.horizontal(|ui| {
                    ui.label("Message");
                    ui.add(egui::TextEdit::singleline(&mut d.message).desired_width(240.0).hint_text("shown when a value is refused"));
                });
                if !d.error.is_empty() {
                    ui.colored_label(ui.visuals().warn_fg_color, &d.error);
                }
                ui.weak("Checked when you type a value; pasting and filling do not check.");
                ui.horizontal(|ui| {
                    ok |= ui.button("OK").clicked();
                    cancel |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    clear |= ui.button("Remove validation").clicked();
                });
            }
            Dialog::Rules(d) => {
                if rules.is_empty() {
                    ui.weak("No rules on this sheet yet.");
                }
                for (i, rule) in rules.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if let (Some(f), None) = (rule.fill, rule.is_scale().then_some(())) {
                            ui.colored_label(super::grid::rgb(f), "■");
                        }
                        ui.label(rule_text(rule));
                        if ui.small_button("Delete").clicked() {
                            delete_rule = Some(i);
                        }
                    });
                }
                ui.separator();
                ui.label(format!("New rule for {}{}:{}{}", column_name(sel.1), sel.0 + 1, column_name(sel.3), sel.2 + 1));
                egui::ComboBox::from_id_salt("rule_kind").selected_text(RULE_KINDS[d.kind.min(7)]).show_ui(ui, |ui| {
                    for (i, name) in RULE_KINDS.iter().enumerate() {
                        ui.selectable_value(&mut d.kind, i, *name);
                    }
                });
                match d.kind {
                    0..=2 => {
                        ui.add(egui::TextEdit::singleline(&mut d.a).desired_width(100.0).hint_text("number"));
                    }
                    3 => {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.a).desired_width(80.0).hint_text("from"));
                            ui.label("and");
                            ui.add(egui::TextEdit::singleline(&mut d.b).desired_width(80.0).hint_text("to"));
                        });
                    }
                    4 => {
                        ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(200.0).hint_text("text"));
                    }
                    _ => {}
                }
                if d.kind == 7 {
                    ui.horizontal(|ui| {
                        ui.label("Smallest");
                        ui.color_edit_button_srgb(&mut d.low);
                        ui.label("Biggest");
                        ui.color_edit_button_srgb(&mut d.high);
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.use_fill, "Fill");
                        ui.color_edit_button_srgb(&mut d.fill);
                        ui.checkbox(&mut d.use_color, "Text");
                        ui.color_edit_button_srgb(&mut d.color);
                        ui.checkbox(&mut d.bold, "Bold");
                    });
                }
                if !d.error.is_empty() {
                    ui.colored_label(ui.visuals().warn_fg_color, &d.error);
                }
                ui.horizontal(|ui| {
                    ok |= ui.button("Add rule").clicked();
                    cancel |= ui.button("Close").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                });
            }
        });
        if cancel {
            return;
        }
        if let Some(i) = delete_rule {
            self.edit_format(maix, sheet, |f| {
                f.extras.remove_rule(i);
            });
        }
        match &mut dialog {
            Dialog::Comment { cell, text } => {
                if clear {
                    let c = *cell;
                    self.edit_format(maix, sheet, |f| f.extras.set_comment(c.0, c.1, None));
                    return;
                }
                if ok {
                    let (c, t) = (*cell, text.clone());
                    self.edit_format(maix, sheet, |f| f.extras.set_comment(c.0, c.1, Some(&t)));
                    return;
                }
            }
            Dialog::Validation(d) => {
                if clear {
                    self.edit_format(maix, sheet, |f| {
                        f.extras.set_validation(sel, None);
                    });
                    return;
                }
                if ok {
                    match validation_from(d, sel) {
                        Ok(v) => {
                            let mut stored = false;
                            self.edit_format(maix, sheet, |f| stored = f.extras.set_validation(sel, Some(v)));
                            if !stored {
                                self.say("There are too many validations on this sheet.");
                            }
                            return;
                        }
                        Err(e) => d.error = e,
                    }
                }
            }
            Dialog::Rules(d) => {
                if ok {
                    match rule_from(d, sel) {
                        Ok(rule) => {
                            let mut stored = false;
                            self.edit_format(maix, sheet, |f| stored = f.extras.add_rule(rule));
                            if stored {
                                d.error.clear();
                            } else {
                                d.error = "There are too many rules on this sheet.".into();
                            }
                        }
                        Err(e) => d.error = e,
                    }
                }
            }
        }
        self.dialog = Some(dialog);
    }

    /// The drop-down of a list-validated cell.
    pub(super) fn list_window(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        let Some((cell, pos)) = self.list_pick else { return };
        let items: Vec<String> = match maix.sheet(sheet).ok().and_then(|s| s.format.extras.validation_at(cell.0, cell.1).cloned()) {
            Some(Validation { kind: ValidKind::List { items }, .. }) => items,
            _ => {
                self.list_pick = None;
                return;
            }
        };
        let mut chosen: Option<String> = None;
        let area = egui::Area::new(egui::Id::unique("data_list_pick")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(120.0);
                for item in &items {
                    if ui.selectable_label(false, item).clicked() {
                        chosen = Some(item.clone());
                    }
                }
            });
        });
        let closed = area.response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(Key::Escape));
        if let Some(item) = chosen {
            self.checked_put(maix, sheet, cell, &item, now, batch);
            self.list_pick = None;
        } else if closed {
            self.list_pick = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::Stamp;

    fn store() -> (Maix, u16) {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        (m, s)
    }

    #[test]
    fn merging_keeps_only_the_top_left_content_and_undoes() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put_input(&mut m, s, (0, 0), "keep", 1, &mut batch);
        put_input(&mut m, s, (0, 1), "lose", 1, &mut batch);
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (1, 1);
        v.merge_selection(&mut m, s);
        assert_eq!(m.sheet(s).unwrap().format.extras.merges, vec![(0, 0, 1, 1)]);
        assert_eq!(super::super::shown(&m, s, 0, 0), "keep");
        assert_eq!(super::super::shown(&m, s, 0, 1), "");
        assert_eq!(DataView::merge_origin(&m, s, (1, 1)), (0, 0));
        assert_eq!(DataView::merge_origin(&m, s, (5, 5)), (5, 5));
        let step = v.steps.pop().unwrap();
        v.run_step(&mut m, &step, true, 3);
        assert!(m.sheet(s).unwrap().format.extras.merges.is_empty());
        v.unmerge_selection(&mut m, s);
        assert!(v.note.is_some());
    }

    #[test]
    fn typed_values_must_pass_the_validation() {
        let (mut m, s) = store();
        let mut batch = vec![];
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (2, 0);
        let rule = validation_from(&ValidDraft { kind: 1, min: "1".into(), max: "5".into(), allow_blank: true, ..ValidDraft::default() }, (0, 0, 2, 0)).unwrap();
        v.edit_format(&mut m, s, |f| {
            f.extras.set_validation((0, 0, 2, 0), Some(rule));
        });
        assert!(v.checked_put(&mut m, s, (1, 0), "3", 1, &mut batch));
        assert!(!v.checked_put(&mut m, s, (1, 0), "9", 1, &mut batch));
        assert!(!v.checked_put(&mut m, s, (1, 0), "abc", 1, &mut batch));
        assert!(v.checked_put(&mut m, s, (4, 0), "abc", 1, &mut batch), "outside the range");
        assert_eq!(super::super::shown(&m, s, 1, 0), "3");
        assert!(v.checked_put(&mut m, s, (1, 0), "=1+1", 1, &mut batch), "formulas are not checked");
        assert!(validation_from(&ValidDraft { kind: 0, ..ValidDraft::default() }, (0, 0, 0, 0)).is_err());
        assert!(validation_from(&ValidDraft { kind: 2, min: "x".into(), ..ValidDraft::default() }, (0, 0, 0, 0)).is_err());
    }

    #[test]
    fn rules_come_from_drafts_and_scales_find_their_range() {
        let d = RuleDraft { kind: 0, a: "10".into(), ..RuleDraft::default() };
        let rule = rule_from(&d, (0, 0, 9, 0)).unwrap();
        assert_eq!(rule.kind, CondKind::Greater { value: 10.0 });
        assert_eq!(rule.fill, Some(0xF4CCCC));
        assert!(rule_from(&RuleDraft { kind: 0, ..RuleDraft::default() }, (0, 0, 0, 0)).is_err());
        assert!(rule_from(&RuleDraft { kind: 5, use_fill: false, ..RuleDraft::default() }, (0, 0, 0, 0)).is_err());
        let (mut m, s) = store();
        let by = Stamp::new("t", 1);
        for (r, x) in [3, 9, 6].into_iter().enumerate() {
            m.set(s, r as u32, 0, &DimVector::new(), Value::Int(x), &by).unwrap();
        }
        m.set(s, 5, 0, &DimVector::new(), Value::Int(100), &by).unwrap();
        m.format_mut(s).unwrap().extras.add_rule(rule_from(&RuleDraft { kind: 7, ..RuleDraft::default() }, (0, 0, 3, 0)).unwrap());
        assert_eq!(scale_ranges(&m, s), vec![Some((3.0, 9.0))]);
        let mut v = DataView::default();
        assert_eq!(v.scales(&m, s), vec![Some((3.0, 9.0))]);
        m.set(s, 3, 0, &DimVector::new(), Value::Int(20), &by).unwrap();
        assert_eq!(v.scales(&m, s), vec![Some((3.0, 20.0))], "the cache notices new values");
    }

    #[test]
    fn groups_collapse_and_expand_by_hiding_lines() {
        let (mut m, s) = store();
        let mut v = DataView::default();
        v.anchor = (2, 0);
        v.focus = (4, 0);
        v.group_selection(&mut m, s);
        assert_eq!(m.sheet(s).unwrap().format.extras.run(Axis::Rows, 3), Some((2, 4)));
        v.group_action(&mut m, s, GroupAction::CollapseHere);
        let hidden = |m: &Maix| m.sheet(s).unwrap().format.layout.hidden_rows.iter().copied().collect::<Vec<_>>();
        assert_eq!(hidden(&m), vec![2, 3, 4]);
        v.anchor = (1, 0);
        v.focus = (1, 0);
        v.group_action(&mut m, s, GroupAction::ExpandHere);
        assert!(hidden(&m).is_empty());
        v.group_action(&mut m, s, GroupAction::CollapseAll);
        assert_eq!(hidden(&m), vec![2, 3, 4]);
        v.group_action(&mut m, s, GroupAction::ExpandAll);
        assert!(hidden(&m).is_empty());
        v.anchor = (2, 0);
        v.focus = (4, 0);
        v.ungroup_selection(&mut m, s);
        assert!(!m.sheet(s).unwrap().format.extras.is_grouped(Axis::Rows));
    }

    #[test]
    fn comments_are_set_through_the_format_and_undo() {
        let (mut m, s) = store();
        let mut v = DataView::default();
        v.focus = (1, 1);
        v.edit_format(&mut m, s, |f| f.extras.set_comment(1, 1, Some("check")));
        assert_eq!(m.sheet(s).unwrap().format.extras.comment_at(1, 1), Some("check"));
        v.open_comment(&m, s);
        assert!(matches!(&v.dialog, Some(Dialog::Comment { text, .. }) if text == "check"));
        v.delete_comment(&mut m, s);
        assert!(m.sheet(s).unwrap().format.extras.comments.is_empty());
    }
}
