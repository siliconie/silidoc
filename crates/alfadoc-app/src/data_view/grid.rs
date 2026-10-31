//! Drawing the grid and handling the pointer: custom scrolling, zoom, selection, resizing by the
//! header edges, frozen panes and the cell editor.

use alfadoc_bridge::textcell;
use alfadoc_model::format::{DEFAULT_COL_W, DEFAULT_ROW_H, MAX_COLS, MAX_ROWS, MIN_SIZE};
use alfadoc_model::{CellStyle, HAlign, Maix, Value};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, CursorIcon, FontId, Key, Painter, Pos2, Rect, Sense, Stroke, Vec2};

use super::axis::{Frame, Lines, scroll_to_show};
use super::{Cell, Change, DataView, Drag, HEADER_H, HEADER_W, Hit, MIN_COLS, MIN_ROWS, Palette, Range, cell_count, column_name, numfmt, put_input, whole_cols, whole_rows};

/// Width of the scroll bars.
const SB: f32 = 12.0;
/// How close to a header edge the pointer must be to grab it.
const EDGE: f32 = 4.0;
/// Default text size at 100% zoom.
const FONT: f32 = 13.5;

pub(super) fn rgb(c: u32) -> Color32 {
    Color32::from_rgb(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

fn font_px(style: &CellStyle, zoom: f32) -> f32 {
    (if style.size == 0 { FONT } else { f32::from(style.size) * FONT / 11.0 }) * zoom
}

/// Paints one cell: fill, then the text with its style and number format.
fn paint_cell(p: &Painter, pal: &Palette, zoom: f32, rc: Rect, value: Option<&Value>, style: Option<&CellStyle>) {
    if let Some(fill) = style.and_then(|s| s.fill) {
        p.rect_filled(rc, 0.0, rgb(fill));
    }
    let Some(value) = value else { return };
    if *value == Value::Null {
        return;
    }
    let plain = CellStyle::default();
    let st = style.unwrap_or(&plain);
    let (text, color, numeric) = match numfmt::display(value, &st.numfmt) {
        Some(s) => (s.text, s.color.map(|c| Color32::from_rgb(c[0], c[1], c[2])), s.numeric),
        None => (textcell::to_text(value), None, matches!(value, Value::Int(_) | Value::Float(_) | Value::Date(_))),
    };
    let color = color.or_else(|| st.color.map(rgb)).unwrap_or(pal.text);
    let align = match st.align {
        HAlign::General if numeric => HAlign::Right,
        HAlign::General if matches!(value, Value::Bool(_)) => HAlign::Center,
        HAlign::General => HAlign::Left,
        other => other,
    };
    let pad = 6.0 * zoom;
    let stroke = |on: bool| if on { Stroke::new(1.0, color) } else { Stroke::NONE };
    let mut job = LayoutJob::single_section(
        text,
        TextFormat { font_id: FontId::proportional(font_px(st, zoom)), color, italics: st.italic, underline: stroke(st.underline), strikethrough: stroke(st.strike), ..TextFormat::default() },
    );
    if st.wrap {
        job.wrap.max_width = (rc.width() - 2.0 * pad).max(1.0);
    }
    let galley = p.layout_job(job);
    let size = galley.size();
    let x = match align {
        HAlign::Right => rc.max.x - pad - size.x,
        HAlign::Center => rc.center().x - size.x / 2.0,
        _ => rc.min.x + pad,
    };
    let y = if st.wrap { rc.min.y + 2.0 * zoom } else { rc.center().y - size.y / 2.0 };
    let clipped = p.with_clip_rect(rc.intersect(p.clip_rect()));
    clipped.galley(Pos2::new(x, y), galley.clone(), color);
    if st.bold {
        clipped.galley(Pos2::new(x + 0.7 * zoom, y), galley, color);
    }
}

/// The drop-down button of a filtered title cell.
fn arrow_rect(rc: Rect) -> Rect {
    let s = (rc.height() - 4.0).clamp(8.0, 20.0).min((rc.width() - 2.0).max(8.0));
    Rect::from_min_size(Pos2::new(rc.max.x - s - 2.0, rc.center().y - s / 2.0), Vec2::splat(s))
}

/// What dragging the fill handle from `src` to the cell under the pointer would fill: down when the
/// pointer went at least as far down as to the right, otherwise right. `None` inside the selection
/// or above and to the left of it.
pub(super) fn fill_target_of(src: Range, hover: Cell) -> Option<(bool, Range)> {
    let down_by = i64::from(hover.0) - i64::from(src.2);
    let right_by = i64::from(hover.1) - i64::from(src.3);
    if down_by <= 0 && right_by <= 0 {
        return None;
    }
    if down_by >= right_by { Some((true, (src.2 + 1, src.1, hover.0, src.3))) } else { Some((false, (src.0, src.3 + 1, src.2, hover.1))) }
}

/// A scroll bar along one axis. `scroll` is the offset of the scrolling part; `frozen` the length
/// of the frozen part and `view` the visible length (both in sheet units).
fn scrollbar(ui: &mut egui::Ui, pal: &Palette, id: egui::Id, track: Rect, vertical: bool, scroll: &mut f64, frozen: f64, view: f64, total: f64) {
    let room = (view - frozen).max(1.0);
    let span = (total - frozen).max(room);
    let max_scroll = span - room;
    let track_len = if vertical { track.height() } else { track.width() };
    let thumb_len = (track_len * (room / span) as f32).clamp(24.0_f32.min(track_len), track_len);
    let travel = (track_len - thumb_len).max(0.0);
    let frac = if max_scroll > 0.0 { ((*scroll - frozen) / max_scroll).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let thumb = if vertical {
        Rect::from_min_size(Pos2::new(track.min.x, track.min.y + frac * travel), Vec2::new(track.width(), thumb_len))
    } else {
        Rect::from_min_size(Pos2::new(track.min.x + frac * travel, track.min.y), Vec2::new(thumb_len, track.height()))
    };
    let response = ui.interact(track, id, Sense::click_and_drag());
    if response.dragged() && travel > 0.0 {
        let d = if vertical { response.drag_delta().y } else { response.drag_delta().x };
        *scroll += f64::from(d) * max_scroll / f64::from(travel);
    } else if response.clicked()
        && let Some(p) = response.interact_pointer_pos()
        && !thumb.contains(p)
    {
        let before = if vertical { p.y < thumb.min.y } else { p.x < thumb.min.x };
        *scroll += if before { -room } else { room };
    }
    let painter = ui.painter();
    painter.rect_filled(track, 0.0, pal.header);
    let thumb = thumb.shrink(2.0);
    let color = if response.dragged() || response.hovered() { pal.header_text } else { pal.line };
    painter.rect_filled(thumb, 3.0, color);
}

impl DataView {
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    pub(super) fn grid(&mut self, ui: &mut egui::Ui, maix: &mut Maix, sheet: u16, now: u64, height: f32, batch: &mut Vec<Change>) {
        let pal = Palette::of(ui);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
        let grid_id = response.id;
        let zoom = self.zoom;
        let z = f64::from(zoom);
        let head_w = (HEADER_W * zoom).round();
        let head_h = (HEADER_H * zoom).round();
        let body = Rect::from_min_max(rect.min + Vec2::new(head_w, head_h), rect.max - Vec2::splat(SB));
        if body.width() < 20.0 || body.height() < 20.0 {
            return;
        }
        let area = Rect::from_min_max(rect.min, rect.max - Vec2::splat(SB));

        // ---- sizes, and how far the scroll bars reach
        let used = self.used(maix, sheet);
        let (rows_l, cols_l, fr, fc) = {
            let Ok(sh) = maix.sheet(sheet) else { return };
            let lay = &sh.format.layout;
            (
                Lines::new(DEFAULT_ROW_H, lay.row_heights.iter().map(|(r, h)| (*r, *h)).chain(lay.hidden_rows.iter().map(|r| (*r, 0.0)))),
                Lines::new(DEFAULT_COL_W, lay.col_widths.iter().map(|(c, w)| (u32::from(*c), *w)).chain(lay.hidden_cols.iter().map(|c| (u32::from(*c), 0.0)))),
                lay.freeze_rows,
                u32::from(lay.freeze_cols),
            )
        };
        let max_cols = u32::from(MAX_COLS);
        self.rows_ext = self.rows_ext.max(MIN_ROWS.max(used.0.saturating_add(60)).max(self.focus.0.saturating_add(40)).max(fr + 20)).min(MAX_ROWS);
        self.cols_ext = self.cols_ext.max(MIN_COLS.max(u32::from(used.1) + 8).max(u32::from(self.focus.1) + 8).max(fc + 5)).min(max_cols);

        // ---- wheel scrolling and zoom
        if ui.rect_contains_pointer(rect) {
            let (delta, zoom_by) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if (zoom_by - 1.0).abs() > f32::EPSILON {
                self.zoom = (self.zoom * zoom_by).clamp(super::MIN_ZOOM, super::MAX_ZOOM);
            } else {
                self.scroll.0 -= f64::from(delta.x) / z;
                self.scroll.1 -= f64::from(delta.y) / z;
            }
        }
        let view_h = f64::from(body.height()) / z;
        let view_w = f64::from(body.width()) / z;
        let (frozen_h, frozen_w) = (rows_l.start(fr), cols_l.start(fc));
        if self.scroll.1 + (view_h - frozen_h) > rows_l.total(self.rows_ext) - 3.0 * f64::from(DEFAULT_ROW_H) && self.rows_ext < MAX_ROWS {
            self.rows_ext = (self.rows_ext + (self.rows_ext / 8).max(200)).min(MAX_ROWS);
        }
        if self.scroll.0 + (view_w - frozen_w) > cols_l.total(self.cols_ext) - 3.0 * f64::from(DEFAULT_COL_W) && self.cols_ext < max_cols {
            self.cols_ext = (self.cols_ext + (self.cols_ext / 8).max(8)).min(max_cols);
        }
        if self.scroll_to_focus {
            self.scroll.1 = scroll_to_show(&rows_l, fr, self.scroll.1, view_h, self.focus.0);
            self.scroll.0 = scroll_to_show(&cols_l, fc, self.scroll.0, view_w, u32::from(self.focus.1));
            self.scroll_to_focus = false;
        }
        let (total_h, total_w) = (rows_l.total(self.rows_ext), cols_l.total(self.cols_ext));
        self.scroll.1 = self.scroll.1.clamp(frozen_h, (total_h - (view_h - frozen_h)).max(frozen_h));
        self.scroll.0 = self.scroll.0.clamp(frozen_w, (total_w - (view_w - frozen_w)).max(frozen_w));

        let rf = Frame { lines: &rows_l, freeze: fr, scroll: self.scroll.1, zoom: z, origin: f64::from(body.min.y) };
        let cf = Frame { lines: &cols_l, freeze: fc, scroll: self.scroll.0, zoom: z, origin: f64::from(body.min.x) };
        let (rows_ext, cols_ext) = (self.rows_ext, self.cols_ext);
        let cell_rect = |r: u32, c: u16| Rect::from_min_size(Pos2::new(cf.pos(u32::from(c)), rf.pos(r)), Vec2::new(cf.len_of(u32::from(c)), rf.len_of(r)));
        let col_at = |x: f32| u16::try_from(cf.index_at(x, cols_ext)).unwrap_or(0);
        let hit_at = |p: Pos2| -> Option<Hit> {
            if !area.contains(p) {
                return None;
            }
            let (in_col_head, in_row_head) = (p.y < body.min.y, p.x < body.min.x);
            let c = col_at(p.x.max(body.min.x));
            let r = rf.index_at(p.y.max(body.min.y), rows_ext);
            Some(match (in_col_head, in_row_head) {
                (true, true) => Hit::Corner,
                (true, false) => Hit::Column(c),
                (false, true) => Hit::Row(r),
                (false, false) => Hit::Cell((r, c)),
            })
        };
        // The header edge under `p`: (columns?, the line it resizes).
        let edge_at = |p: Pos2| -> Option<(bool, u32)> {
            if p.y >= rect.min.y && p.y < body.min.y && p.x >= body.min.x && p.x <= body.max.x {
                let c = cf.index_at(p.x, cols_ext);
                if (p.x - (cf.pos(c) + cf.len_of(c))).abs() <= EDGE {
                    return Some((true, c));
                }
                let prev = cols_l.prev_visible(c);
                if prev != c && (p.x - cf.pos(c)).abs() <= EDGE {
                    return Some((true, prev));
                }
            }
            if p.x >= rect.min.x && p.x < body.min.x && p.y >= body.min.y && p.y <= body.max.y {
                let r = rf.index_at(p.y, rows_ext);
                if (p.y - (rf.pos(r) + rf.len_of(r))).abs() <= EDGE {
                    return Some((false, r));
                }
                let prev = rows_l.prev_visible(r);
                if prev != r && (p.y - rf.pos(r)).abs() <= EDGE {
                    return Some((false, prev));
                }
            }
            None
        };

        let fview = self.filter_view(sheet);
        let arrow_hit = |p: Pos2| -> Option<u16> {
            let (range, _) = fview.as_ref()?;
            let Some(Hit::Cell((r, c))) = hit_at(p) else { return None };
            (r == range.0 && (range.1..=range.3).contains(&c) && arrow_rect(cell_rect(r, c)).contains(p)).then_some(c)
        };
        // The fill handle: the small square at the corner of the selection, when that corner is in view.
        let fill_handle: Option<Rect> = {
            let s = self.sel();
            let (rows_ok, cols_ok) = (rf.len_of(s.2) > 0.0 && cf.len_of(u32::from(s.3)) > 0.0, !whole_cols(s) && !whole_rows(s));
            let corner = cell_rect(s.2, s.3).max;
            let in_view = corner.x <= body.max.x + 0.5
                && corner.y <= body.max.y + 0.5
                && (s.2 < fr || corner.y > rf.scroll_start() + 2.0)
                && (u32::from(s.3) < fc || corner.x > cf.scroll_start() + 2.0)
                && corner.x > body.min.x
                && corner.y > body.min.y;
            (rows_ok && cols_ok && in_view && self.edit.is_none()).then(|| Rect::from_center_size(corner, Vec2::splat((7.0 * zoom).clamp(6.0, 11.0))))
        };

        // ---- pointer
        let (pressed, down, ptr, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers.shift));
        let editing = self.edit.as_ref().map(|e| cell_rect(e.cell.0, e.cell.1));
        if let Some(p) = ui.input(|i| i.pointer.hover_pos())
            && ui.rect_contains_pointer(rect)
            && self.drag.is_none()
            && let Some((cols, _)) = edge_at(p)
        {
            ui.ctx().set_cursor_icon(if cols { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
        }
        if let Some(p) = ui.input(|i| i.pointer.hover_pos())
            && (matches!(self.drag, Some(Drag::Fill { .. })) || (self.drag.is_none() && fill_handle.is_some_and(|h| h.expand(3.0).contains(p))))
        {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if pressed && response.hovered() && let Some(p) = ptr && editing.is_none_or(|e| !e.contains(p)) {
            self.addr = None;
            if fill_handle.is_some_and(|h| h.expand(3.0).contains(p)) {
                self.drag = Some(Drag::Fill { src: self.sel() });
                self.fill_target = None;
            } else if let Some(col) = arrow_hit(p) {
                self.open_filter_popup(maix, sheet, col);
            } else if let Some((cols, idx)) = edge_at(p) {
                let sel = self.sel();
                let (first, last) = if cols {
                    if whole_cols(sel) && (u32::from(sel.1)..=u32::from(sel.3)).contains(&idx) { (u32::from(sel.1), u32::from(sel.3)) } else { (idx, idx) }
                } else if whole_rows(sel) && (sel.0..=sel.2).contains(&idx) {
                    (sel.0, sel.2)
                } else {
                    (idx, idx)
                };
                let start = if cols { cols_l.size(idx) } else { rows_l.size(idx) } as f32;
                let before = maix.sheet(sheet).map(|s| Box::new(s.format.clone())).unwrap_or_default();
                self.drag = Some(Drag::Resize { cols, first, last, start, from: if cols { p.x } else { p.y }, before });
            } else {
                match hit_at(p) {
                    Some(Hit::Cell(cell)) => {
                        self.focus = cell;
                        if !shift {
                            self.anchor = cell;
                        }
                        self.drag = Some(Drag::Select);
                    }
                    Some(Hit::Column(c)) => {
                        if shift {
                            self.focus = (0, c);
                        } else {
                            self.anchor = (MAX_ROWS - 1, c);
                            self.focus = (0, c);
                        }
                        self.drag = Some(Drag::Header(true));
                    }
                    Some(Hit::Row(r)) => {
                        if shift {
                            self.focus = (r, 0);
                        } else {
                            self.anchor = (r, MAX_COLS - 1);
                            self.focus = (r, 0);
                        }
                        self.drag = Some(Drag::Header(false));
                    }
                    Some(Hit::Corner) => {
                        self.anchor = (MAX_ROWS - 1, MAX_COLS - 1);
                        self.focus = (0, 0);
                    }
                    None => {}
                }
            }
            ui.memory_mut(|m| m.request_focus(grid_id));
            self.bar = None;
        }
        if down && let Some(p) = ptr {
            let mut scroll_by = Vec2::ZERO;
            match &self.drag {
                Some(Drag::Select | Drag::Fill { .. }) => {
                    let q = Pos2::new(p.x.clamp(body.min.x, body.max.x - 1.0), p.y.clamp(body.min.y, body.max.y - 1.0));
                    let hover = (rf.index_at(q.y, rows_ext), col_at(q.x));
                    if let Some(Drag::Fill { src }) = &self.drag {
                        self.fill_target = fill_target_of(*src, hover);
                    } else {
                        self.focus = hover;
                    }
                    scroll_by = Vec2::new(
                        if p.x < body.min.x { p.x - body.min.x } else if p.x > body.max.x { p.x - body.max.x } else { 0.0 },
                        if p.y < body.min.y { p.y - body.min.y } else if p.y > body.max.y { p.y - body.max.y } else { 0.0 },
                    );
                }
                Some(Drag::Header(true)) => {
                    self.focus.1 = col_at(p.x.clamp(body.min.x, body.max.x - 1.0));
                    scroll_by.x = if p.x < body.min.x { p.x - body.min.x } else if p.x > body.max.x { p.x - body.max.x } else { 0.0 };
                }
                Some(Drag::Header(false)) => {
                    self.focus.0 = rf.index_at(p.y.clamp(body.min.y, body.max.y - 1.0), rows_ext);
                    scroll_by.y = if p.y < body.min.y { p.y - body.min.y } else if p.y > body.max.y { p.y - body.max.y } else { 0.0 };
                }
                Some(Drag::Resize { cols, first, last, start, from, .. }) => {
                    let size = (start + (if *cols { p.x } else { p.y } - from) / zoom).max(MIN_SIZE);
                    if let Ok(f) = maix.format_mut(sheet) {
                        if *cols {
                            f.layout.set_col_width(u16::try_from(*first).unwrap_or(0)..=u16::try_from(*last).unwrap_or(0), Some(size));
                        } else {
                            f.layout.set_row_height(*first..=*last, Some(size));
                        }
                    }
                }
                None => {}
            }
            if scroll_by != Vec2::ZERO {
                self.scroll.0 += f64::from(scroll_by.x) / z * 0.3;
                self.scroll.1 += f64::from(scroll_by.y) / z * 0.3;
                ui.ctx().request_repaint();
            }
        } else if !down && let Some(drag) = self.drag.take() {
            match drag {
                Drag::Resize { before, .. } => {
                    if let Ok(sh) = maix.sheet(sheet)
                        && sh.format != *before
                    {
                        self.steps.push(super::Step::Format { sheet, before, after: Box::new(sh.format.clone()) });
                    }
                }
                Drag::Fill { src } => {
                    if let Some((below, target)) = self.fill_target.take() {
                        self.fill_range(maix, sheet, src, target, below, now, batch);
                        self.anchor = (src.0, src.1);
                        self.focus = (src.2.max(target.2), src.3.max(target.3));
                    }
                }
                Drag::Select | Drag::Header(_) => {}
            }
        }
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let hit = hit_at(p);
            self.ctx_hit = hit;
            let sel = self.sel();
            match hit {
                Some(Hit::Cell(c)) if !(sel.0..=sel.2).contains(&c.0) || !(sel.1..=sel.3).contains(&c.1) => {
                    self.anchor = c;
                    self.focus = c;
                }
                Some(Hit::Column(c)) if !(whole_cols(sel) && (sel.1..=sel.3).contains(&c)) => {
                    self.anchor = (MAX_ROWS - 1, c);
                    self.focus = (0, c);
                }
                Some(Hit::Row(r)) if !(whole_rows(sel) && (sel.0..=sel.2).contains(&r)) => {
                    self.anchor = (r, MAX_COLS - 1);
                    self.focus = (r, 0);
                }
                Some(Hit::Corner) => {
                    self.anchor = (MAX_ROWS - 1, MAX_COLS - 1);
                    self.focus = (0, 0);
                }
                _ => {}
            }
            ui.memory_mut(|m| m.request_focus(grid_id));
        }
        if response.double_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            if let Some((cols, idx)) = edge_at(p) {
                self.autofit(ui, maix, sheet, cols, idx);
            } else if let Some(Hit::Cell(cell)) = hit_at(p) {
                self.focus = cell;
                self.anchor = cell;
                self.begin_edit(maix, sheet, None);
            }
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

        // ---- cells
        let (r_frozen, r_scroll) = (rf.frozen(rows_ext), rf.visible(body.max.y, rows_ext));
        let (c_frozen, c_scroll) = (cf.frozen(cols_ext), cf.visible(body.max.x, cols_ext));
        let (split_x, split_y) = (cf.scroll_start(), rf.scroll_start());
        let quads = [
            (r_frozen.clone(), c_frozen.clone(), Rect::from_min_max(body.min, Pos2::new(split_x, split_y))),
            (r_frozen.clone(), c_scroll.clone(), Rect::from_min_max(Pos2::new(split_x, body.min.y), Pos2::new(body.max.x, split_y))),
            (r_scroll.clone(), c_frozen.clone(), Rect::from_min_max(Pos2::new(body.min.x, split_y), Pos2::new(split_x, body.max.y))),
            (r_scroll.clone(), c_scroll.clone(), Rect::from_min_max(Pos2::new(split_x, split_y), body.max)),
        ];
        let sel = self.sel();
        let line = Stroke::new(1.0, pal.line);
        {
            let Ok(sh) = maix.sheet(sheet) else { return };
            let dim = maix.vectors.find(&maix.active_dims);
            for (rr, cc, clip) in &quads {
                if rr.is_empty() || cc.is_empty() || !clip.is_positive() {
                    continue;
                }
                let p = ui.painter_at(*clip);
                p.rect_filled(*clip, 0.0, pal.cell);
                for (s, r) in &self.linked {
                    if *s == sheet {
                        let area = Rect::from_min_max(cell_rect(r.0, r.1).min, cell_rect(r.2, r.3).max);
                        p.rect_filled(area, 0.0, pal.select_line.gamma_multiply(0.12));
                    }
                }
                if self.gridlines {
                    for r in rr.clone() {
                        let h = rf.len_of(r);
                        if h > 0.0 {
                            p.hline(clip.x_range(), rf.pos(r) + h, line);
                        }
                    }
                    for c in cc.clone() {
                        let w = cf.len_of(c);
                        if w > 0.0 {
                            p.vline(cf.pos(c) + w, clip.y_range(), line);
                        }
                    }
                }
                for r in rr.clone() {
                    let (y, h) = (rf.pos(r), rf.len_of(r));
                    if h <= 0.0 {
                        continue;
                    }
                    for c in cc.clone() {
                        let (x, w) = (cf.pos(c), cf.len_of(c));
                        if w <= 0.0 {
                            continue;
                        }
                        let Ok(col) = u16::try_from(c) else { continue };
                        let value = dim.and_then(|d| sh.get(r, col, d)).map(|cell| &cell.value);
                        let style = sh.format.get(sh.format.id_at(r, col));
                        if value.is_some() || style.is_some() {
                            paint_cell(&p, &pal, zoom, Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)), value, style);
                        }
                        if let Some((range, active)) = &fview
                            && r == range.0
                            && (range.1..=range.3).contains(&col)
                            && w > 26.0 * zoom
                        {
                            let ar = arrow_rect(Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)));
                            p.rect_filled(ar, 2.0, if active.contains(&col) { pal.select_line.gamma_multiply(0.35) } else { pal.header });
                            p.rect_stroke(ar, 2.0, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
                            let (c, k) = (ar.center(), ar.width() * 0.22);
                            p.add(egui::Shape::convex_polygon(vec![Pos2::new(c.x - k, c.y - k * 0.5), Pos2::new(c.x + k, c.y - k * 0.5), Pos2::new(c.x, c.y + k * 0.8)], pal.header_text, Stroke::NONE));
                        }
                    }
                }
                let sel_rect = Rect::from_min_max(cell_rect(sel.0, sel.1).min, cell_rect(sel.2, sel.3).max);
                if cell_count(sel) > 1 {
                    p.rect_filled(sel_rect, 0.0, pal.select_fill);
                    p.rect_stroke(sel_rect, 0.0, Stroke::new(1.5, pal.select_line), egui::StrokeKind::Inside);
                }
                p.rect_stroke(cell_rect(self.focus.0, self.focus.1), 0.0, Stroke::new(2.0, pal.select_line), egui::StrokeKind::Inside);
            }
        }

        // ---- the fill handle and, while it is dragged, where the fill would reach
        {
            let p = ui.painter_at(body);
            if let (Some((_, target)), Some(Drag::Fill { src })) = (&self.fill_target, &self.drag) {
                let area = Rect::from_min_max(cell_rect(src.0.min(target.0), src.1.min(target.1)).min, cell_rect(src.2.max(target.2), src.3.max(target.3)).max);
                p.rect_stroke(area, 0.0, Stroke::new(1.5, pal.select_line), egui::StrokeKind::Outside);
            }
            if let Some(h) = fill_handle {
                p.rect_filled(h.expand(1.0), 0.0, pal.cell);
                p.rect_filled(h, 0.0, pal.select_line);
            }
        }

        // ---- the headers stay in view
        {
            let hfont = FontId::proportional(12.0 * zoom);
            ui.painter().rect_filled(Rect::from_min_max(rect.min, body.min), 0.0, pal.header);
            let col_areas = [(c_frozen.clone(), Rect::from_min_max(Pos2::new(body.min.x, rect.min.y), Pos2::new(split_x, body.min.y))), (c_scroll.clone(), Rect::from_min_max(Pos2::new(split_x, rect.min.y), Pos2::new(body.max.x, body.min.y)))];
            for (range, clip) in col_areas {
                if range.is_empty() || !clip.is_positive() {
                    continue;
                }
                let p = ui.painter_at(clip);
                p.rect_filled(clip, 0.0, pal.header);
                for c in range {
                    let (x, w) = (cf.pos(c), cf.len_of(c));
                    if w <= 0.0 {
                        continue;
                    }
                    let Ok(col) = u16::try_from(c) else { continue };
                    let hr = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(w, head_h));
                    if (sel.1..=sel.3).contains(&col) {
                        p.rect_filled(hr, 0.0, if whole_cols(sel) { pal.select_fill.gamma_multiply(1.6) } else { pal.select_fill });
                    }
                    p.vline(x + w, hr.y_range(), line);
                    p.text(hr.center(), Align2::CENTER_CENTER, column_name(col), hfont.clone(), pal.header_text);
                }
            }
            let row_areas = [(r_frozen.clone(), Rect::from_min_max(Pos2::new(rect.min.x, body.min.y), Pos2::new(body.min.x, split_y))), (r_scroll.clone(), Rect::from_min_max(Pos2::new(rect.min.x, split_y), Pos2::new(body.min.x, body.max.y)))];
            for (range, clip) in row_areas {
                if range.is_empty() || !clip.is_positive() {
                    continue;
                }
                let p = ui.painter_at(clip);
                p.rect_filled(clip, 0.0, pal.header);
                for r in range {
                    let (y, h) = (rf.pos(r), rf.len_of(r));
                    if h <= 0.0 {
                        continue;
                    }
                    let hr = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(head_w, h));
                    if (sel.0..=sel.2).contains(&r) {
                        p.rect_filled(hr, 0.0, if whole_rows(sel) { pal.select_fill.gamma_multiply(1.6) } else { pal.select_fill });
                    }
                    p.hline(hr.x_range(), y + h, line);
                    p.text(hr.center(), Align2::CENTER_CENTER, (r + 1).to_string(), hfont.clone(), pal.header_text);
                }
            }
            let p = ui.painter();
            p.hline(rect.min.x..=area.max.x, body.min.y, line);
            p.vline(body.min.x, rect.min.y..=area.max.y, line);
            let split = Stroke::new(2.0, pal.header_text);
            if fr > 0 {
                p.hline(body.min.x..=body.max.x, split_y, split);
            }
            if fc > 0 {
                p.vline(split_x, body.min.y..=body.max.y, split);
            }
        }

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
            let font = FontId::proportional(FONT * zoom);
            let widget = egui::TextEdit::singleline(&mut edit.text).font(font).margin(egui::Margin::symmetric(5, 3)).desired_width(rc.width());
            let r = ui.put(Rect::from_min_size(rc.min, Vec2::new(rc.width().max(40.0), rc.height())), widget);
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
                if enter || tab {
                    ui.memory_mut(|m| m.request_focus(grid_id));
                    self.focus = edit.cell;
                    self.anchor = edit.cell;
                    if enter {
                        self.step(maix, sheet, if shift { -1 } else { 1 }, 0, false);
                    } else {
                        self.step(maix, sheet, 0, if shift { -1 } else { 1 }, false);
                    }
                }
            } else {
                self.edit = Some(edit);
            }
        }

        // ---- scroll bars
        let vbar = Rect::from_min_max(Pos2::new(rect.max.x - SB, body.min.y), Pos2::new(rect.max.x, rect.max.y - SB));
        let hbar = Rect::from_min_max(Pos2::new(body.min.x, rect.max.y - SB), Pos2::new(rect.max.x - SB, rect.max.y));
        scrollbar(ui, &pal, grid_id.with("vbar"), vbar, true, &mut self.scroll.1, frozen_h, view_h, total_h);
        scrollbar(ui, &pal, grid_id.with("hbar"), hbar, false, &mut self.scroll.0, frozen_w, view_w, total_w);
        ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.max.x - SB, rect.max.y - SB), rect.max), 0.0, pal.header);

        // ---- context menu (what it offers depends on what was right-clicked)
        response.context_menu(|ui| self.context_menu(ui, maix, sheet, now, batch));
    }
}
