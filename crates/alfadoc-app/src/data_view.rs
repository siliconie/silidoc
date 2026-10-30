//! The **Data** tab: the package's shared maix store as an editable grid.
//!
//! Anything linked to a Data range (see `linking.rs`) appears here, and a value typed here flows
//! out to every sheet, document table and slide table linked to the same cells. Values are typed
//! the same way as in tables: `12` is a number, `007` stays text, `true` is a boolean, and
//! `2026-03-01` is a date.
//!
//! The grid is drawn by hand and only for the cells in view, so a store with thousands of rows
//! stays fast. It works like a spreadsheet: click or use the arrow keys to select, type to
//! replace, `F2` or double-click to edit, `Enter`/`Tab` to move, `Delete` to clear, drag or
//! `Shift`+arrows to select a range, `Ctrl`+`C`/`X`/`V` to copy, cut and paste (tab-separated, so
//! it pastes to and from Excel), `Ctrl`+`Z`/`Y` to undo and redo.

use alfadoc_bridge::textcell;
use alfadoc_model::{DimVector, Maix, Stamp, Value};
use egui::{Align2, Color32, Event, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};

const ROW_H: f32 = 24.0;
const COL_W: f32 = 96.0;
const HEADER_W: f32 = 54.0;
const HEADER_H: f32 = 26.0;
const MIN_ROWS: u32 = 200;
const MIN_COLS: u16 = 26;
const MAX_ROWS: u32 = 100_000;
const MAX_COLS: u16 = 200;
const PAGE_ROWS: u32 = 20;
/// The most cells one paste, fill or clear touches.
const MAX_BATCH: usize = 200_000;
/// Undo steps kept.
const MAX_UNDO: usize = 200;
/// Who the store records as the author of edits made in this tab.
const AUTHOR: &str = "alfadoc-user";

type Cell = (u32, u16);

/// One cell change, kept so it can be undone.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    sheet: u16,
    row: u32,
    col: u16,
    /// The dimension vector the cell was written under.
    dims: DimVector,
    old: Content,
    new: Content,
}

/// What a cell holds as far as editing is concerned: a typed value, or a formula (its value is
/// worked out by the store).
#[derive(Clone, Debug, PartialEq)]
enum Content {
    Value(Value),
    Formula(String),
}

/// Where a write lands.
struct Spot {
    sheet: u16,
    cell: Cell,
    dims: DimVector,
}

struct Edit {
    cell: Cell,
    text: String,
    /// The text box has not been given the keyboard yet.
    fresh: bool,
}

pub struct DataView {
    sheet: Option<u16>,
    anchor: Cell,
    focus: Cell,
    edit: Option<Edit>,
    /// Text typed in the bar above the grid, before it is applied.
    bar: Option<String>,
    undo: Vec<Vec<Change>>,
    redo: Vec<Vec<Change>>,
    /// (sheet, history length, extent): the used area, recomputed only after the store changed.
    extent: Option<(u16, usize, (u32, u16))>,
    rename: Option<(u16, String)>,
    scroll_to_focus: bool,
    grab_focus: bool,
    /// Set by undo and redo, which change formulas without touching the history.
    needs_calc: bool,
    /// The "new dimension" boxes of the Dimensions menu.
    dim_key: String,
    dim_value: String,
    /// Ranges (sheet, top, left, bottom, right) that links keep in step; the shell sets them
    /// before each frame and they are drawn tinted.
    pub linked: Vec<(u16, (u32, u16, u32, u16))>,
}

impl Default for DataView {
    fn default() -> Self {
        DataView {
            sheet: None,
            anchor: (0, 0),
            focus: (0, 0),
            edit: None,
            bar: None,
            undo: vec![],
            redo: vec![],
            extent: None,
            rename: None,
            scroll_to_focus: false,
            grab_focus: false,
            needs_calc: false,
            dim_key: String::new(),
            dim_value: String::new(),
            linked: vec![],
        }
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

fn address(cell: Cell) -> String {
    format!("{}{}", column_name(cell.1), cell.0 + 1)
}

fn shown(maix: &Maix, sheet: u16, row: u32, col: u16) -> String {
    match maix.get(sheet, row, col, &maix.active_dims) {
        Ok(Some(v)) if *v != Value::Null => textcell::to_text(v),
        _ => String::new(),
    }
}

/// What to put in the edit box: the formula if the cell has one, otherwise what it shows.
fn input_text(maix: &Maix, sheet: u16, row: u32, col: u16) -> String {
    match maix.formula(sheet, row, col, &maix.active_dims) {
        Some(f) => f.to_string(),
        None => shown(maix, sheet, row, col),
    }
}

fn value_at(maix: &Maix, sheet: u16, row: u32, col: u16) -> Value {
    maix.get(sheet, row, col, &maix.active_dims).ok().flatten().cloned().unwrap_or(Value::Null)
}

fn content_of(maix: &Maix, spot: &Spot) -> Content {
    match maix.formula(spot.sheet, spot.cell.0, spot.cell.1, &spot.dims) {
        Some(f) => Content::Formula(f.to_string()),
        None => Content::Value(maix.get(spot.sheet, spot.cell.0, spot.cell.1, &spot.dims).ok().flatten().cloned().unwrap_or(Value::Null)),
    }
}

/// Writes `content` at `spot`; the change is added to `batch` when something actually changed.
fn write(maix: &mut Maix, spot: &Spot, content: Content, now: u64, batch: &mut Vec<Change>) {
    let old = content_of(maix, spot);
    if old == content {
        return;
    }
    let (sheet, row, col) = (spot.sheet, spot.cell.0, spot.cell.1);
    let stamp = Stamp::new(AUTHOR, now);
    let done = match &content {
        Content::Value(v) => {
            let cleared = maix.set_formula(sheet, row, col, &spot.dims, None).unwrap_or(false);
            let set = maix.set(sheet, row, col, &spot.dims, v.clone(), &stamp).unwrap_or(false);
            cleared || set
        }
        Content::Formula(f) => maix.set_formula(sheet, row, col, &spot.dims, Some(f)).unwrap_or(false),
    };
    if done {
        batch.push(Change { sheet, row, col, dims: spot.dims.clone(), old, new: content });
    }
}

/// Text typed by a person: `=…` is a formula, anything else a typed value.
fn content_from(text: &str) -> Content {
    let t = text.trim();
    if t.len() > 1 && t.starts_with('=') { Content::Formula(t.to_string()) } else { Content::Value(parse_input(text)) }
}

/// Text typed by a person → the value stored.
fn parse_input(text: &str) -> Value {
    if text.trim().is_empty() { Value::Null } else { textcell::from_text(text) }
}

/// Writes `value`; the change is added to `batch` when something actually changed.
fn put(maix: &mut Maix, sheet: u16, cell: Cell, value: Value, now: u64, batch: &mut Vec<Change>) {
    let spot = Spot { sheet, cell, dims: maix.active_dims.clone() };
    write(maix, &spot, Content::Value(value), now, batch);
}

/// Like [`put`], for text as typed: a formula when it starts with `=`.
fn put_input(maix: &mut Maix, sheet: u16, cell: Cell, text: &str, now: u64, batch: &mut Vec<Change>) {
    let spot = Spot { sheet, cell, dims: maix.active_dims.clone() };
    write(maix, &spot, content_from(text), now, batch);
}

/// The selected rectangle as (top, left, bottom, right), both ends included.
fn normalized(a: Cell, b: Cell) -> (u32, u16, u32, u16) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn cell_count(range: (u32, u16, u32, u16)) -> usize {
    let rows = (range.2 - range.0) as usize + 1;
    let cols = (range.3 - range.1) as usize + 1;
    rows.saturating_mul(cols)
}

/// The cells of `range` as tab-separated text, one line per row.
fn to_tsv(maix: &Maix, sheet: u16, range: (u32, u16, u32, u16)) -> String {
    let mut out = String::new();
    for r in range.0..=range.2 {
        if r > range.0 {
            out.push('\n');
        }
        for c in range.1..=range.3 {
            if c > range.1 {
                out.push('\t');
            }
            out.push_str(&shown(maix, sheet, r, c));
        }
    }
    out
}

/// Pastes tab-separated text with its top-left corner at `at`. A single value pasted over a
/// larger selection fills the selection.
fn paste(maix: &mut Maix, sheet: u16, at: Cell, selection: (u32, u16, u32, u16), text: &str, now: u64, batch: &mut Vec<Change>) {
    let text = text.strip_suffix('\n').unwrap_or(text).replace('\r', "");
    let rows: Vec<Vec<&str>> = text.split('\n').map(|line| line.split('\t').collect()).collect();
    let single = rows.len() == 1 && rows[0].len() == 1;
    if single && cell_count(selection) > 1 {
        if cell_count(selection) > MAX_BATCH {
            return;
        }
        for r in selection.0..=selection.2 {
            for c in selection.1..=selection.3 {
                put_input(maix, sheet, (r, c), rows[0][0], now, batch);
            }
        }
        return;
    }
    if rows.iter().map(Vec::len).sum::<usize>() > MAX_BATCH {
        return;
    }
    for (dr, line) in rows.iter().enumerate() {
        for (dc, cell) in line.iter().enumerate() {
            let (Ok(dr), Ok(dc)) = (u32::try_from(dr), u16::try_from(dc)) else { continue };
            let (row, col) = (at.0.saturating_add(dr), at.1.saturating_add(dc));
            if row < MAX_ROWS && col < MAX_COLS {
                put_input(maix, sheet, (row, col), cell, now, batch);
            }
        }
    }
}

fn clear(maix: &mut Maix, sheet: u16, range: (u32, u16, u32, u16), now: u64, batch: &mut Vec<Change>) {
    if cell_count(range) > MAX_BATCH {
        return;
    }
    for r in range.0..=range.2 {
        for c in range.1..=range.3 {
            put(maix, sheet, (r, c), Value::Null, now, batch);
        }
    }
}

/// Applies `changes` backwards (undo) or forwards (redo). Returns the changes made, as written.
fn replay(maix: &mut Maix, changes: &[Change], backwards: bool, now: u64) -> Vec<Change> {
    let mut done = vec![];
    let order: Vec<&Change> = if backwards { changes.iter().rev().collect() } else { changes.iter().collect() };
    for c in order {
        let content = if backwards { c.old.clone() } else { c.new.clone() };
        let spot = Spot { sheet: c.sheet, cell: (c.row, c.col), dims: c.dims.clone() };
        write(maix, &spot, content, now, &mut done);
    }
    done
}

/// Count, sum and average of the numbers in `range`.
fn stats(maix: &Maix, sheet: u16, range: (u32, u16, u32, u16)) -> (usize, usize, f64) {
    let (mut filled, mut numbers, mut sum) = (0usize, 0usize, 0.0f64);
    if cell_count(range) > MAX_BATCH {
        return (0, 0, 0.0);
    }
    for r in range.0..=range.2 {
        for c in range.1..=range.3 {
            match value_at(maix, sheet, r, c) {
                Value::Null => {}
                Value::Int(i) => {
                    filled += 1;
                    numbers += 1;
                    sum += i as f64;
                }
                Value::Float(f) => {
                    filled += 1;
                    numbers += 1;
                    sum += f;
                }
                _ => filled += 1,
            }
        }
    }
    (filled, numbers, sum)
}

struct Palette {
    cell: Color32,
    header: Color32,
    header_text: Color32,
    line: Color32,
    text: Color32,
    select_fill: Color32,
    select_line: Color32,
}

impl Palette {
    fn of(ui: &egui::Ui) -> Palette {
        let v = ui.visuals();
        Palette {
            cell: v.extreme_bg_color,
            header: v.widgets.inactive.weak_bg_fill,
            header_text: v.weak_text_color(),
            line: v.widgets.noninteractive.bg_stroke.color,
            text: v.text_color(),
            select_fill: v.selection.bg_fill.gamma_multiply(0.35),
            select_line: v.selection.stroke.color,
        }
    }
}

enum Hit {
    Cell(Cell),
    Column(u16),
    Row(u32),
    Corner,
}

impl DataView {
    /// The sheet shown and the selected rectangle (top, left, bottom, right).
    pub fn selection(&self) -> Option<(u16, (u32, u16, u32, u16))> {
        self.sheet.map(|s| (s, normalized(self.anchor, self.focus)))
    }

    /// The used area of `sheet`, cached until the store changes.
    fn used(&mut self, maix: &Maix, sheet: u16) -> (u32, u16) {
        let stamp = maix.history.len();
        if let Some((s, h, e)) = self.extent
            && s == sheet
            && h == stamp
        {
            return e;
        }
        let e = maix.sheet(sheet).map_or((0, 0), alfadoc_model::maix::Sheet::extent);
        self.extent = Some((sheet, stamp, e));
        e
    }

    /// Draws the tab. Returns true when the user changed a value.
    pub fn show(&mut self, ui: &mut egui::Ui, maix: &mut Maix, now_ms: u64) -> bool {
        if self.sheet.is_none_or(|id| maix.sheet(id).is_err()) {
            self.sheet = maix.sheets.first().map(|s| s.id);
            self.anchor = (0, 0);
            self.focus = (0, 0);
            self.edit = None;
        }
        let Some(sheet) = self.sheet else {
            let mut changed = false;
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("The shared data store is empty");
                ui.label("Add a sheet to type data here, or link a range to Data from the Links window.");
                if ui.button("Add a sheet").clicked() {
                    self.sheet = Some(maix.add_sheet("Sheet1"));
                    changed = true;
                }
            });
            return changed;
        };
        let mut batch: Vec<Change> = vec![];
        let history_before = maix.history.len();
        self.bar_row(ui, maix, sheet, now_ms, &mut batch);
        let footer = 32.0;
        let grid_height = (ui.available_height() - footer).max(80.0);
        self.grid(ui, maix, sheet, now_ms, grid_height, &mut batch);
        self.footer(ui, maix, sheet);
        let mut changed = false;
        if !batch.is_empty() || self.needs_calc || maix.history.len() != history_before {
            maix.recalc(&Stamp::new(AUTHOR, now_ms));
            self.needs_calc = false;
        }
        if !batch.is_empty() {
            self.undo.push(batch);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
            changed = true;
        }
        changed
    }

    /// The address box and the value bar above the grid.
    fn bar_row(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        let range = normalized(self.anchor, self.focus);
        let name = if cell_count(range) > 1 { format!("{}:{}", address((range.0, range.1)), address((range.2, range.3))) } else { address(self.focus) };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.add_sized([86.0, 22.0], egui::Label::new(egui::RichText::new(name).monospace().strong()));
            ui.label(egui::RichText::new("fx").weak().italics());
            if let Some(edit) = &self.edit {
                let mut mirror = edit.text.clone();
                ui.add(egui::TextEdit::singleline(&mut mirror).desired_width(f32::INFINITY).interactive(false));
                return;
            }
            let mut text = self.bar.clone().unwrap_or_else(|| input_text(maix, sheet, self.focus.0, self.focus.1));
            let response = ui.add(egui::TextEdit::singleline(&mut text).id_salt("data_value_bar").desired_width(f32::INFINITY).hint_text("Type a value, then press Enter"));
            if response.changed() || response.has_focus() {
                self.bar = Some(text.clone());
            }
            if response.lost_focus() {
                let escaped = ui.input(|i| i.key_pressed(Key::Escape));
                if !escaped {
                    put_input(maix, sheet, self.focus, &text, now, batch);
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.step(1, 0, false);
                    }
                }
                self.bar = None;
                self.grab_focus = true;
            }
        });
        ui.add_space(2.0);
    }

    /// Moves the focus cell (and the anchor unless `extend`), keeping it on the sheet.
    fn step(&mut self, dr: i64, dc: i64, extend: bool) {
        let max_row = MAX_ROWS - 1;
        let max_col = MAX_COLS - 1;
        let row = (i64::from(self.focus.0) + dr).clamp(0, i64::from(max_row));
        let col = (i64::from(self.focus.1) + dc).clamp(0, i64::from(max_col));
        self.focus = (u32::try_from(row).unwrap_or(0), u16::try_from(col).unwrap_or(0));
        if !extend {
            self.anchor = self.focus;
        }
        self.scroll_to_focus = true;
    }

    fn begin_edit(&mut self, maix: &Maix, sheet: u16, replace_with: Option<String>) {
        let text = replace_with.unwrap_or_else(|| input_text(maix, sheet, self.focus.0, self.focus.1));
        self.edit = Some(Edit { cell: self.focus, text, fresh: true });
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    fn grid(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, height: f32, batch: &mut Vec<Change>) {
        let pal = Palette::of(ui);
        let used = self.used(maix, sheet);
        let rows = MIN_ROWS.max(used.0.saturating_add(60)).max(self.focus.0.saturating_add(40)).min(MAX_ROWS);
        let cols = MIN_COLS.max(used.1.saturating_add(8)).max(self.focus.1.saturating_add(8)).min(MAX_COLS);
        let size = Vec2::new(HEADER_W + f32::from(cols) * COL_W, HEADER_H + rows as f32 * ROW_H);
        let font = FontId::proportional(13.5);
        let header_font = FontId::proportional(12.0);

        egui::ScrollArea::both().id_salt(("data_scroll", sheet)).max_height(height).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let origin = rect.min;
            let grid_id = response.id;
            let vis = Rect::from_min_size(origin + viewport.min.to_vec2(), viewport.size());
            let painter = ui.painter_at(vis);
            let cell_rect = |r: u32, c: u16| Rect::from_min_size(origin + Vec2::new(HEADER_W + f32::from(c) * COL_W, HEADER_H + r as f32 * ROW_H), Vec2::new(COL_W, ROW_H));

            let first_row = ((viewport.min.y - HEADER_H) / ROW_H).floor().max(0.0) as u32;
            let last_row = ((((viewport.max.y - HEADER_H) / ROW_H).ceil() as i64) + 1).clamp(0, i64::from(rows)) as u32;
            let first_col = ((viewport.min.x - HEADER_W) / COL_W).floor().max(0.0) as u16;
            let last_col = ((((viewport.max.x - HEADER_W) / COL_W).ceil() as i64) + 1).clamp(0, i64::from(cols)) as u16;

            // ---- pointer
            let hit = |pos: Pos2| -> Hit {
                let in_header_x = pos.x < vis.min.x + HEADER_W;
                let in_header_y = pos.y < vis.min.y + HEADER_H;
                let col = (((pos.x - origin.x - HEADER_W) / COL_W).floor().max(0.0) as u32).min(u32::from(cols) - 1);
                let row = (((pos.y - origin.y - HEADER_H) / ROW_H).floor().max(0.0) as u32).min(rows - 1);
                let col = u16::try_from(col).unwrap_or(0);
                match (in_header_x, in_header_y) {
                    (true, true) => Hit::Corner,
                    (false, true) => Hit::Column(col),
                    (true, false) => Hit::Row(row),
                    (false, false) => Hit::Cell((row, col)),
                }
            };
            let shift = ui.input(|i| i.modifiers.shift);
            if let Some(pos) = response.interact_pointer_pos()
                && (response.clicked() || response.drag_started() || response.dragged())
                && self.edit.as_ref().is_none_or(|e| !cell_rect(e.cell.0, e.cell.1).contains(pos))
            {
                let started = response.clicked() || response.drag_started();
                match hit(pos) {
                    Hit::Cell(cell) => {
                        self.focus = cell;
                        if started && !shift {
                            self.anchor = cell;
                        }
                    }
                    Hit::Column(c) if started => {
                        self.anchor = (0, c);
                        self.focus = (rows - 1, c);
                    }
                    Hit::Row(r) if started => {
                        self.anchor = (r, 0);
                        self.focus = (r, cols - 1);
                    }
                    Hit::Corner if started => {
                        self.anchor = (0, 0);
                        self.focus = (rows - 1, cols - 1);
                    }
                    _ => {}
                }
                if started {
                    ui.memory_mut(|m| m.request_focus(grid_id));
                    self.bar = None;
                }
            }
            if response.double_clicked()
                && let Some(Hit::Cell(cell)) = response.interact_pointer_pos().map(hit)
            {
                self.focus = cell;
                self.anchor = cell;
                self.begin_edit(maix, sheet, None);
            }
            // The grid is one focusable thing; give it the keys the arrows and Tab would use to
            // move between widgets.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(grid_id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false });
            });
            if self.grab_focus {
                ui.memory_mut(|m| m.request_focus(grid_id));
                self.grab_focus = false;
            }

            // ---- background and cells
            painter.rect_filled(vis, 0.0, pal.cell);
            for (s, r) in &self.linked {
                if *s == sheet {
                    let area = Rect::from_min_max(cell_rect(r.0, r.1).min, cell_rect(r.2, r.3).max).intersect(vis);
                    if area.is_positive() {
                        painter.rect_filled(area, 0.0, pal.select_line.gamma_multiply(0.12));
                    }
                }
            }
            let stroke = Stroke::new(1.0, pal.line);
            for r in first_row..last_row {
                let y = origin.y + HEADER_H + r as f32 * ROW_H;
                painter.hline(vis.x_range(), y + ROW_H, stroke);
                for c in first_col..last_col {
                    let value = value_at(maix, sheet, r, c);
                    if value == Value::Null {
                        continue;
                    }
                    let rc = cell_rect(r, c);
                    let clipped = painter.with_clip_rect(rc.intersect(vis));
                    let text = textcell::to_text(&value);
                    match value {
                        Value::Int(_) | Value::Float(_) | Value::Date(_) => {
                            clipped.text(rc.right_center() - Vec2::new(6.0, 0.0), Align2::RIGHT_CENTER, text, font.clone(), pal.text);
                        }
                        Value::Bool(_) => {
                            clipped.text(rc.center(), Align2::CENTER_CENTER, text, font.clone(), pal.text);
                        }
                        _ => {
                            clipped.text(rc.left_center() + Vec2::new(6.0, 0.0), Align2::LEFT_CENTER, text, font.clone(), pal.text);
                        }
                    }
                }
            }
            for c in first_col..=last_col {
                let x = origin.x + HEADER_W + f32::from(c) * COL_W;
                painter.vline(x, vis.y_range(), stroke);
            }

            // ---- selection
            let sel = normalized(self.anchor, self.focus);
            let sel_rect = Rect::from_min_max(cell_rect(sel.0, sel.1).min, cell_rect(sel.2, sel.3).max);
            if cell_count(sel) > 1 {
                painter.rect_filled(sel_rect.intersect(vis), 0.0, pal.select_fill);
            }
            painter.rect_stroke(cell_rect(self.focus.0, self.focus.1), 0.0, Stroke::new(2.0, pal.select_line), egui::StrokeKind::Inside);

            // ---- headers stay in view
            painter.rect_filled(Rect::from_min_size(vis.min, Vec2::new(vis.width(), HEADER_H)), 0.0, pal.header);
            painter.rect_filled(Rect::from_min_size(vis.min, Vec2::new(HEADER_W, vis.height())), 0.0, pal.header);
            for c in first_col..last_col {
                let x = origin.x + HEADER_W + f32::from(c) * COL_W;
                let hr = Rect::from_min_size(Pos2::new(x, vis.min.y), Vec2::new(COL_W, HEADER_H));
                if c >= sel.1 && c <= sel.3 {
                    painter.rect_filled(hr, 0.0, pal.select_fill);
                }
                painter.vline(x, hr.y_range(), stroke);
                painter.text(hr.center(), Align2::CENTER_CENTER, column_name(c), header_font.clone(), pal.header_text);
            }
            for r in first_row..last_row {
                let y = origin.y + HEADER_H + r as f32 * ROW_H;
                let hr = Rect::from_min_size(Pos2::new(vis.min.x, y), Vec2::new(HEADER_W, ROW_H));
                if r >= sel.0 && r <= sel.2 {
                    painter.rect_filled(hr, 0.0, pal.select_fill);
                }
                painter.hline(hr.x_range(), y + ROW_H, stroke);
                painter.text(hr.center(), Align2::CENTER_CENTER, (r + 1).to_string(), header_font.clone(), pal.header_text);
            }
            painter.hline(vis.x_range(), vis.min.y + HEADER_H, stroke);
            painter.vline(vis.min.x + HEADER_W, vis.y_range(), stroke);

            // ---- keyboard (only while the grid, not a text box, has the keys)
            if self.edit.is_none() && ui.memory(|m| m.has_focus(grid_id)) {
                let events = ui.input(|i| i.events.clone());
                for event in events {
                    self.key_event(ui.ctx(), maix, sheet, now, &event, batch);
                }
            }

            // ---- the cell being edited
            if let Some(mut edit) = self.edit.take() {
                let rc = cell_rect(edit.cell.0, edit.cell.1);
                let widget = egui::TextEdit::singleline(&mut edit.text).font(font.clone()).margin(egui::Margin::symmetric(5, 3)).desired_width(COL_W);
                let r = ui.put(rc, widget);
                if edit.fresh {
                    r.request_focus();
                    edit.fresh = false;
                }
                let escaped = ui.input(|i| i.key_pressed(Key::Escape));
                if escaped {
                    ui.memory_mut(|m| m.request_focus(grid_id));
                } else if r.lost_focus() {
                    put_input(maix, sheet, edit.cell, &edit.text, now, batch);
                    let (enter, tab, shift) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Tab), i.modifiers.shift));
                    ui.memory_mut(|m| m.request_focus(grid_id));
                    self.focus = edit.cell;
                    self.anchor = edit.cell;
                    if enter {
                        self.step(if shift { -1 } else { 1 }, 0, false);
                    } else if tab {
                        self.step(0, if shift { -1 } else { 1 }, false);
                    }
                } else {
                    self.edit = Some(edit);
                }
            }

            if self.scroll_to_focus {
                let rc = cell_rect(self.focus.0, self.focus.1);
                ui.scroll_to_rect(Rect::from_min_max(rc.min - Vec2::new(HEADER_W, HEADER_H), rc.max), None);
                self.scroll_to_focus = false;
            }
        });
    }

    fn key_event(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16, now: u64, event: &Event, batch: &mut Vec<Change>) {
        let sel = normalized(self.anchor, self.focus);
        match event {
            Event::Copy => ctx.copy_text(to_tsv(maix, sheet, sel)),
            Event::Cut => {
                ctx.copy_text(to_tsv(maix, sheet, sel));
                clear(maix, sheet, sel, now, batch);
            }
            Event::Paste(text) => {
                paste(maix, sheet, (sel.0, sel.1), sel, text, now, batch);
                // Select what was pasted.
                let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(text).split('\n').collect();
                let width = lines.iter().map(|l| l.split('\t').count()).max().unwrap_or(1);
                if lines.len() > 1 || width > 1 {
                    let bottom = sel.0.saturating_add(u32::try_from(lines.len()).unwrap_or(1) - 1).min(MAX_ROWS - 1);
                    let right = sel.1.saturating_add(u16::try_from(width).unwrap_or(1) - 1).min(MAX_COLS - 1);
                    self.anchor = (sel.0, sel.1);
                    self.focus = (bottom, right);
                }
            }
            Event::Text(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                self.begin_edit(maix, sheet, Some(t.clone()));
            }
            Event::Key { key, pressed: true, modifiers, repeat: _, .. } => {
                let cmd = modifiers.command;
                let extend = modifiers.shift;
                match key {
                    Key::ArrowDown => self.step(1, 0, extend),
                    Key::ArrowUp => self.step(-1, 0, extend),
                    Key::ArrowLeft => self.step(0, -1, extend),
                    Key::ArrowRight => self.step(0, 1, extend),
                    Key::PageDown => self.step(i64::from(PAGE_ROWS), 0, extend),
                    Key::PageUp => self.step(-i64::from(PAGE_ROWS), 0, extend),
                    Key::Home => {
                        let col = i64::from(self.focus.1);
                        self.step(0, -col, extend);
                    }
                    Key::Tab => self.step(0, if extend { -1 } else { 1 }, false),
                    Key::Enter if !cmd => {
                        if extend {
                            self.step(-1, 0, false);
                        } else {
                            self.begin_edit(maix, sheet, None);
                        }
                    }
                    Key::F2 => self.begin_edit(maix, sheet, None),
                    Key::Delete | Key::Backspace => clear(maix, sheet, sel, now, batch),
                    Key::Escape => self.anchor = self.focus,
                    Key::A if cmd => {
                        let used = self.used(maix, sheet);
                        self.anchor = (0, 0);
                        self.focus = (used.0.saturating_sub(1), used.1.saturating_sub(1));
                    }
                    Key::Z if cmd && !extend => self.undo_step(maix, now),
                    Key::Z | Key::Y if cmd => self.redo_step(maix, now),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn undo_step(&mut self, maix: &mut Maix, now: u64) {
        if let Some(changes) = self.undo.pop() {
            replay(maix, &changes, true, now);
            self.needs_calc = true;
            if let Some(first) = changes.first() {
                self.focus = (first.row, first.col);
                self.anchor = self.focus;
                self.scroll_to_focus = true;
            }
            self.redo.push(changes);
        }
    }

    fn redo_step(&mut self, maix: &mut Maix, now: u64) {
        if let Some(changes) = self.redo.pop() {
            replay(maix, &changes, false, now);
            self.needs_calc = true;
            self.undo.push(changes);
        }
    }

    /// The dimension vector the grid shows and writes: pick a value per dimension or add one.
    fn dimension_menu(&mut self, ui: &mut egui::Ui, maix: &mut Maix) {
        let label = if maix.active_dims.is_empty() { "Dimensions".to_string() } else { format!("Dimensions: {}", alfadoc_model::dims::canonical(&maix.active_dims).replace('|', ", ")) };
        ui.menu_button(label, |ui| {
            ui.set_min_width(280.0);
            let mut known: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = std::collections::BTreeMap::new();
            for (_, vector) in maix.vectors.iter() {
                for (k, v) in vector {
                    known.entry(k.clone()).or_default().insert(v.clone());
                }
            }
            for (k, v) in &maix.active_dims {
                known.entry(k.clone()).or_default().insert(v.clone());
            }
            if known.is_empty() {
                ui.weak("No dimensions yet. Add one below, for example scenario = actual.");
            }
            for (key, values) in &known {
                let current = maix.active_dims.get(key).cloned();
                ui.horizontal(|ui| {
                    ui.label(key);
                    egui::ComboBox::from_id_salt(("dimension", key)).selected_text(current.clone().unwrap_or_else(|| "(none)".to_string())).show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "(none)").clicked() {
                            maix.active_dims.remove(key);
                        }
                        for value in values {
                            if ui.selectable_label(current.as_deref() == Some(value.as_str()), value).clicked() {
                                maix.active_dims.insert(key.clone(), value.clone());
                            }
                        }
                    });
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.dim_key).hint_text("dimension").desired_width(95.0));
                ui.add(egui::TextEdit::singleline(&mut self.dim_value).hint_text("value").desired_width(95.0));
                if ui.button("Use").clicked() && !self.dim_key.trim().is_empty() && !self.dim_value.trim().is_empty() {
                    maix.active_dims.insert(self.dim_key.trim().to_string(), self.dim_value.trim().to_string());
                    self.dim_key.clear();
                    self.dim_value.clear();
                }
            });
            ui.weak("What you type is stored under the chosen values; with none chosen it is the plain value. Formulas can read another vector: =B2@{scenario=budget}, =DIMSUM(B2:B9, \"scenario=actual\").");
        });
    }

    /// Sheet tabs on the left, selection summary on the right.
    fn footer(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16) {
        ui.separator();
        ui.horizontal(|ui| {
            let ids: Vec<(u16, String)> = maix.sheets.iter().map(|s| (s.id, s.name.clone())).collect();
            for (id, name) in ids {
                if let Some((rid, text)) = &mut self.rename
                    && *rid == id
                {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(90.0));
                    r.request_focus();
                    if r.lost_focus() {
                        if !ui.input(|i| i.key_pressed(Key::Escape)) {
                            maix.rename_sheet(id, text);
                        }
                        self.rename = None;
                    }
                    continue;
                }
                let response = ui.selectable_label(sheet == id, &name);
                if response.clicked() && sheet != id {
                    self.sheet = Some(id);
                    self.anchor = (0, 0);
                    self.focus = (0, 0);
                    self.edit = None;
                }
                response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        self.rename = Some((id, name.clone()));
                        ui.close();
                    }
                });
            }
            self.dimension_menu(ui, maix);
            if ui.button("+").on_hover_text("Add a sheet").clicked() {
                let mut n = maix.sheets.len() + 1;
                while maix.sheet_by_name(&format!("Sheet{n}")).is_some() {
                    n += 1;
                }
                self.sheet = Some(maix.add_sheet(&format!("Sheet{n}")));
                self.anchor = (0, 0);
                self.focus = (0, 0);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let sel = normalized(self.anchor, self.focus);
                let (filled, numbers, sum) = stats(maix, sheet, sel);
                let mut parts = vec![format!("{} cell(s) selected", cell_count(sel))];
                if filled > 0 {
                    parts.push(format!("Count {filled}"));
                }
                if numbers > 0 {
                    parts.push(format!("Sum {}", number(sum)));
                    parts.push(format!("Average {}", number(sum / numbers as f64)));
                }
                ui.weak(parts.join("   "));
            });
        });
    }
}

fn number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{x:.0}") } else { format!("{x:.4}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Maix, u16) {
        let mut m = Maix::default();
        let s = m.add_sheet("Costs");
        (m, s)
    }

    #[test]
    fn column_names_count_like_a_spreadsheet() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
        assert_eq!(address((2, 1)), "B3");
    }

    #[test]
    fn shown_text_follows_the_store() {
        let (mut m, s) = store();
        assert_eq!(shown(&m, s, 0, 0), "");
        m.set(s, 0, 0, &DimVector::new(), Value::Int(12), &Stamp::new("t", 1)).unwrap();
        m.set(s, 1, 0, &DimVector::new(), Value::Null, &Stamp::new("t", 2)).unwrap();
        assert_eq!(shown(&m, s, 0, 0), "12");
        assert_eq!(shown(&m, s, 1, 0), "");
    }

    #[test]
    fn input_is_typed_like_table_cells() {
        assert_eq!(parse_input("12"), Value::Int(12));
        assert_eq!(parse_input("007"), Value::Text("007".into()));
        assert_eq!(parse_input("TRUE"), Value::Bool(true));
        assert_eq!(parse_input("  "), Value::Null);
        assert!(matches!(parse_input("2026-03-01"), Value::Date(_)));
    }

    #[test]
    fn a_pasted_block_lands_at_the_corner_and_undoes_as_one_step() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (1, 1), (1, 1, 1, 1), "a\t1\nb\t2\n", 5, &mut batch);
        assert_eq!(shown(&m, s, 1, 1), "a");
        assert_eq!(shown(&m, s, 1, 2), "1");
        assert_eq!(shown(&m, s, 2, 2), "2");
        assert_eq!(batch.len(), 4);
        let undone = replay(&mut m, &batch, true, 6);
        assert_eq!(undone.len(), 4);
        assert_eq!(shown(&m, s, 1, 1), "");
        let redone = replay(&mut m, &batch, false, 7);
        assert_eq!(redone.len(), 4);
        assert_eq!(shown(&m, s, 2, 1), "b");
    }

    #[test]
    fn one_value_fills_the_whole_selection_and_copy_round_trips() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (0, 0), (0, 0, 1, 2), "7", 1, &mut batch);
        assert_eq!(batch.len(), 6);
        let tsv = to_tsv(&m, s, (0, 0, 1, 2));
        assert_eq!(tsv, "7\t7\t7\n7\t7\t7");
        let mut again = vec![];
        clear(&mut m, s, (0, 0, 1, 2), 2, &mut again);
        assert_eq!(again.len(), 6);
        assert_eq!(shown(&m, s, 1, 2), "");
    }

    #[test]
    fn statistics_cover_numbers_only() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (0, 0), (0, 0, 0, 0), "1\n2.5\nword\n", 1, &mut batch);
        let (filled, numbers, sum) = stats(&m, s, (0, 0, 3, 0));
        assert_eq!((filled, numbers), (3, 2));
        assert!((sum - 3.5).abs() < 1e-9);
        assert_eq!(number(3.0), "3");
        assert_eq!(number(3.5), "3.5");
    }

    #[test]
    fn formulas_are_typed_calculated_and_undone() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put_input(&mut m, s, (0, 0), "5", 1, &mut batch);
        put_input(&mut m, s, (0, 1), "=A1*2", 2, &mut batch);
        m.recalc(&Stamp::new("t", 3));
        assert_eq!(shown(&m, s, 0, 1), "10");
        assert_eq!(input_text(&m, s, 0, 1), "=A1*2");
        replay(&mut m, &batch, true, 4);
        m.recalc(&Stamp::new("t", 5));
        assert_eq!(shown(&m, s, 0, 1), "");
        assert_eq!(input_text(&m, s, 0, 1), "");
        replay(&mut m, &batch, false, 6);
        m.recalc(&Stamp::new("t", 7));
        assert_eq!(shown(&m, s, 0, 1), "10");
        // Typing a value over a formula replaces it.
        let mut again = vec![];
        put_input(&mut m, s, (0, 1), "7", 8, &mut again);
        assert_eq!(input_text(&m, s, 0, 1), "7");
    }

    #[test]
    fn values_follow_the_active_dimensions() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put(&mut m, s, (0, 0), Value::Int(1), 1, &mut batch);
        m.active_dims.insert("scenario".into(), "budget".into());
        assert_eq!(shown(&m, s, 0, 0), "");
        put(&mut m, s, (0, 0), Value::Int(9), 2, &mut batch);
        assert_eq!(shown(&m, s, 0, 0), "9");
        // Undo goes back to the cell it was written under, even though the view has moved on.
        m.active_dims.clear();
        replay(&mut m, &batch[1..], true, 3);
        assert_eq!(shown(&m, s, 0, 0), "1");
        m.active_dims.insert("scenario".into(), "budget".into());
        assert_eq!(shown(&m, s, 0, 0), "");
    }
}
