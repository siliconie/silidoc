//! The Data tab's menus, toolbar, context menus and small dialogs.

use alfadoc_model::{Axis, CellStyle, HAlign, Maix};
use egui::{Align2, Key, RichText, Sense, Vec2};

use super::grid::rgb;
use super::ops::{PasteKind, PasteOpts, Toggle};
use super::{Change, DataView, Hit, MAX_ZOOM, MIN_ZOOM, cell_count, clear, column_name, numfmt, parse_address, value_at, whole_cols, whole_rows};
use alfadoc_model::format::{MAX_COLS, MAX_COL_W, MAX_ROWS, MAX_ROW_H, MIN_SIZE};

/// A small dialog.
pub(super) enum Prompt {
    GoTo(String),
    Size { cols: bool, value: f32 },
    NumFmt(String),
    Sort { key: u16, ascending: bool, header: bool },
    DeleteSheet(u16),
    PasteSpecial(PasteOpts),
}

/// Sizes offered in the toolbar, in points.
const FONT_SIZES: [u8; 12] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36];

const SWATCHES: [u32; 40] = [
    0x000000, 0x434343, 0x666666, 0x999999, 0xB7B7B7, 0xCCCCCC, 0xEFEFEF, 0xFFFFFF, 0x980000, 0xFF0000, 0xFF9900, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x4A86E8, 0x9900FF, 0xE6B8AF, 0xF4CCCC, 0xFCE5CD, 0xFFF2CC, 0xD9EAD3, 0xD0E0E3, 0xC9DAF8, 0xD9D2E9,
    0xDD7E6B, 0xEA9999, 0xF9CB9C, 0xFFE599, 0xB6D7A8, 0xA2C4C9, 0xA4C2F4, 0xB4A7D6, 0xA61C00, 0xCC0000, 0xE69138, 0xF1C232, 0x6AA84F, 0x45818E, 0x3C78D8, 0x674EA7,
];

/// A menu line; closes the menu when clicked.
fn item(ui: &mut egui::Ui, text: &str, shortcut: &str, enabled: bool) -> bool {
    let mut button = egui::Button::new(text);
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// A colour picker button: `Some(None)` is "automatic / none", `Some(Some(c))` a colour.
fn swatches(ui: &mut egui::Ui, label: RichText, tip: &str, none_label: &str, current: Option<u32>) -> Option<Option<u32>> {
    let mut chosen = None;
    let label = match current {
        Some(c) => label.color(rgb(c)),
        None => label,
    };
    let response = ui.menu_button(label, |ui| {
        if ui.button(none_label).clicked() {
            chosen = Some(None);
            ui.close();
        }
        egui::Grid::new("swatches").spacing([3.0, 3.0]).show(ui, |ui| {
            for (i, c) in SWATCHES.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                ui.painter().rect_filled(rect, 2.0, rgb(*c));
                ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
                if response.clicked() {
                    chosen = Some(Some(*c));
                    ui.close();
                }
                if i % 8 == 7 {
                    ui.end_row();
                }
            }
        });
    });
    response.response.on_hover_text(tip);
    chosen
}

impl DataView {
    // ---- menu bar

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Edit", |ui| self.edit_menu(ui, maix, sheet, now, batch));
            ui.menu_button("Insert", |ui| self.insert_menu(ui, maix, sheet, now));
            ui.menu_button("Format", |ui| self.format_menu(ui, maix, sheet));
            ui.menu_button("Data", |ui| self.data_menu(ui, maix, sheet));
            ui.menu_button("View", |ui| self.view_menu(ui, maix, sheet));
        });
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        if item(ui, "Undo", "Ctrl+Z", !self.undo.is_empty()) {
            self.undo_step(maix, now);
        }
        if item(ui, "Redo", "Ctrl+Y", !self.redo.is_empty()) {
            self.redo_step(maix, now);
        }
        ui.separator();
        self.clipboard_items(ui, maix, sheet, now, batch);
        ui.separator();
        self.clear_items(ui, maix, sheet, now, batch);
        ui.separator();
        if item(ui, "Fill down", "Ctrl+D", true) {
            self.fill(maix, sheet, true, now, batch);
        }
        if item(ui, "Fill right", "Ctrl+R", true) {
            self.fill(maix, sheet, false, now, batch);
        }
        ui.separator();
        if item(ui, "Select all", "Ctrl+A", true) {
            let used = self.used(maix, sheet);
            self.anchor = (0, 0);
            self.focus = (used.0.saturating_sub(1), used.1.saturating_sub(1));
        }
        if item(ui, "Go to…", "Ctrl+G", true) {
            self.prompt = Some(Prompt::GoTo(String::new()));
        }
        if item(ui, "Find and replace…", "Ctrl+F", true) {
            self.open_find();
        }
    }

    fn clipboard_items(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        if item(ui, "Cut", "Ctrl+X", true) {
            self.copy(ui.ctx(), maix, sheet, true);
        }
        if item(ui, "Copy", "Ctrl+C", true) {
            self.copy(ui.ctx(), maix, sheet, false);
        }
        if item(ui, "Paste", "Ctrl+V", true) {
            self.paste_special(maix, sheet, PasteKind::All, now, batch);
        }
        ui.menu_button("Paste special", |ui| {
            if item(ui, "Values only", "", self.clip.is_some()) {
                self.paste_special(maix, sheet, PasteKind::Values, now, batch);
            }
            if item(ui, "Formulas only", "", self.clip.is_some()) {
                self.paste_special(maix, sheet, PasteKind::Formulas, now, batch);
            }
            if item(ui, "Formatting only", "", self.clip.is_some()) {
                self.paste_special(maix, sheet, PasteKind::Formats, now, batch);
            }
            ui.separator();
            if item(ui, "Paste special…", "", self.clip.is_some()) {
                self.prompt = Some(Prompt::PasteSpecial(PasteOpts::plain(PasteKind::All)));
            }
        });
    }

    fn clear_items(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        let sel = self.sel();
        if item(ui, "Clear contents", "Del", true) {
            clear(maix, sheet, sel, now, batch);
        }
        if item(ui, "Clear formatting", "", true) {
            self.clear_formats(maix, sheet);
        }
        if item(ui, "Clear all", "", true) {
            clear(maix, sheet, sel, now, batch);
            self.clear_formats(maix, sheet);
        }
    }

    fn insert_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64) {
        let _ = now;
        let sel = self.sel();
        let rows = sel.2 - sel.0 + 1;
        let cols = u32::from(sel.3 - sel.1) + 1;
        if item(ui, &format!("{rows} row(s) above"), "", true) {
            self.insert_lines(maix, sheet, Axis::Rows, sel.0, rows);
        }
        if item(ui, &format!("{rows} row(s) below"), "", true) {
            self.insert_lines(maix, sheet, Axis::Rows, sel.2 + 1, rows);
        }
        if item(ui, &format!("{cols} column(s) left"), "", true) {
            self.insert_lines(maix, sheet, Axis::Cols, u32::from(sel.1), cols);
        }
        if item(ui, &format!("{cols} column(s) right"), "", true) {
            self.insert_lines(maix, sheet, Axis::Cols, u32::from(sel.3) + 1, cols);
        }
        ui.separator();
        if item(ui, "Delete rows", "", true) {
            self.delete_lines(maix, sheet, Axis::Rows, sel.0, rows);
        }
        if item(ui, "Delete columns", "", true) {
            self.delete_lines(maix, sheet, Axis::Cols, u32::from(sel.1), cols);
        }
        ui.separator();
        if item(ui, "New sheet", "", true) {
            self.add_sheet(maix);
        }
    }

    pub(super) fn add_sheet(&mut self, maix: &mut Maix) {
        let mut n = maix.sheets.len() + 1;
        while maix.sheet_by_name(&format!("Sheet{n}")).is_some() {
            n += 1;
        }
        let id = maix.add_sheet(&format!("Sheet{n}"));
        self.switch_to(id);
    }

    fn format_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        for (label, which, key) in [("Bold", Toggle::Bold, "Ctrl+B"), ("Italic", Toggle::Italic, "Ctrl+I"), ("Underline", Toggle::Underline, "Ctrl+U"), ("Strikethrough", Toggle::Strike, "Ctrl+Shift+5"), ("Wrap text", Toggle::Wrap, "")] {
            if item(ui, label, key, true) {
                self.toggle(maix, sheet, which);
            }
        }
        ui.separator();
        ui.menu_button("Number format", |ui| self.number_format_items(ui, maix, sheet));
        ui.menu_button("Align", |ui| {
            for (label, a) in [("General", HAlign::General), ("Left", HAlign::Left), ("Center", HAlign::Center), ("Right", HAlign::Right)] {
                if item(ui, label, "", true) {
                    self.restyle_selection(maix, sheet, &move |s| CellStyle { align: a, ..s.clone() });
                }
            }
        });
        ui.separator();
        self.size_items(ui, maix, sheet);
        ui.separator();
        if item(ui, "Clear formatting", "", true) {
            self.clear_formats(maix, sheet);
        }
    }

    fn number_format_items(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        for (name, code) in numfmt::PRESETS {
            if item(ui, name, "", true) {
                let code = (*code).to_string();
                self.restyle_selection(maix, sheet, &move |s| CellStyle { numfmt: code.clone(), ..s.clone() });
            }
        }
        ui.separator();
        if item(ui, "Custom…", "", true) {
            let current = maix.sheet(sheet).map(|s| s.format.style_at(self.focus.0, self.focus.1).numfmt).unwrap_or_default();
            self.prompt = Some(Prompt::NumFmt(current));
        }
    }

    fn size_items(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        if item(ui, "Row height…", "", true) {
            let value = self.current_size(maix, sheet, false);
            self.prompt = Some(Prompt::Size { cols: false, value });
        }
        if item(ui, "Column width…", "", true) {
            let value = self.current_size(maix, sheet, true);
            self.prompt = Some(Prompt::Size { cols: true, value });
        }
        if item(ui, "Fit row height to content", "", true) {
            let (first, _) = self.selected_lines(false);
            self.fit_selection(ui, maix, sheet, false, first);
        }
        if item(ui, "Fit column width to content", "", true) {
            let (first, _) = self.selected_lines(true);
            self.fit_selection(ui, maix, sheet, true, first);
        }
        if item(ui, "Reset row height and column width", "", true) {
            let (r0, r1) = self.selected_lines(false);
            let (c0, c1) = self.selected_lines(true);
            self.set_size(maix, sheet, false, r0, r1, None);
            self.set_size(maix, sheet, true, c0, c1, None);
        }
        ui.separator();
        if item(ui, "Hide rows", "", true) {
            let (a, b) = self.selected_lines(false);
            self.hide_lines(maix, sheet, false, a, b, true);
        }
        if item(ui, "Hide columns", "", true) {
            let (a, b) = self.selected_lines(true);
            self.hide_lines(maix, sheet, true, a, b, true);
        }
        if item(ui, "Unhide rows", "", true) {
            let (a, b) = self.selected_lines(false);
            self.hide_lines(maix, sheet, false, a, b, false);
        }
        if item(ui, "Unhide columns", "", true) {
            let (a, b) = self.selected_lines(true);
            self.hide_lines(maix, sheet, true, a, b, false);
        }
    }

    /// Fits what the selection covers: all its columns or rows when whole ones are selected.
    fn fit_selection(&mut self, ui: &egui::Ui, maix: &mut Maix, sheet: u16, cols: bool, idx: u32) {
        let sel = self.sel();
        let (a, b) = self.selected_lines(cols);
        if (cols && !whole_cols(sel)) || (!cols && !whole_rows(sel)) {
            // A cell range: fit each of its lines in turn.
            for line in a..=b.min(a + 200) {
                self.autofit(ui, maix, sheet, cols, line);
            }
        } else {
            self.autofit(ui, maix, sheet, cols, idx);
        }
    }

    fn data_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        if item(ui, "Sort A → Z", "", true) {
            self.sort_selection(maix, sheet, true, None, None);
        }
        if item(ui, "Sort Z → A", "", true) {
            self.sort_selection(maix, sheet, false, None, None);
        }
        if item(ui, "Sort range…", "", true) {
            self.prompt = Some(Prompt::Sort { key: self.focus.1, ascending: true, header: false });
        }
        ui.separator();
        let filtered = self.has_filter(sheet);
        if item(ui, if filtered { "Remove filter" } else { "Create a filter" }, "Ctrl+Shift+L", true) {
            self.toggle_filter(maix, sheet);
        }
        if item(ui, "Compare vectors…", "", true) {
            self.open_compare(maix);
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        if item(ui, "Freeze panes at the active cell", "", true) {
            self.freeze(maix, sheet, self.focus.0, self.focus.1);
        }
        if item(ui, "Freeze top row", "", true) {
            self.freeze(maix, sheet, 1, 0);
        }
        if item(ui, "Freeze first column", "", true) {
            self.freeze(maix, sheet, 0, 1);
        }
        if item(ui, "Unfreeze panes", "", true) {
            self.freeze(maix, sheet, 0, 0);
        }
        ui.separator();
        ui.checkbox(&mut self.gridlines, "Gridlines");
        ui.menu_button("Zoom", |ui| {
            for z in [0.5_f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
                if ui.selectable_label((self.zoom - z).abs() < 0.01, format!("{}%", (z * 100.0).round())).clicked() {
                    self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
                    ui.close();
                }
            }
        });
    }

    // ---- toolbar

    #[allow(clippy::too_many_lines)]
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64) {
        let cur = maix.sheet(sheet).map(|s| s.format.style_at(self.focus.0, self.focus.1)).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                self.undo_step(maix, now);
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
                self.redo_step(maix, now);
            }
            ui.separator();
            egui::ComboBox::from_id_salt("data_numfmt").width(96.0).selected_text(numfmt::name_of(&cur.numfmt)).show_ui(ui, |ui| {
                for (name, code) in numfmt::PRESETS {
                    if ui.selectable_label(cur.numfmt == *code, *name).clicked() {
                        let code = (*code).to_string();
                        self.restyle_selection(maix, sheet, &move |s| CellStyle { numfmt: code.clone(), ..s.clone() });
                    }
                }
                if ui.selectable_label(false, "Custom…").clicked() {
                    self.prompt = Some(Prompt::NumFmt(cur.numfmt.clone()));
                }
            });
            if ui.small_button("−.0").on_hover_text("Fewer decimals").clicked() {
                let code = numfmt::with_decimals(&cur.numfmt, -1);
                self.restyle_selection(maix, sheet, &move |s| CellStyle { numfmt: code.clone(), ..s.clone() });
            }
            if ui.small_button("+.0").on_hover_text("More decimals").clicked() {
                let code = numfmt::with_decimals(&cur.numfmt, 1);
                self.restyle_selection(maix, sheet, &move |s| CellStyle { numfmt: code.clone(), ..s.clone() });
            }
            ui.separator();
            egui::ComboBox::from_id_salt("data_font_size").width(44.0).selected_text(if cur.size == 0 { "11".to_string() } else { cur.size.to_string() }).show_ui(ui, |ui| {
                for size in FONT_SIZES {
                    if ui.selectable_label(cur.size == size || (cur.size == 0 && size == 11), size.to_string()).clicked() {
                        let stored = if size == 11 { 0 } else { size };
                        self.restyle_selection(maix, sheet, &move |s| CellStyle { size: stored, ..s.clone() });
                    }
                }
            });
            for (text, on, which, tip) in [
                (RichText::new("B").strong(), cur.bold, Toggle::Bold, "Bold (Ctrl+B)"),
                (RichText::new("I").italics(), cur.italic, Toggle::Italic, "Italic (Ctrl+I)"),
                (RichText::new("U").underline(), cur.underline, Toggle::Underline, "Underline (Ctrl+U)"),
                (RichText::new("S").strikethrough(), cur.strike, Toggle::Strike, "Strikethrough"),
            ] {
                if ui.selectable_label(on, text).on_hover_text(tip).clicked() {
                    self.toggle(maix, sheet, which);
                }
            }
            ui.separator();
            if let Some(c) = swatches(ui, RichText::new("A").strong().underline(), "Text colour", "Automatic", cur.color) {
                self.restyle_selection(maix, sheet, &move |s| CellStyle { color: c, ..s.clone() });
            }
            if let Some(c) = swatches(ui, RichText::new("Fill"), "Fill colour", "No fill", cur.fill) {
                self.restyle_selection(maix, sheet, &move |s| CellStyle { fill: c, ..s.clone() });
            }
            ui.separator();
            for (text, a, tip) in [("L", HAlign::Left, "Align left"), ("C", HAlign::Center, "Centre"), ("R", HAlign::Right, "Align right")] {
                if ui.selectable_label(cur.align == a, text).on_hover_text(tip).clicked() {
                    let target = if cur.align == a { HAlign::General } else { a };
                    self.restyle_selection(maix, sheet, &move |s| CellStyle { align: target, ..s.clone() });
                }
            }
            if ui.selectable_label(cur.wrap, "Wrap").on_hover_text("Wrap text").clicked() {
                self.toggle(maix, sheet, Toggle::Wrap);
            }
        });
    }

    // ---- context menus

    pub(super) fn context_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        ui.set_min_width(210.0);
        let sel = self.sel();
        match self.ctx_hit {
            Some(Hit::Column(c)) => {
                let (a, b) = self.selected_lines(true);
                let n = b - a + 1;
                self.clipboard_items(ui, maix, sheet, now, batch);
                ui.separator();
                if item(ui, &format!("Insert {n} column(s) left"), "", true) {
                    self.insert_lines(maix, sheet, Axis::Cols, a, n);
                }
                if item(ui, &format!("Insert {n} column(s) right"), "", true) {
                    self.insert_lines(maix, sheet, Axis::Cols, b + 1, n);
                }
                if item(ui, "Delete columns", "", true) {
                    self.delete_lines(maix, sheet, Axis::Cols, a, n);
                }
                if item(ui, "Clear contents", "Del", true) {
                    clear(maix, sheet, sel, now, batch);
                }
                ui.separator();
                if item(ui, "Column width…", "", true) {
                    let value = self.current_size(maix, sheet, true);
                    self.prompt = Some(Prompt::Size { cols: true, value });
                }
                if item(ui, "Fit width to content", "", true) {
                    self.fit_selection(ui, maix, sheet, true, a);
                }
                if item(ui, "Hide", "", true) {
                    self.hide_lines(maix, sheet, true, a, b, true);
                }
                if item(ui, "Unhide", "", true) {
                    self.hide_lines(maix, sheet, true, a, b, false);
                }
                ui.separator();
                if item(ui, "Sort A → Z by this column", "", true) {
                    self.sort_selection_by(maix, sheet, true, c);
                }
                if item(ui, "Sort Z → A by this column", "", true) {
                    self.sort_selection_by(maix, sheet, false, c);
                }
                if item(ui, &format!("Freeze up to column {}", column_name(c)), "", true) {
                    self.freeze(maix, sheet, 0, c + 1);
                }
            }
            Some(Hit::Row(r)) => {
                let (a, b) = self.selected_lines(false);
                let n = b - a + 1;
                self.clipboard_items(ui, maix, sheet, now, batch);
                ui.separator();
                if item(ui, &format!("Insert {n} row(s) above"), "", true) {
                    self.insert_lines(maix, sheet, Axis::Rows, a, n);
                }
                if item(ui, &format!("Insert {n} row(s) below"), "", true) {
                    self.insert_lines(maix, sheet, Axis::Rows, b + 1, n);
                }
                if item(ui, "Delete rows", "", true) {
                    self.delete_lines(maix, sheet, Axis::Rows, a, n);
                }
                if item(ui, "Clear contents", "Del", true) {
                    clear(maix, sheet, sel, now, batch);
                }
                ui.separator();
                if item(ui, "Row height…", "", true) {
                    let value = self.current_size(maix, sheet, false);
                    self.prompt = Some(Prompt::Size { cols: false, value });
                }
                if item(ui, "Fit height to content", "", true) {
                    self.fit_selection(ui, maix, sheet, false, a);
                }
                if item(ui, "Hide", "", true) {
                    self.hide_lines(maix, sheet, false, a, b, true);
                }
                if item(ui, "Unhide", "", true) {
                    self.hide_lines(maix, sheet, false, a, b, false);
                }
                ui.separator();
                if item(ui, &format!("Freeze up to row {}", r + 1), "", true) {
                    self.freeze(maix, sheet, r + 1, 0);
                }
            }
            Some(Hit::Corner) => {
                if item(ui, "Unhide all rows", "", true) {
                    self.hide_lines(maix, sheet, false, 0, MAX_ROWS - 1, false);
                }
                if item(ui, "Unhide all columns", "", true) {
                    self.hide_lines(maix, sheet, true, 0, u32::from(MAX_COLS) - 1, false);
                }
                if item(ui, "Reset all sizes", "", true) {
                    self.edit_format(maix, sheet, |f| {
                        f.layout.row_heights.clear();
                        f.layout.col_widths.clear();
                    });
                }
                if item(ui, "Unfreeze panes", "", true) {
                    self.freeze(maix, sheet, 0, 0);
                }
            }
            _ => {
                self.clipboard_items(ui, maix, sheet, now, batch);
                ui.separator();
                ui.menu_button("Insert", |ui| self.insert_menu(ui, maix, sheet, now));
                if item(ui, "Delete rows", "", true) {
                    self.delete_lines(maix, sheet, Axis::Rows, sel.0, sel.2 - sel.0 + 1);
                }
                if item(ui, "Delete columns", "", true) {
                    self.delete_lines(maix, sheet, Axis::Cols, u32::from(sel.1), u32::from(sel.3 - sel.1) + 1);
                }
                self.clear_items(ui, maix, sheet, now, batch);
                ui.separator();
                ui.menu_button("Sort", |ui| self.data_menu(ui, maix, sheet));
                ui.menu_button("Format", |ui| self.format_menu(ui, maix, sheet));
                if item(ui, "Format cells…", "", true) {
                    let current = maix.sheet(sheet).map(|s| s.format.style_at(self.focus.0, self.focus.1).numfmt).unwrap_or_default();
                    self.prompt = Some(Prompt::NumFmt(current));
                }
                ui.separator();
                if item(ui, "Row height…", "", true) {
                    let value = self.current_size(maix, sheet, false);
                    self.prompt = Some(Prompt::Size { cols: false, value });
                }
                if item(ui, "Column width…", "", true) {
                    let value = self.current_size(maix, sheet, true);
                    self.prompt = Some(Prompt::Size { cols: true, value });
                }
            }
        }
    }

    /// Sorts the selection (or the used area) by one column.
    fn sort_selection_by(&mut self, maix: &mut Maix, sheet: u16, ascending: bool, col: u16) {
        let sel = self.sel();
        if cell_count(sel) <= 1 || whole_cols(sel) {
            // Whole columns selected: sort all the used rows of the sheet.
            self.anchor = (0, 0);
            self.focus = (0, 0);
        }
        self.sort_selection(maix, sheet, ascending, Some(col), None);
    }

    pub(super) fn sheet_tab_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix, id: u16, name: &str) {
        if item(ui, "Rename", "", true) {
            self.rename = Some((id, name.to_string()));
        }
        if item(ui, "Duplicate", "", true)
            && let Some(copy) = maix.duplicate_sheet(id)
        {
            self.switch_to(copy);
        }
        let position = maix.sheets.iter().position(|s| s.id == id).unwrap_or(0);
        if item(ui, "Move left", "", position > 0) {
            maix.move_sheet(id, -1);
        }
        if item(ui, "Move right", "", position + 1 < maix.sheets.len()) {
            maix.move_sheet(id, 1);
        }
        ui.separator();
        if item(ui, "Insert new sheet", "", true) {
            self.add_sheet(maix);
        }
        if item(ui, "Delete…", "", maix.sheets.len() > 1) {
            self.prompt = Some(Prompt::DeleteSheet(id));
        }
    }

    // ---- dialogs

    #[allow(clippy::too_many_lines)]
    pub(super) fn prompt_window(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        let Some(mut prompt) = self.prompt.take() else { return };
        let title = match &prompt {
            Prompt::GoTo(_) => "Go to",
            Prompt::Size { cols: true, .. } => "Column width",
            Prompt::Size { .. } => "Row height",
            Prompt::NumFmt(_) => "Number format",
            Prompt::Sort { .. } => "Sort range",
            Prompt::DeleteSheet(_) => "Delete sheet",
            Prompt::PasteSpecial(_) => "Paste special",
        };
        let sel = self.sel();
        let focus_value = value_at(maix, sheet, self.focus.0, self.focus.1);
        let sheet_name = if let Prompt::DeleteSheet(id) = &prompt { maix.sheet(*id).map(|s| s.name.clone()).unwrap_or_default() } else { String::new() };
        let (mut ok, mut cancel, mut reset) = (false, false, false);
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match &mut prompt {
                Prompt::GoTo(text) => {
                    ui.label("A cell or range, like B7, A1:D20, C:C or 3:5");
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(200.0));
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        ok = true;
                    }
                }
                Prompt::Size { cols, value } => {
                    let max = if *cols { MAX_COL_W } else { MAX_ROW_H };
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(value).range(MIN_SIZE..=max).speed(1.0).suffix(" pt"));
                        ui.weak(if *cols { "default 96" } else { "default 24" });
                    });
                }
                Prompt::NumFmt(code) => {
                    ui.label("Format code (as in Excel: #,##0.00  0.0%  yyyy-mm-dd  [Red]-0)");
                    ui.add(egui::TextEdit::singleline(code).desired_width(260.0));
                    let preview = numfmt::display(&focus_value, code).map_or_else(|| alfadoc_bridge::textcell::to_text(&focus_value), |s| s.text);
                    ui.horizontal(|ui| {
                        ui.weak("Preview, active cell:");
                        ui.monospace(preview);
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (name, c) in numfmt::PRESETS {
                            if ui.small_button(name.split("  ").next().unwrap_or(name)).clicked() {
                                *code = (*c).to_string();
                            }
                        }
                    });
                }
                Prompt::Sort { key, ascending, header } => {
                    ui.horizontal(|ui| {
                        ui.label("Sort by column");
                        egui::ComboBox::from_id_salt("sort_key").selected_text(column_name(*key)).show_ui(ui, |ui| {
                            for c in sel.1..=sel.3.min(sel.1.saturating_add(200)) {
                                ui.selectable_value(key, c, column_name(c));
                            }
                        });
                    });
                    ui.radio_value(ascending, true, "A → Z, smallest first");
                    ui.radio_value(ascending, false, "Z → A, largest first");
                    ui.checkbox(header, "The first row holds titles");
                    ui.weak("With one cell selected the whole used area is sorted. Formulas stay with their rows.");
                }
                Prompt::PasteSpecial(opts) => {
                    ui.label("Paste what was copied in this window:");
                    ui.radio_value(&mut opts.kind, PasteKind::All, "Everything (values, formulas, formatting)");
                    ui.radio_value(&mut opts.kind, PasteKind::Values, "Values only");
                    ui.radio_value(&mut opts.kind, PasteKind::Formulas, "Formulas only");
                    ui.radio_value(&mut opts.kind, PasteKind::Formats, "Formatting only");
                    ui.separator();
                    ui.checkbox(&mut opts.skip_blanks, "Skip blanks (empty copied cells keep what is there)");
                    ui.checkbox(&mut opts.transpose, "Transpose (rows become columns)");
                }
                Prompt::DeleteSheet(_) => {
                    ui.label(format!("Delete the sheet “{sheet_name}” and everything on it?"));
                    ui.weak("Links that point at it stop working. This cannot be undone.");
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                let label = if matches!(prompt, Prompt::DeleteSheet(_)) { "Delete" } else { "OK" };
                if ui.button(label).clicked() {
                    ok = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
                if matches!(prompt, Prompt::Size { .. }) && ui.button("Default").clicked() {
                    reset = true;
                }
            });
        });
        if cancel {
            return;
        }
        if !ok && !reset {
            self.prompt = Some(prompt);
            return;
        }
        match prompt {
            Prompt::GoTo(text) => {
                if let Some((a, f)) = parse_address(&text) {
                    self.anchor = a;
                    self.focus = f;
                    self.scroll_to_focus = true;
                } else {
                    self.say("That is not a cell or range.");
                    self.prompt = Some(Prompt::GoTo(text));
                }
            }
            Prompt::Size { cols, value } => {
                let (a, b) = self.selected_lines(cols);
                self.set_size(maix, sheet, cols, a, b, if reset { None } else { Some(value) });
            }
            Prompt::NumFmt(code) => {
                self.restyle_selection(maix, sheet, &move |s| CellStyle { numfmt: code.clone(), ..s.clone() });
            }
            Prompt::Sort { key, ascending, header } => {
                self.sort_selection(maix, sheet, ascending, Some(key), Some(header));
            }
            Prompt::PasteSpecial(opts) => self.paste_with(maix, sheet, opts, now, batch),
            Prompt::DeleteSheet(id) => {
                if maix.delete_sheet(id) {
                    if let Some(first) = maix.sheets.first().map(|s| s.id) {
                        self.switch_to(first);
                    }
                    self.undo.clear();
                    self.redo.clear();
                    self.say(format!("Deleted “{sheet_name}”."));
                }
            }
        }
    }
}
