//! The **Data** tab: the package's shared maix store as an editable grid.
//!
//! Anything linked to a Data range (see `linking.rs`) appears here, and a value typed here flows
//! out to every sheet, document table and slide table linked to the same cells. Values are typed
//! the same way as in tables: `12` is a number, `007` stays text, `true` is a boolean, and
//! `2026-03-01` is a date. `=` starts a formula.
//!
//! The editor follows GridCraft's conventions and limits: 1,048,576 rows by 16,384 columns,
//! resizable rows and columns (drag a header edge, double-click to fit), hide/unhide, frozen
//! panes, zoom, context menus on cells, row and column headers and sheet tabs, a menu bar and a
//! formatting toolbar (bold, colours, alignment, number formats in Excel's codes), insert and
//! delete rows and columns (formulas and links follow), sorting, fill, and a clipboard that keeps
//! formulas and formatting inside the app and plain tab-separated text for everything else.
//!
//! The grid is drawn by hand and only for the cells in view, with its own scrolling (a million
//! rows do not fit the precision of a normal scroll area), so it stays fast on big sheets.
//!
//! Layout of the code: `grid.rs` draws and handles the pointer, `menu.rs` has the menus,
//! toolbar and dialogs, `ops.rs` the operations they trigger, `axis.rs` row/column geometry and
//! `numfmt.rs` number formats.

mod axis;
mod compare;
mod filter;
mod find;
mod grid;
mod menu;
mod numfmt;
mod ops;

use alfadoc_bridge::textcell;
use alfadoc_model::format::{MAX_COLS, MAX_ROWS};
use alfadoc_model::{Axis, DimVector, Maix, Shift, SheetFormat, Stamp, Value, maix::Sheet};
use egui::{Color32, Event, Key};

const HEADER_W: f32 = 54.0;
const HEADER_H: f32 = 26.0;
const MIN_ROWS: u32 = 200;
const MIN_COLS: u32 = 26;
const PAGE_ROWS: u32 = 20;
/// The most cells one paste, fill or clear touches.
const MAX_BATCH: usize = 200_000;
/// Ranges bigger than this are cut back to the used area before copying, clearing or counting.
const BIG_RANGE: usize = 50_000;
/// Undo steps kept.
const MAX_UNDO: usize = 100;
/// Who the store records as the author of edits made in this tab.
const AUTHOR: &str = "alfadoc-user";
const MIN_ZOOM: f32 = 0.4;
const MAX_ZOOM: f32 = 3.0;

type Cell = (u32, u16);
/// (top, left, bottom, right), both ends included.
type Range = alfadoc_model::Rect;

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

/// One undo step.
#[derive(Clone, Debug)]
enum Step {
    Cells(Vec<Change>),
    /// A change of sizes, hidden lines, frozen panes or styles.
    Format { sheet: u16, before: Box<SheetFormat>, after: Box<SheetFormat> },
    /// A change of the sheet's shape (insert/delete rows or columns, sort). `shift` says which, so
    /// links can follow it.
    Sheet { before: Box<Sheet>, after: Box<Sheet>, shift: Option<(Axis, Shift)> },
    Many(Vec<Step>),
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

/// A pointer gesture in progress.
enum Drag {
    /// Selecting cells.
    Select,
    /// Selecting whole columns (`true`) or rows by dragging over their headers.
    Header(bool),
    /// Resizing the lines `first..=last` of one axis by dragging a header edge.
    /// Dragging the fill handle from the selection `src` (always a normalised range).
    Fill { src: Range },
    Resize { cols: bool, first: u32, last: u32, start: f32, from: f32, before: Box<SheetFormat> },
}

/// What a right-click landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Cell(Cell),
    Column(u16),
    Row(u32),
    Corner,
}

/// Cells copied inside the app: contents with their formulas, and their styles.
#[derive(Clone, Debug)]
struct Clip {
    sheet: u16,
    range: Range,
    /// The text put on the system clipboard (to tell whether a paste is ours).
    text: String,
    /// Row by row.
    contents: Vec<Content>,
    /// The shown values (for pasting values only).
    values: Vec<Value>,
    styles: Vec<alfadoc_model::CellStyle>,
    cut: bool,
}

pub struct DataView {
    sheet: Option<u16>,
    anchor: Cell,
    focus: Cell,
    edit: Option<Edit>,
    /// Text typed in the bar above the grid, before it is applied.
    bar: Option<String>,
    /// Text typed in the address box, before Enter.
    addr: Option<String>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Steps made this frame besides cell changes, added to the undo list at the end of it.
    steps: Vec<Step>,
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
    /// Scroll offsets (x, y) of the scrolling part, in sheet units.
    scroll: (f64, f64),
    zoom: f32,
    /// How many rows and columns the scroll bars cover; grows as you scroll towards the end.
    rows_ext: u32,
    cols_ext: u32,
    drag: Option<Drag>,
    /// What the last right-click landed on.
    ctx_hit: Option<Hit>,
    clip: Option<Clip>,
    find: find::FindState,
    filters: std::collections::HashMap<u16, filter::Filter>,
    filter_popup: Option<filter::Popup>,
    compare: compare::CompareState,
    /// While the fill handle is dragged: (down, the cells that would be filled).
    fill_target: Option<(bool, Range)>,
    prompt: Option<menu::Prompt>,
    gridlines: bool,
    /// A short message shown in the footer, with the time it was first shown (NaN: not yet).
    note: Option<(String, f64)>,
    /// Counts every change made in this tab (values, sizes, styles, undo), so the shell can tell
    /// that the file has unsaved changes and that links need a look.
    revision: u64,
    /// Inserted/deleted lines of this frame, for the shell to move link anchors.
    shifts: Vec<(u16, Axis, Shift)>,
    /// Ranges (sheet, top, left, bottom, right) that links keep in step; the shell sets them
    /// before each frame and they are drawn tinted.
    pub linked: Vec<(u16, Range)>,
}

impl Default for DataView {
    fn default() -> Self {
        DataView {
            sheet: None,
            anchor: (0, 0),
            focus: (0, 0),
            edit: None,
            bar: None,
            addr: None,
            undo: vec![],
            redo: vec![],
            steps: vec![],
            extent: None,
            rename: None,
            scroll_to_focus: false,
            grab_focus: false,
            needs_calc: false,
            dim_key: String::new(),
            dim_value: String::new(),
            scroll: (0.0, 0.0),
            zoom: 1.0,
            rows_ext: 0,
            cols_ext: 0,
            drag: None,
            ctx_hit: None,
            clip: None,
            find: find::FindState::default(),
            filters: std::collections::HashMap::new(),
            filter_popup: None,
            compare: compare::CompareState::default(),
            fill_target: None,
            prompt: None,
            gridlines: true,
            note: None,
            revision: 0,
            shifts: vec![],
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

/// The cells of `range` as tab-separated text, one line per row.
fn to_tsv(maix: &Maix, sheet: u16, range: Range) -> String {
    let range = effective(maix, sheet, range);
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
fn paste(maix: &mut Maix, sheet: u16, at: Cell, selection: Range, text: &str, now: u64, batch: &mut Vec<Change>) {
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

fn clear(maix: &mut Maix, sheet: u16, range: Range, now: u64, batch: &mut Vec<Change>) {
    let range = effective(maix, sheet, range);
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
fn stats(maix: &Maix, sheet: u16, range: Range) -> (usize, usize, f64) {
    let range = effective(maix, sheet, range);
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


/// The selected rectangle as (top, left, bottom, right), both ends included.
fn normalized(a: Cell, b: Cell) -> Range {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn cell_count(range: Range) -> usize {
    let rows = (range.2 - range.0) as usize + 1;
    let cols = (range.3 - range.1) as usize + 1;
    rows.saturating_mul(cols)
}

/// `range`, cut back to the used area when it is huge (a whole column, say), so that copying,
/// clearing and counting only look at cells that can hold something.
fn effective(maix: &Maix, sheet: u16, range: Range) -> Range {
    if cell_count(range) <= BIG_RANGE {
        return range;
    }
    let used = maix.sheet(sheet).map_or((0, 0), Sheet::extent);
    let bottom = range.2.min(used.0.saturating_sub(1)).max(range.0);
    let right = range.3.min(used.1.saturating_sub(1)).max(range.1);
    (range.0, range.1, bottom, right)
}

fn whole_cols(sel: Range) -> bool {
    sel.0 == 0 && sel.2 == MAX_ROWS - 1
}

fn whole_rows(sel: Range) -> bool {
    sel.1 == 0 && sel.3 == MAX_COLS - 1
}

/// Where the cell reached by moving `count` steps in direction `dir` from `start` lies, skipping
/// hidden lines and stopping at the sheet edge (`max` is the last valid index).
fn walk(start: i64, dir: i64, count: u64, max: i64, hidden: impl Fn(i64) -> bool) -> i64 {
    let mut at = start;
    for _ in 0..count {
        let mut n = at + dir;
        while (0..=max).contains(&n) && hidden(n) {
            n += dir;
        }
        if !(0..=max).contains(&n) {
            break;
        }
        at = n;
    }
    at
}

fn filled(maix: &Maix, sheet: u16, row: u32, col: u16) -> bool {
    !matches!(value_at(maix, sheet, row, col), Value::Null) || maix.formula(sheet, row, col, &maix.active_dims).is_some()
}

/// Where `Ctrl`+arrow goes: to the end of the block of filled cells, or to the next filled cell,
/// or to the edge of the sheet.
fn jump_target(maix: &Maix, sheet: u16, from: Cell, dr: i64, dc: i64, used: (u32, u16)) -> Cell {
    let inside = |r: i64, c: i64| r >= 0 && c >= 0 && r < i64::from(MAX_ROWS) && c < i64::from(MAX_COLS);
    let cell = |r: i64, c: i64| (u32::try_from(r).unwrap_or(0), u16::try_from(c).unwrap_or(0));
    let (mut r, mut c) = (i64::from(from.0), i64::from(from.1));
    let edge = cell(
        if dr > 0 { i64::from(MAX_ROWS) - 1 } else if dr < 0 { 0 } else { r },
        if dc > 0 { i64::from(MAX_COLS) - 1 } else if dc < 0 { 0 } else { c },
    );
    if !inside(r + dr, c + dc) {
        return from;
    }
    let at = |r: i64, c: i64| {
        let (row, col) = cell(r, c);
        filled(maix, sheet, row, col)
    };
    if at(r, c) && at(r + dr, c + dc) {
        while inside(r + dr, c + dc) && at(r + dr, c + dc) {
            r += dr;
            c += dc;
        }
        return cell(r, c);
    }
    loop {
        r += dr;
        c += dc;
        if !inside(r, c) {
            return edge;
        }
        if at(r, c) {
            return cell(r, c);
        }
        if (dr > 0 && r >= i64::from(used.0)) || (dc > 0 && c >= i64::from(used.1)) {
            return edge;
        }
    }
}

/// `B7`, `b7:d9`, `C:C`, `3:5` → the cells they name, as an anchor and a focus.
pub(crate) fn parse_address(text: &str) -> Option<(Cell, Cell)> {
    fn part(t: &str) -> Option<(Option<u32>, Option<u16>)> {
        let t = t.trim().trim_start_matches('$');
        let split = t.find(|c: char| c.is_ascii_digit()).unwrap_or(t.len());
        let (letters, digits) = t.split_at(split);
        let digits = digits.replace('$', "");
        if letters.is_empty() && digits.is_empty() {
            return None;
        }
        let col = if letters.is_empty() {
            None
        } else {
            if letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            let n = letters.chars().fold(0u32, |n, c| n * 26 + (u32::from(c.to_ascii_uppercase() as u8) - u32::from(b'A') + 1));
            Some(u16::try_from(n.checked_sub(1)?).ok().filter(|c| *c < MAX_COLS)?)
        };
        let row = if digits.is_empty() {
            None
        } else {
            let n: u32 = digits.parse().ok()?;
            Some(n.checked_sub(1).filter(|r| *r < MAX_ROWS)?)
        };
        Some((row, col))
    }
    let (a, b) = match text.split_once(':') {
        Some((a, b)) => (part(a)?, part(b)?),
        None => {
            let a = part(text)?;
            (a, a)
        }
    };
    let (top, bottom) = match (a.0, b.0) {
        (Some(x), Some(y)) => (x.min(y), x.max(y)),
        _ => (0, MAX_ROWS - 1),
    };
    let (left, right) = match (a.1, b.1) {
        (Some(x), Some(y)) => (x.min(y), x.max(y)),
        _ => (0, MAX_COLS - 1),
    };
    Some(((bottom, right), (top, left)))
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

impl DataView {
    /// The sheet shown and the selected rectangle (top, left, bottom, right).
    pub fn selection(&self) -> Option<(u16, Range)> {
        self.sheet.map(|s| (s, normalized(self.anchor, self.focus)))
    }

    /// Rows or columns inserted or deleted since the last call: `(sheet, axis, what)`. The shell
    /// moves the links that point into those sheets.
    /// A number that grows with every change made in this tab.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn take_shifts(&mut self) -> Vec<(u16, Axis, Shift)> {
        std::mem::take(&mut self.shifts)
    }

    fn sel(&self) -> Range {
        normalized(self.anchor, self.focus)
    }

    /// Shows another sheet from its top-left corner.
    fn switch_to(&mut self, id: u16) {
        self.sheet = Some(id);
        self.anchor = (0, 0);
        self.focus = (0, 0);
        self.edit = None;
        self.scroll = (0.0, 0.0);
        self.rows_ext = 0;
        self.cols_ext = 0;
        self.drag = None;
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
        let e = maix.sheet(sheet).map_or((0, 0), Sheet::extent);
        self.extent = Some((sheet, stamp, e));
        e
    }

    /// Draws the tab. Returns true when the user changed the store (a value, a size, a style…).
    pub fn show(&mut self, ui: &mut egui::Ui, maix: &mut Maix, now_ms: u64) -> bool {
        if self.sheet.is_none_or(|id| maix.sheet(id).is_err()) {
            match maix.sheets.first().map(|s| s.id) {
                Some(id) => self.switch_to(id),
                None => self.sheet = None,
            }
        }
        let Some(sheet) = self.sheet else {
            let mut changed = false;
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("The shared data store is empty");
                ui.label("Add a sheet to type data here, or link a range to Data from the Links tab.");
                if ui.button("Add a sheet").clicked() {
                    let id = maix.add_sheet("Sheet1");
                    self.switch_to(id);
                    changed = true;
                }
            });
            return changed;
        };
        let mut batch: Vec<Change> = vec![];
        let history_before = maix.history.len();
        let sheets_before = maix.sheets.len();
        self.menu_bar(ui, maix, sheet, now_ms, &mut batch);
        self.toolbar(ui, maix, sheet, now_ms);
        self.bar_row(ui, maix, sheet, now_ms, &mut batch);
        let footer = 34.0;
        let grid_height = (ui.available_height() - footer).max(80.0);
        self.grid(ui, maix, sheet, now_ms, grid_height, &mut batch);
        self.footer(ui, maix, sheet);
        self.prompt_window(ui.ctx(), maix, sheet, now_ms, &mut batch);
        self.find_window(ui.ctx(), maix, sheet, now_ms, &mut batch);
        self.filter_window(ui.ctx(), maix, sheet);
        self.compare_window(ui.ctx(), maix, sheet);
        let extra = std::mem::take(&mut self.steps);
        let mut changed = maix.sheets.len() != sheets_before;
        if !batch.is_empty() || !extra.is_empty() || self.needs_calc || maix.history.len() != history_before {
            maix.recalc(&Stamp::new(AUTHOR, now_ms));
            self.needs_calc = false;
        }
        let mut parts = vec![];
        if !batch.is_empty() {
            parts.push(Step::Cells(batch));
        }
        parts.extend(extra);
        if let Some(step) = if parts.len() > 1 { Some(Step::Many(parts)) } else { parts.pop() } {
            self.undo.push(step);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
            changed = true;
        }
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// The address box and the value bar above the grid.
    fn bar_row(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, batch: &mut Vec<Change>) {
        let range = self.sel();
        let name = if cell_count(range) > 1 {
            if whole_cols(range) {
                format!("{}:{}", column_name(range.1), column_name(range.3))
            } else if whole_rows(range) {
                format!("{}:{}", range.0 + 1, range.2 + 1)
            } else {
                format!("{}:{}", address((range.0, range.1)), address((range.2, range.3)))
            }
        } else {
            address(self.focus)
        };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let mut text = self.addr.clone().unwrap_or(name);
            let r = ui.add(egui::TextEdit::singleline(&mut text).id_salt("data_address").font(egui::TextStyle::Monospace).desired_width(92.0));
            if r.changed() || r.has_focus() {
                self.addr = Some(text.clone());
            }
            if r.lost_focus() {
                if ui.input(|i| i.key_pressed(Key::Enter))
                    && let Some((a, f)) = parse_address(&text)
                {
                    self.anchor = a;
                    self.focus = f;
                    self.scroll_to_focus = true;
                    self.grab_focus = true;
                }
                self.addr = None;
            }
            ui.label(egui::RichText::new("fx").weak().italics());
            if let Some(edit) = &self.edit {
                let mut mirror = edit.text.clone();
                ui.add(egui::TextEdit::singleline(&mut mirror).desired_width(f32::INFINITY).interactive(false));
                return;
            }
            let mut text = self.bar.clone().unwrap_or_else(|| input_text(maix, sheet, self.focus.0, self.focus.1));
            let response = ui.add(egui::TextEdit::singleline(&mut text).id_salt("data_value_bar").desired_width(f32::INFINITY).hint_text("Type a value or a formula, then press Enter"));
            if response.changed() || response.has_focus() {
                self.bar = Some(text.clone());
            }
            if response.lost_focus() {
                let escaped = ui.input(|i| i.key_pressed(Key::Escape));
                if !escaped {
                    put_input(maix, sheet, self.focus, &text, now, batch);
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.step(maix, sheet, 1, 0, false);
                    }
                }
                self.bar = None;
                self.grab_focus = true;
            }
        });
        ui.add_space(2.0);
    }

    /// Moves the focus cell (and the anchor unless `extend`), skipping hidden lines and stopping
    /// at the edge of the sheet.
    fn step(&mut self, maix: &Maix, sheet: u16, dr: i64, dc: i64, extend: bool) {
        let lay = maix.sheet(sheet).ok().map(|s| &s.format.layout);
        let hidden_row = |r: i64| lay.is_some_and(|l| u32::try_from(r).is_ok_and(|r| l.hidden_rows.contains(&r)));
        let hidden_col = |c: i64| lay.is_some_and(|l| u16::try_from(c).is_ok_and(|c| l.hidden_cols.contains(&c)));
        let row = walk(i64::from(self.focus.0), dr.signum(), dr.unsigned_abs(), i64::from(MAX_ROWS) - 1, hidden_row);
        let col = walk(i64::from(self.focus.1), dc.signum(), dc.unsigned_abs(), i64::from(MAX_COLS) - 1, hidden_col);
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
    fn key_event(&mut self, ctx: &egui::Context, maix: &mut Maix, sheet: u16, now: u64, event: &Event, batch: &mut Vec<Change>) {
        let sel = self.sel();
        match event {
            Event::Copy => self.copy(ctx, maix, sheet, false),
            Event::Cut => self.copy(ctx, maix, sheet, true),
            Event::Paste(text) => self.paste_text(maix, sheet, text, now, batch),
            Event::Text(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                self.begin_edit(maix, sheet, Some(t.clone()));
            }
            Event::Key { key, pressed: true, modifiers, repeat: _, .. } => {
                let cmd = modifiers.command;
                let extend = modifiers.shift;
                let used = self.used(maix, sheet);
                let arrow = |this: &mut Self, maix: &Maix, dr: i64, dc: i64| {
                    if cmd {
                        let to = jump_target(maix, sheet, this.focus, dr, dc, used);
                        this.focus = to;
                        if !extend {
                            this.anchor = to;
                        }
                        this.scroll_to_focus = true;
                    } else {
                        this.step(maix, sheet, dr, dc, extend);
                    }
                };
                match key {
                    Key::ArrowDown => arrow(self, maix, 1, 0),
                    Key::ArrowUp => arrow(self, maix, -1, 0),
                    Key::ArrowLeft => arrow(self, maix, 0, -1),
                    Key::ArrowRight => arrow(self, maix, 0, 1),
                    Key::PageDown => self.step(maix, sheet, i64::from(PAGE_ROWS), 0, extend),
                    Key::PageUp => self.step(maix, sheet, -i64::from(PAGE_ROWS), 0, extend),
                    Key::Home => {
                        let to = if cmd { (0, 0) } else { (self.focus.0, 0) };
                        self.focus = to;
                        if !extend {
                            self.anchor = to;
                        }
                        self.scroll_to_focus = true;
                    }
                    Key::End => {
                        let to = if cmd { (used.0.saturating_sub(1), used.1.saturating_sub(1)) } else { (self.focus.0, used.1.saturating_sub(1)) };
                        self.focus = to;
                        if !extend {
                            self.anchor = to;
                        }
                        self.scroll_to_focus = true;
                    }
                    Key::Tab => self.step(maix, sheet, 0, if extend { -1 } else { 1 }, false),
                    Key::Enter if !cmd => {
                        if extend {
                            self.step(maix, sheet, -1, 0, false);
                        } else {
                            self.begin_edit(maix, sheet, None);
                        }
                    }
                    Key::F2 => self.begin_edit(maix, sheet, None),
                    Key::Delete | Key::Backspace => clear(maix, sheet, sel, now, batch),
                    Key::Escape => {
                        self.anchor = self.focus;
                        self.clip = self.clip.take().filter(|c| !c.cut);
                    }
                    Key::A if cmd => {
                        self.anchor = (0, 0);
                        self.focus = (used.0.saturating_sub(1), used.1.saturating_sub(1));
                    }
                    Key::Space if cmd => {
                        self.anchor = (0, sel.1);
                        self.focus = (MAX_ROWS - 1, sel.3);
                    }
                    Key::Space if extend => {
                        self.anchor = (sel.0, 0);
                        self.focus = (sel.2, MAX_COLS - 1);
                    }
                    Key::Z if cmd && !extend => self.undo_step(maix, now),
                    Key::Z | Key::Y if cmd => self.redo_step(maix, now),
                    Key::B if cmd => self.toggle(maix, sheet, ops::Toggle::Bold),
                    Key::I if cmd => self.toggle(maix, sheet, ops::Toggle::Italic),
                    Key::U if cmd => self.toggle(maix, sheet, ops::Toggle::Underline),
                    Key::D if cmd => self.fill(maix, sheet, true, now, batch),
                    Key::R if cmd => self.fill(maix, sheet, false, now, batch),
                    Key::Num5 if cmd && extend => self.toggle(maix, sheet, ops::Toggle::Strike),
                    Key::Plus | Key::Equals if cmd => self.insert_for_selection(maix, sheet, now),
                    Key::Minus if cmd => self.delete_for_selection(maix, sheet, now),
                    Key::G if cmd => self.prompt = Some(menu::Prompt::GoTo(String::new())),
                    Key::F | Key::H if cmd => self.open_find(),
                    Key::L if cmd && extend => self.toggle_filter(maix, sheet),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(crate) fn undo_step(&mut self, maix: &mut Maix, now: u64) {
        if let Some(step) = self.undo.pop() {
            self.run_step(maix, &step, true, now);
            self.needs_calc = true;
            self.revision += 1;
            self.redo.push(step);
        }
    }

    pub(crate) fn redo_step(&mut self, maix: &mut Maix, now: u64) {
        if let Some(step) = self.redo.pop() {
            self.run_step(maix, &step, false, now);
            self.needs_calc = true;
            self.revision += 1;
            self.undo.push(step);
        }
    }

    /// Applies a step backwards (undo) or forwards (redo).
    fn run_step(&mut self, maix: &mut Maix, step: &Step, backwards: bool, now: u64) {
        match step {
            Step::Cells(changes) => {
                replay(maix, changes, backwards, now);
                if let Some(first) = changes.first() {
                    self.focus = (first.row, first.col);
                    self.anchor = self.focus;
                    self.scroll_to_focus = true;
                }
            }
            Step::Format { sheet, before, after } => {
                if let Ok(f) = maix.format_mut(*sheet) {
                    *f = if backwards { before.as_ref().clone() } else { after.as_ref().clone() };
                }
            }
            Step::Sheet { before, after, shift } => {
                let id = before.id;
                maix.replace_sheet(if backwards { before.as_ref().clone() } else { after.as_ref().clone() });
                if let Some((axis, op)) = shift {
                    let applied = if backwards {
                        match *op {
                            Shift::Insert { at, count } => Shift::Delete { at, count },
                            Shift::Delete { at, count } => Shift::Insert { at, count },
                        }
                    } else {
                        *op
                    };
                    self.shifts.push((id, *axis, applied));
                }
            }
            Step::Many(parts) => {
                if backwards {
                    for p in parts.iter().rev() {
                        self.run_step(maix, p, true, now);
                    }
                } else {
                    for p in parts {
                        self.run_step(maix, p, false, now);
                    }
                }
            }
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
                    self.switch_to(id);
                }
                response.context_menu(|ui| self.sheet_tab_menu(ui, maix, id, &name));
            }
            self.dimension_menu(ui, maix);
            if ui.button("+").on_hover_text("Add a sheet").clicked() {
                let mut n = maix.sheets.len() + 1;
                while maix.sheet_by_name(&format!("Sheet{n}")).is_some() {
                    n += 1;
                }
                let id = maix.add_sheet(&format!("Sheet{n}"));
                self.switch_to(id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let sel = self.sel();
                let (filled, numbers, sum) = stats(maix, sheet, sel);
                let now = ui.input(|i| i.time);
                if let Some((_, at)) = &mut self.note
                    && at.is_nan()
                {
                    *at = now;
                }
                if let Some((text, at)) = &self.note
                    && now - *at < 6.0
                {
                    ui.colored_label(ui.visuals().warn_fg_color, text);
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
                let mut parts = vec![format!("{} cell(s) selected", cell_count(sel))];
                if filled > 0 {
                    parts.push(format!("Count {filled}"));
                }
                if numbers > 0 {
                    parts.push(format!("Sum {}", number(sum)));
                    parts.push(format!("Average {}", number(sum / numbers as f64)));
                }
                parts.push(format!("{}%", (self.zoom * 100.0).round()));
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

    #[test]
    fn addresses_are_parsed_with_the_active_cell_at_the_top_left() {
        assert_eq!(parse_address("B7"), Some(((6, 1), (6, 1))));
        assert_eq!(parse_address("b7:d9"), Some(((8, 3), (6, 1))));
        assert_eq!(parse_address("C:C"), Some(((MAX_ROWS - 1, 2), (0, 2))));
        assert_eq!(parse_address("3:5"), Some(((4, MAX_COLS - 1), (2, 0))));
        assert_eq!(parse_address("XFD1048576"), Some(((MAX_ROWS - 1, MAX_COLS - 1), (MAX_ROWS - 1, MAX_COLS - 1))));
        assert_eq!(parse_address("XFE1"), None);
        assert_eq!(parse_address("A1048577"), None);
        assert_eq!(parse_address("hello world"), None);
        assert_eq!(parse_address(""), None);
    }

    #[test]
    fn the_sheet_is_as_big_as_gridcrafts() {
        assert_eq!(MAX_ROWS, 1_048_576);
        assert_eq!(MAX_COLS, 16_384);
        assert_eq!(column_name(MAX_COLS - 1), "XFD");
    }

    #[test]
    fn walking_skips_hidden_lines_and_stops_at_the_edge() {
        assert_eq!(walk(0, 1, 3, 10, |n| n == 2), 4);
        assert_eq!(walk(5, -1, 10, 10, |_| false), 0);
        assert_eq!(walk(9, 1, 5, 10, |_| false), 10);
        assert_eq!(walk(3, 1, 4, 10, |n| n >= 5), 4);
    }

    #[test]
    fn ctrl_arrow_jumps_between_blocks() {
        let (mut m, s) = store();
        let mut batch = vec![];
        for r in [0, 1, 2, 4] {
            put(&mut m, s, (r, 0), Value::Int(1), 1, &mut batch);
        }
        let used = (5, 1);
        assert_eq!(jump_target(&m, s, (0, 0), 1, 0, used), (2, 0));
        assert_eq!(jump_target(&m, s, (2, 0), 1, 0, used), (4, 0));
        assert_eq!(jump_target(&m, s, (4, 0), 1, 0, used), (MAX_ROWS - 1, 0));
        assert_eq!(jump_target(&m, s, (4, 0), -1, 0, used), (2, 0));
        assert_eq!(jump_target(&m, s, (0, 0), -1, 0, used), (0, 0));
        assert_eq!(jump_target(&m, s, (0, 0), 0, 1, used), (0, MAX_COLS - 1));
    }

    #[test]
    fn a_huge_range_is_cut_back_to_the_used_area() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put(&mut m, s, (9, 0), Value::Int(1), 1, &mut batch);
        assert_eq!(effective(&m, s, (0, 0, 4, 4)), (0, 0, 4, 4));
        assert_eq!(effective(&m, s, (0, 2, MAX_ROWS - 1, 2)), (0, 2, 9, 2));
        assert!(whole_cols((0, 2, MAX_ROWS - 1, 2)));
        assert!(whole_rows((3, 0, 3, MAX_COLS - 1)));
        // Clearing a whole column only visits the used rows.
        let mut again = vec![];
        clear(&mut m, s, (0, 0, MAX_ROWS - 1, 0), 2, &mut again);
        assert_eq!(again.len(), 1);
    }

    #[test]
    fn the_cursor_steps_over_hidden_rows() {
        let (mut m, s) = store();
        m.format_mut(s).unwrap().layout.set_rows_hidden(1..=1, true);
        let mut v = DataView::default();
        v.step(&m, s, 1, 0, false);
        assert_eq!(v.focus, (2, 0));
        v.step(&m, s, -1, 0, false);
        assert_eq!(v.focus, (0, 0));
        v.step(&m, s, 0, -1, true);
        assert_eq!(v.focus, (0, 0));
    }

    #[test]
    fn styling_is_one_undo_step() {
        let (mut m, s) = store();
        let mut v = DataView::default();
        v.sheet = Some(s);
        v.anchor = (0, 0);
        v.focus = (1, 1);
        v.toggle(&mut m, s, ops::Toggle::Bold);
        assert!(m.sheet(s).unwrap().format.style_at(1, 1).bold);
        let step = v.steps.pop().unwrap();
        v.run_step(&mut m, &step, true, 1);
        assert!(!m.sheet(s).unwrap().format.style_at(1, 1).bold);
        v.run_step(&mut m, &step, false, 2);
        assert!(m.sheet(s).unwrap().format.style_at(0, 0).bold);
        // Toggling again with the cell bold turns it off.
        v.toggle(&mut m, s, ops::Toggle::Bold);
        assert!(!m.sheet(s).unwrap().format.style_at(0, 0).bold);
    }

    #[test]
    fn inserting_rows_undoes_and_tells_the_links() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put(&mut m, s, (0, 0), Value::Text("a".into()), 1, &mut batch);
        let mut v = DataView::default();
        v.insert_lines(&mut m, s, Axis::Rows, 0, 2);
        assert_eq!(shown(&m, s, 2, 0), "a");
        assert_eq!(shown(&m, s, 0, 0), "");
        assert_eq!(v.take_shifts(), vec![(s, Axis::Rows, Shift::Insert { at: 0, count: 2 })]);
        let step = v.steps.pop().unwrap();
        v.run_step(&mut m, &step, true, 2);
        assert_eq!(shown(&m, s, 0, 0), "a");
        assert_eq!(v.take_shifts(), vec![(s, Axis::Rows, Shift::Delete { at: 0, count: 2 })]);
        v.run_step(&mut m, &step, false, 3);
        assert_eq!(shown(&m, s, 2, 0), "a");
    }

    #[test]
    fn copying_inside_the_app_keeps_formulas_moving_and_styles() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put_input(&mut m, s, (0, 0), "5", 1, &mut batch);
        put_input(&mut m, s, (0, 1), "=A1*2", 1, &mut batch);
        m.format_mut(s).unwrap().apply((0, 1, 0, 1), &|st| alfadoc_model::CellStyle { bold: true, ..st.clone() });
        m.recalc(&Stamp::new("t", 2));
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (0, 1);
        let ctx = egui::Context::default();
        v.copy(&ctx, &m, s, false);
        let clip = v.clip.clone().unwrap();
        v.anchor = (3, 0);
        v.focus = (3, 0);
        v.paste_clip(&mut m, s, &clip, ops::PasteKind::All, 2, &mut batch);
        m.recalc(&Stamp::new("t", 3));
        assert_eq!(input_text(&m, s, 3, 1), "=A4*2");
        assert_eq!(shown(&m, s, 3, 1), "10");
        assert!(m.sheet(s).unwrap().format.style_at(3, 1).bold);
        assert_eq!(v.sel(), (3, 0, 3, 1));
        // Values only: no formula and no style come along.
        v.anchor = (6, 0);
        v.focus = (6, 0);
        v.paste_clip(&mut m, s, &clip, ops::PasteKind::Values, 4, &mut batch);
        assert_eq!(input_text(&m, s, 6, 1), "10");
        assert!(!m.sheet(s).unwrap().format.style_at(6, 1).bold);
    }

    #[test]
    fn cutting_moves_the_cells_and_the_clipboard_is_used_up() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put_input(&mut m, s, (0, 0), "x", 1, &mut batch);
        let mut v = DataView::default();
        let ctx = egui::Context::default();
        v.copy(&ctx, &m, s, true);
        let clip = v.clip.clone().unwrap();
        v.anchor = (2, 2);
        v.focus = (2, 2);
        v.paste_clip(&mut m, s, &clip, ops::PasteKind::All, 2, &mut batch);
        assert_eq!(shown(&m, s, 0, 0), "");
        assert_eq!(shown(&m, s, 2, 2), "x");
        assert!(v.clip.is_none());
    }

    #[test]
    fn fill_down_copies_the_first_row_and_moves_references() {
        let (mut m, s) = store();
        let mut batch = vec![];
        put_input(&mut m, s, (0, 0), "1", 1, &mut batch);
        put_input(&mut m, s, (0, 1), "=A1+1", 1, &mut batch);
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (2, 1);
        v.fill(&mut m, s, true, 2, &mut batch);
        m.recalc(&Stamp::new("t", 3));
        assert_eq!(shown(&m, s, 2, 0), "1");
        assert_eq!(input_text(&m, s, 2, 1), "=A3+1");
        assert_eq!(shown(&m, s, 2, 1), "2");
        // A single row fills from the row above.
        v.anchor = (5, 0);
        v.focus = (5, 0);
        v.fill(&mut m, s, true, 4, &mut batch);
        assert_eq!(shown(&m, s, 5, 0), "");
        v.anchor = (3, 0);
        v.focus = (3, 0);
        v.fill(&mut m, s, true, 5, &mut batch);
        assert_eq!(shown(&m, s, 3, 0), "1");
    }

    #[test]
    fn sorting_the_selection_is_one_undoable_step_and_guesses_titles() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (0, 0), (0, 0, 0, 0), "name\tqty\nb\t2\na\t9\nc\t1\n", 1, &mut batch);
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (0, 0);
        v.sort_selection(&mut m, s, true, Some(1), None);
        assert_eq!((shown(&m, s, 0, 0), shown(&m, s, 1, 0), shown(&m, s, 3, 0)), ("name".to_string(), "c".to_string(), "a".to_string()));
        let step = v.steps.pop().unwrap();
        v.run_step(&mut m, &step, true, 2);
        assert_eq!(shown(&m, s, 1, 0), "b");
    }

    #[test]
    fn number_series_continue_and_anything_else_is_copied() {
        use ops::series_next;
        let ints = |v: &[i64]| v.iter().map(|i| Value::Int(*i)).collect::<Vec<_>>();
        assert_eq!(series_next(&ints(&[1, 2]), 3), Some(ints(&[3, 4, 5])));
        assert_eq!(series_next(&ints(&[2, 4, 6]), 1), Some(ints(&[8])));
        assert_eq!(series_next(&ints(&[10, 5]), 2), Some(ints(&[0, -5])));
        assert_eq!(series_next(&ints(&[1, 3, 4]), 2), None);
        assert_eq!(series_next(&ints(&[7]), 2), None);
        assert_eq!(series_next(&[Value::Text("a".into()), Value::Text("b".into())], 2), None);
        assert_eq!(series_next(&[Value::Int(1), Value::Float(1.5)], 1), Some(vec![Value::Float(2.0)]));
        let day = 86_400_000;
        assert_eq!(series_next(&[Value::Date(0), Value::Date(day)], 1), Some(vec![Value::Date(2 * day)]));
        assert_eq!(series_next(&[Value::Date(0), Value::Int(1)], 1), None);
    }

    #[test]
    fn the_fill_handle_continues_series_repeats_the_rest_and_moves_formulas() {
        let (mut m, s) = store();
        let mut batch = vec![];
        for (cell, text) in [((0, 0), "1"), ((1, 0), "2"), ((0, 1), "x"), ((1, 1), "y"), ((0, 2), "=A1*2")] {
            put_input(&mut m, s, cell, text, 1, &mut batch);
        }
        let mut v = DataView::default();
        v.fill_range(&mut m, s, (0, 0, 1, 2), (2, 0, 4, 2), true, 2, &mut batch);
        m.recalc(&Stamp::new("t", 3));
        assert_eq!((shown(&m, s, 2, 0), shown(&m, s, 4, 0)), ("3".to_string(), "5".to_string()));
        assert_eq!((shown(&m, s, 2, 1), shown(&m, s, 3, 1), shown(&m, s, 4, 1)), ("x".to_string(), "y".to_string(), "x".to_string()));
        assert_eq!(input_text(&m, s, 0, 2), "=A1*2");
        assert_eq!(input_text(&m, s, 3, 2), "");
        // Rightwards: a row of 10, 20 goes on with 30, 40.
        put_input(&mut m, s, (8, 0), "10", 4, &mut batch);
        put_input(&mut m, s, (8, 1), "20", 4, &mut batch);
        v.fill_range(&mut m, s, (8, 0, 8, 1), (8, 2, 8, 3), false, 5, &mut batch);
        assert_eq!((shown(&m, s, 8, 2), shown(&m, s, 8, 3)), ("30".to_string(), "40".to_string()));
        // A formula in a single cell moves its references.
        put_input(&mut m, s, (10, 0), "=A1+1", 6, &mut batch);
        v.fill_range(&mut m, s, (10, 0, 10, 0), (11, 0, 12, 0), true, 7, &mut batch);
        assert_eq!((input_text(&m, s, 11, 0), input_text(&m, s, 12, 0)), ("=A2+1".to_string(), "=A3+1".to_string()));
    }

    #[test]
    fn dragging_the_handle_fills_down_or_right_never_back() {
        use grid::fill_target_of;
        let src = (2, 2, 3, 3);
        assert_eq!(fill_target_of(src, (6, 3)), Some((true, (4, 2, 6, 3))));
        assert_eq!(fill_target_of(src, (3, 7)), Some((false, (2, 4, 3, 7))));
        assert_eq!(fill_target_of(src, (5, 5)), Some((true, (4, 2, 5, 3))));
        assert_eq!(fill_target_of(src, (3, 3)), None);
        assert_eq!(fill_target_of(src, (0, 0)), None);
    }

    #[test]
    fn paste_special_can_transpose_and_skip_blanks() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (0, 0), (0, 0, 0, 0), "1\t2\n3\t4\n", 1, &mut batch);
        let mut v = DataView::default();
        let ctx = egui::Context::default();
        v.anchor = (0, 0);
        v.focus = (1, 1);
        v.copy(&ctx, &m, s, false);
        let clip = v.clip.clone().unwrap();
        v.anchor = (0, 3);
        v.focus = (0, 3);
        let opts = ops::PasteOpts { kind: ops::PasteKind::All, transpose: true, skip_blanks: false };
        v.paste_clip_with(&mut m, s, &clip, opts, 2, &mut batch);
        assert_eq!((shown(&m, s, 0, 3), shown(&m, s, 0, 4), shown(&m, s, 1, 3), shown(&m, s, 1, 4)), ("1".to_string(), "3".to_string(), "2".to_string(), "4".to_string()));
        assert_eq!(v.sel(), (0, 3, 1, 4));
        // Blank copied cells leave what is there alone when asked.
        put_input(&mut m, s, (5, 0), "7", 3, &mut batch);
        put_input(&mut m, s, (9, 0), "keep", 3, &mut batch);
        put_input(&mut m, s, (9, 1), "gone", 3, &mut batch);
        v.anchor = (5, 0);
        v.focus = (5, 1);
        v.copy(&ctx, &m, s, false);
        let clip = v.clip.clone().unwrap();
        v.anchor = (9, 0);
        v.focus = (9, 0);
        v.paste_clip_with(&mut m, s, &clip, ops::PasteOpts { kind: ops::PasteKind::All, transpose: false, skip_blanks: true }, 4, &mut batch);
        assert_eq!((shown(&m, s, 9, 0), shown(&m, s, 9, 1)), ("7".to_string(), "gone".to_string()));
        v.anchor = (9, 0);
        v.focus = (9, 0);
        v.paste_clip_with(&mut m, s, &clip, ops::PasteOpts::plain(ops::PasteKind::All), 5, &mut batch);
        assert_eq!(shown(&m, s, 9, 1), "");
    }

    #[test]
    fn inserting_rows_drops_the_filter_and_shows_its_rows() {
        let (mut m, s) = store();
        let mut batch = vec![];
        paste(&mut m, s, (0, 0), (0, 0, 0, 0), "name\nb\na\n", 1, &mut batch);
        let mut v = DataView::default();
        v.anchor = (0, 0);
        v.focus = (0, 0);
        v.toggle_filter(&mut m, s);
        assert!(v.has_filter(s));
        v.filters.get_mut(&s).unwrap().allowed.insert(0, ["a".to_string()].into_iter().collect());
        v.apply_filter(&mut m, s);
        assert_eq!(m.sheet(s).unwrap().format.layout.hidden_rows.len(), 1);
        v.insert_lines(&mut m, s, Axis::Rows, 1, 1);
        assert!(!v.has_filter(s));
        assert!(m.sheet(s).unwrap().format.layout.hidden_rows.is_empty());
    }
}
