//! A 3D view of the data: the store is a cube of rows, columns and the values of a dimension,
//! and this draws two of those three as the floor of a bar chart (the third is held at one value
//! you choose with a slider). Drag to turn it, scroll to zoom, hover a bar to read it.
//!
//! The drawing is plain egui shapes (a perspective projection and a back-to-front sort), so it
//! needs no graphics API and works the same on every platform, including the browser.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use alfadoc_model::Maix;
use egui::{Align2, Color32, FontId, Pos2, Rect as ScreenRect, Sense, Shape, Stroke, vec2};

use super::{Range, column_name, effective};

/// Most bars along one side.
pub(super) const MAX_SIDE: usize = 40;

/// One of the three directions of the cube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Along {
    Cols,
    Rows,
    Dim,
}

impl Along {
    const ALL: [Along; 3] = [Along::Cols, Along::Rows, Along::Dim];

    fn name(self, key: &str) -> String {
        match self {
            Along::Cols => "Columns".to_string(),
            Along::Rows => "Rows".to_string(),
            Along::Dim => if key.is_empty() { "Dimension".to_string() } else { key.to_string() },
        }
    }
}

/// What the floor of the chart is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Two dimensions: every value of each (and plain) is a place on the floor, the height is the
    /// numbers of the selection stored under it.
    Dimensions,
    /// Rows, columns and one dimension, two of them on the floor.
    Cells,
}

/// How the numbers of the selection under one place on the floor become one height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Agg {
    Sum,
    Average,
    Count,
}

impl Agg {
    const ALL: [Agg; 3] = [Agg::Sum, Agg::Average, Agg::Count];

    fn name(self) -> &'static str {
        match self {
            Agg::Sum => "Sum",
            Agg::Average => "Average",
            Agg::Count => "Count",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct CubeState {
    pub mode: Mode,
    /// The two dimensions of the floor in [`Mode::Dimensions`] (the second may be empty).
    pub x_key: String,
    pub z_key: String,
    pub agg: Agg,
    pub open: bool,
    /// Across the floor, and into the screen.
    pub x: Along,
    pub z: Along,
    /// The dimension whose values form the third direction of the cube.
    pub key: String,
    /// Where the direction that is not on the floor is held.
    pub fixed: usize,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for CubeState {
    fn default() -> Self {
        CubeState { mode: Mode::Cells, x_key: String::new(), z_key: String::new(), agg: Agg::Sum, open: false, x: Along::Cols, z: Along::Rows, key: String::new(), fixed: 0, yaw: -0.6, pitch: 0.55, zoom: 1.0 }
    }
}

impl CubeState {
    fn reset_view(&mut self) {
        let fresh = CubeState::default();
        self.yaw = fresh.yaw;
        self.pitch = fresh.pitch;
        self.zoom = fresh.zoom;
    }

    /// The floor's two directions, made different from each other.
    fn floor(&self) -> (Along, Along) {
        if self.x != self.z {
            return (self.x, self.z);
        }
        let other = Along::ALL.into_iter().find(|a| *a != self.x).unwrap_or(Along::Rows);
        (self.x, other)
    }

    /// The direction held still.
    fn third(&self) -> Along {
        let (x, z) = self.floor();
        Along::ALL.into_iter().find(|a| *a != x && *a != z).unwrap_or(Along::Dim)
    }
}

/// The numbers to draw.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cube {
    pub x_labels: Vec<String>,
    pub z_labels: Vec<String>,
    /// `heights[z][x]`; `None` where the cell holds no number.
    pub heights: Vec<Vec<Option<f64>>>,
    pub third: Along,
    pub third_len: usize,
    pub third_label: String,
    pub min: f64,
    pub max: f64,
    /// What the two floor directions are called.
    pub x_title: String,
    pub z_title: String,
}

fn number_of(v: &alfadoc_model::Value) -> Option<f64> {
    match v {
        alfadoc_model::Value::Int(i) => Some(*i as f64),
        alfadoc_model::Value::Float(f) if f.is_finite() => Some(*f),
        _ => None,
    }
}

/// How many positions a direction has.
fn length(along: Along, range: Range, values: &[String]) -> usize {
    let n = match along {
        Along::Cols => (range.3 - range.1) as usize + 1,
        Along::Rows => (range.2 - range.0) as usize + 1,
        Along::Dim => values.len().max(1),
    };
    n.min(MAX_SIDE)
}

fn label(along: Along, range: Range, values: &[String], i: usize) -> String {
    match along {
        Along::Cols => column_name(range.1.saturating_add(i as u16)),
        Along::Rows => (range.0 as usize + i + 1).to_string(),
        Along::Dim => values.get(i).cloned().unwrap_or_else(|| "(plain)".to_string()),
    }
}

/// Reads the cube for `range` of `sheet`. `values` are the values of `st.key`, in order.
pub(super) fn build(maix: &Maix, sheet: u16, range: Range, st: &CubeState, values: &[String]) -> Cube {
    let (ax, az) = st.floor();
    let third = st.third();
    let (nx, nz, n3) = (length(ax, range, values), length(az, range, values), length(third, range, values));
    let fixed = st.fixed.min(n3.saturating_sub(1));
    let mut heights = vec![vec![None; nx]; nz];
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (zi, line) in heights.iter_mut().enumerate() {
        for (xi, slot) in line.iter_mut().enumerate() {
            let (mut ri, mut ci, mut di) = (0usize, 0usize, 0usize);
            for (along, i) in [(ax, xi), (az, zi), (third, fixed)] {
                match along {
                    Along::Rows => ri = i,
                    Along::Cols => ci = i,
                    Along::Dim => di = i,
                }
            }
            let mut dims = maix.active_dims.clone();
            if !st.key.is_empty()
                && let Some(v) = values.get(di)
            {
                dims.insert(st.key.clone(), v.clone());
            }
            let row = range.0.saturating_add(ri as u32);
            let col = range.1.saturating_add(ci as u16);
            *slot = maix.get(sheet, row, col, &dims).ok().flatten().and_then(number_of);
            if let Some(h) = *slot {
                min = min.min(h);
                max = max.max(h);
            }
        }
    }
    if min > max {
        (min, max) = (0.0, 0.0);
    }
    Cube {
        x_labels: (0..nx).map(|i| label(ax, range, values, i)).collect(),
        z_labels: (0..nz).map(|i| label(az, range, values, i)).collect(),
        heights,
        third,
        third_len: n3,
        third_label: label(third, range, values, fixed),
        min,
        max,
        x_title: ax.name(&st.key),
        z_title: az.name(&st.key),
    }
}

/// The places on the floor along one dimension: plain, then each value.
fn places(catalog: &BTreeMap<String, BTreeSet<String>>, key: &str) -> Vec<Option<String>> {
    if key.is_empty() {
        return vec![None];
    }
    std::iter::once(None).chain(catalog.get(key).into_iter().flatten().map(|v| Some(v.clone()))).take(MAX_SIDE).collect()
}

/// Reads the chart of two dimensions: each place on the floor is one value of each dimension
/// (or plain), and the height is the numbers of `range` stored under exactly that vector, with
/// every other dimension as chosen now. One pass over the stored cells, whatever the number of places.
pub(super) fn build_dims(maix: &Maix, sheet: u16, range: Range, st: &CubeState, catalog: &BTreeMap<String, BTreeSet<String>>) -> Cube {
    let z_key = if st.z_key == st.x_key { "" } else { st.z_key.as_str() };
    let xs = places(catalog, &st.x_key);
    let zs = places(catalog, z_key);
    let mut base = maix.active_dims.clone();
    base.remove(&st.x_key);
    base.remove(z_key);
    // Which place each stored vector belongs to.
    let mut at: HashMap<u64, (usize, usize)> = HashMap::new();
    for (zi, zv) in zs.iter().enumerate() {
        for (xi, xv) in xs.iter().enumerate() {
            let mut dims = base.clone();
            if let Some(v) = xv {
                dims.insert(st.x_key.clone(), v.clone());
            }
            if let Some(v) = zv {
                dims.insert(z_key.to_string(), v.clone());
            }
            if let Some(hash) = maix.vectors.find(&dims) {
                at.insert(hash, (xi, zi));
            }
        }
    }
    let mut sum = vec![vec![0.0f64; xs.len()]; zs.len()];
    let mut count = vec![vec![0usize; xs.len()]; zs.len()];
    if let Ok(sh) = maix.sheet(sheet) {
        for (_, _, hash, cell) in sh.cells_in(range.0, range.1, range.2, range.3) {
            if let (Some(&(xi, zi)), Some(n)) = (at.get(&hash), number_of(&cell.value))
                && let (Some(s), Some(c)) = (sum.get_mut(zi).and_then(|l| l.get_mut(xi)), count.get_mut(zi).and_then(|l| l.get_mut(xi)))
            {
                *s += n;
                *c += 1;
            }
        }
    }
    let mut heights = vec![vec![None; xs.len()]; zs.len()];
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for zi in 0..zs.len() {
        for xi in 0..xs.len() {
            let (n, s) = (count[zi][xi], sum[zi][xi]);
            let h = match st.agg {
                Agg::Count => (n > 0).then_some(n as f64),
                Agg::Sum => (n > 0).then_some(s),
                Agg::Average => (n > 0).then(|| s / n as f64),
            };
            heights[zi][xi] = h;
            if let Some(h) = h {
                min = min.min(h);
                max = max.max(h);
            }
        }
    }
    if min > max {
        (min, max) = (0.0, 0.0);
    }
    let name = |v: &Option<String>| v.clone().unwrap_or_else(|| "plain".to_string());
    let held: Vec<String> = base.iter().map(|(k, v)| format!("{k}={v}")).collect();
    Cube {
        x_labels: xs.iter().map(name).collect(),
        z_labels: zs.iter().map(name).collect(),
        heights,
        third: Along::Dim,
        third_len: 1,
        third_label: if held.is_empty() { "no other dimension chosen".to_string() } else { held.join(", ") },
        min,
        max,
        x_title: if st.x_key.is_empty() { "plain".to_string() } else { st.x_key.clone() },
        z_title: if z_key.is_empty() { String::new() } else { z_key.to_string() },
    }
}

// ---- projection -------------------------------------------------------------------------------

/// A perspective camera looking at the middle of the floor.
#[derive(Clone, Copy, Debug)]
pub(super) struct Camera {
    yaw: f32,
    pitch: f32,
    centre: [f32; 3],
    distance: f32,
    focal: f32,
    screen: Pos2,
}

impl Camera {
    pub(super) fn new(yaw: f32, pitch: f32, zoom: f32, centre: [f32; 3], radius: f32, screen: Pos2, size: f32) -> Camera {
        let radius = (radius / zoom.max(0.05)).max(0.5);
        let distance = radius * 2.6;
        Camera { yaw, pitch, centre, distance, focal: 0.45 * size * distance / radius, screen }
    }

    /// Turns a point about the middle of the floor: first about the vertical, then tilts.
    fn turn(&self, p: [f32; 3]) -> [f32; 3] {
        let (x, y, z) = (p[0] - self.centre[0], p[1] - self.centre[1], p[2] - self.centre[2]);
        let (sy, cy) = self.yaw.sin_cos();
        let (x1, z1) = (x * cy + z * sy, -x * sy + z * cy);
        let (sp, cp) = self.pitch.sin_cos();
        [x1, y * cp + z1 * sp, -y * sp + z1 * cp]
    }

    /// The screen position of a point and its distance from the camera, or `None` behind it.
    pub(super) fn project(&self, p: [f32; 3]) -> Option<(Pos2, f32)> {
        let t = self.turn(p);
        let depth = t[2] + self.distance;
        if depth < 0.1 {
            return None;
        }
        Some((Pos2::new(self.screen.x + self.focal * t[0] / depth, self.screen.y - self.focal * t[1] / depth), depth))
    }

    /// Whether a face with outward direction `normal` and middle `at` faces the camera.
    fn faces_camera(&self, at: [f32; 3], normal: [f32; 3]) -> bool {
        let c = self.turn(at);
        let n = {
            let o = self.turn([self.centre[0] + normal[0], self.centre[1] + normal[1], self.centre[2] + normal[2]]);
            [o[0], o[1], o[2]]
        };
        let origin = self.turn(self.centre);
        let n = [n[0] - origin[0], n[1] - origin[1], n[2] - origin[2]];
        let to_camera = [-c[0], -c[1], -self.distance - c[2]];
        n[0] * to_camera[0] + n[1] * to_camera[1] + n[2] * to_camera[2] > 0.0
    }
}

/// Whether `p` is inside the convex polygon `poly` (either winding).
pub(super) fn inside(poly: &[Pos2], p: Pos2) -> bool {
    let (mut pos, mut neg) = (false, false);
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross > 0.0 {
            pos = true;
        } else if cross < 0.0 {
            neg = true;
        }
    }
    !(pos && neg)
}

struct Face {
    depth: f32,
    poly: [Pos2; 4],
    colour: Color32,
    bar: (usize, usize),
}

fn shade(c: Color32, k: f32) -> Color32 {
    let m = |v: u8| (f32::from(v) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

fn ramp(t: f32) -> Color32 {
    let (a, b) = ((70.0, 130.0, 210.0), (240.0, 150.0, 50.0));
    let l = |x: f32, y: f32| (x + (y - x) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// The visible faces of every bar, far ones first.
fn faces(cube: &Cube, cam: &Camera, height: f32) -> Vec<Face> {
    let span = (cube.max.abs().max(cube.min.abs())).max(f64::MIN_POSITIVE);
    let mut out = vec![];
    for (zi, line) in cube.heights.iter().enumerate() {
        for (xi, h) in line.iter().enumerate() {
            let Some(h) = *h else { continue };
            let y = (h / span) as f32 * height;
            let (lo, hi) = (y.min(0.0), y.max(0.0));
            if hi - lo < 1e-4 {
                continue;
            }
            let t = if cube.max > cube.min { ((h - cube.min) / (cube.max - cube.min)) as f32 } else { 0.5 };
            let base = ramp(t);
            let (x0, x1, z0, z1) = (xi as f32 + 0.12, xi as f32 + 0.88, zi as f32 + 0.12, zi as f32 + 0.88);
            let (xm, zm, ym) = ((x0 + x1) / 2.0, (z0 + z1) / 2.0, (lo + hi) / 2.0);
            // (corners, outward direction, middle of the face, shade)
            let sides: [([[f32; 3]; 4], [f32; 3], [f32; 3], f32); 5] = [
                ([[x0, hi, z0], [x1, hi, z0], [x1, hi, z1], [x0, hi, z1]], [0.0, 1.0, 0.0], [xm, hi, zm], 1.0),
                ([[x0, lo, z0], [x1, lo, z0], [x1, hi, z0], [x0, hi, z0]], [0.0, 0.0, -1.0], [xm, ym, z0], 0.72),
                ([[x0, lo, z1], [x1, lo, z1], [x1, hi, z1], [x0, hi, z1]], [0.0, 0.0, 1.0], [xm, ym, z1], 0.6),
                ([[x0, lo, z0], [x0, lo, z1], [x0, hi, z1], [x0, hi, z0]], [-1.0, 0.0, 0.0], [x0, ym, zm], 0.82),
                ([[x1, lo, z0], [x1, lo, z1], [x1, hi, z1], [x1, hi, z0]], [1.0, 0.0, 0.0], [x1, ym, zm], 0.66),
            ];
            for (corners, normal, at, k) in sides {
                if !cam.faces_camera(at, normal) {
                    continue;
                }
                let projected: Vec<(Pos2, f32)> = corners.iter().filter_map(|c| cam.project(*c)).collect();
                if projected.len() != 4 {
                    continue;
                }
                let depth = projected.iter().map(|p| p.1).sum::<f32>() / 4.0;
                out.push(Face { depth, poly: [projected[0].0, projected[1].0, projected[2].0, projected[3].0], colour: shade(base, k), bar: (xi, zi) });
            }
        }
    }
    out.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    out
}

impl super::DataView {
    pub(super) fn open_cube(&mut self, maix: &Maix) {
        self.cube.open = true;
        let catalog = maix.dimension_catalog();
        if self.cube.key.is_empty() {
            self.cube.key = maix.active_dims.keys().next().cloned().or_else(|| catalog.keys().next().cloned()).unwrap_or_default();
        }
        if !catalog.contains_key(&self.cube.x_key) {
            let mut keys = catalog.keys();
            self.cube.x_key = keys.next().cloned().unwrap_or_default();
            self.cube.z_key = keys.next().cloned().unwrap_or_default();
            // With dimensions to show, start with the picture of all of them.
            self.cube.mode = if catalog.is_empty() { Mode::Cells } else { Mode::Dimensions };
        }
    }

    pub(super) fn cube_window(&mut self, ctx: &egui::Context, maix: &Maix, sheet: u16) {
        if !self.cube.open {
            return;
        }
        let catalog = maix.dimension_catalog();
        let values: Vec<String> = catalog.get(&self.cube.key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        let sel = self.sel();
        let one_cell = sel.0 == sel.2 && sel.1 == sel.3;
        let mut st = self.cube.clone();
        if st.mode == Mode::Dimensions && catalog.is_empty() {
            st.mode = Mode::Cells;
        }
        // Rows and columns: one selected cell means the whole used area. Dimensions: it means that cell.
        let range = if one_cell && st.mode == Mode::Cells {
            let used = maix.sheet(sheet).map_or((0, 0), alfadoc_model::maix::Sheet::extent);
            (0, 0, used.0.saturating_sub(1), used.1.saturating_sub(1))
        } else {
            effective(maix, sheet, sel)
        };
        let mut open = true;
        egui::Window::new("3D view").open(&mut open).collapsible(false).default_size([720.0, 580.0]).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut st.mode, Mode::Dimensions, "Dimensions").on_hover_text("Every value of the chosen dimensions on the floor; the height is your selection's numbers under each");
                ui.radio_value(&mut st.mode, Mode::Cells, "Rows and columns").on_hover_text("The cells of the selection on the floor, for one value of a dimension");
                if ui.button("Reset view").clicked() {
                    st.reset_view();
                }
            });
            let cube = if st.mode == Mode::Dimensions {
                ui.horizontal_wrapped(|ui| {
                    let keys: Vec<&String> = catalog.keys().collect();
                    ui.label("Across");
                    egui::ComboBox::from_id_salt("cube_xkey").selected_text(&st.x_key).show_ui(ui, |ui| {
                        for k in &keys {
                            ui.selectable_value(&mut st.x_key, (*k).clone(), k.as_str());
                        }
                    });
                    ui.label("Depth");
                    let z_text = if st.z_key.is_empty() { "(none)".to_string() } else { st.z_key.clone() };
                    egui::ComboBox::from_id_salt("cube_zkey").selected_text(z_text).show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.z_key, String::new(), "(none)");
                        for k in keys.iter().filter(|k| ***k != st.x_key) {
                            ui.selectable_value(&mut st.z_key, (*k).clone(), k.as_str());
                        }
                    });
                    ui.label("Height");
                    egui::ComboBox::from_id_salt("cube_agg").selected_text(st.agg.name()).show_ui(ui, |ui| {
                        for a in Agg::ALL {
                            ui.selectable_value(&mut st.agg, a, a.name());
                        }
                    });
                });
                if st.z_key == st.x_key {
                    st.z_key = String::new();
                }
                let cube = build_dims(maix, sheet, range, &st, &catalog);
                ui.weak(format!("{} of the numbers in {} under each place; \"plain\" is the values with none of that dimension. Others held: {}.", st.agg.name().to_lowercase(), if one_cell { "the selected cell".to_string() } else { "the selection".to_string() }, cube.third_label));
                cube
            } else {
                let key = st.key.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Across");
                    along_box(ui, "cube_x", &mut st.x, &key, !catalog.is_empty());
                    ui.label("Depth");
                    along_box(ui, "cube_z", &mut st.z, &key, !catalog.is_empty());
                    if !catalog.is_empty() {
                        ui.label("Dimension");
                        egui::ComboBox::from_id_salt("cube_key").selected_text(&st.key).show_ui(ui, |ui| {
                            for k in catalog.keys() {
                                if ui.selectable_label(st.key == *k, k).clicked() {
                                    st.key = k.clone();
                                    st.fixed = 0;
                                }
                            }
                        });
                    }
                });
                let cube = build(maix, sheet, range, &st, &values);
                ui.horizontal(|ui| {
                    ui.label(format!("{} held at", cube.third_label_name(&st.key)));
                    let mut at = st.fixed.min(cube.third_len.saturating_sub(1));
                    if cube.third_len > 1 {
                        ui.add(egui::Slider::new(&mut at, 0..=cube.third_len - 1).show_value(false));
                        st.fixed = at;
                    }
                    ui.strong(&cube.third_label);
                    ui.weak("(the selection, or the used area when one cell is selected)");
                });
                cube
            };
            ui.weak(format!("{} × {} places", cube.x_labels.len(), cube.z_labels.len()));
            let size = vec2(ui.available_width().max(320.0), (ui.available_height() - 24.0).clamp(280.0, 520.0));
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            if response.dragged() {
                let d = response.drag_delta();
                st.yaw += d.x * 0.01;
                st.pitch = (st.pitch + d.y * 0.01).clamp(0.05, 1.5);
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    st.zoom = (st.zoom * (1.0 + scroll * 0.002)).clamp(0.3, 5.0);
                }
            }
            if response.double_clicked() {
                st.reset_view();
            }
            paint(ui, rect, &cube, &st, response.hover_pos());
            ui.weak("Drag to turn, scroll to zoom, double-click to reset. Only numbers are drawn.");
        });
        self.cube = st;
        self.cube.open = open;
    }
}

impl Cube {
    fn third_label_name(&self, key: &str) -> String {
        self.third.name(key)
    }
}

fn along_box(ui: &mut egui::Ui, id: &str, along: &mut Along, key: &str, has_dims: bool) {
    egui::ComboBox::from_id_salt(id).selected_text(along.name(key)).show_ui(ui, |ui| {
        for a in Along::ALL {
            if a == Along::Dim && !has_dims {
                continue;
            }
            if ui.selectable_label(*along == a, a.name(key)).clicked() {
                *along = a;
            }
        }
    });
}

fn paint(ui: &egui::Ui, rect: ScreenRect, cube: &Cube, st: &CubeState, hover: Option<Pos2>) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let (nx, nz) = (cube.x_labels.len().max(1) as f32, cube.z_labels.len().max(1) as f32);
    let height = nx.max(nz) * 0.6;
    let centre = [nx / 2.0, 0.0, nz / 2.0];
    let radius = (nx * nx + nz * nz + height * height).sqrt() / 2.0;
    let cam = Camera::new(st.yaw, st.pitch, st.zoom, centre, radius, rect.center(), rect.width().min(rect.height()));
    let ink = ui.visuals().text_color();
    let faint = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    // The floor.
    for i in 0..=(nx as usize) {
        if let (Some(a), Some(b)) = (cam.project([i as f32, 0.0, 0.0]), cam.project([i as f32, 0.0, nz])) {
            painter.line_segment([a.0, b.0], faint);
        }
    }
    for j in 0..=(nz as usize) {
        if let (Some(a), Some(b)) = (cam.project([0.0, 0.0, j as f32]), cam.project([nx, 0.0, j as f32])) {
            painter.line_segment([a.0, b.0], faint);
        }
    }
    // Names along the floor's edges (every few when there are many).
    let font = FontId::proportional(11.0);
    let step = |n: usize| n.div_ceil(12).max(1);
    for (i, name) in cube.x_labels.iter().enumerate().step_by(step(cube.x_labels.len())) {
        if let Some((p, _)) = cam.project([i as f32 + 0.5, 0.0, nz + 0.35]) {
            painter.text(p, Align2::CENTER_CENTER, name, font.clone(), ink);
        }
    }
    for (j, name) in cube.z_labels.iter().enumerate().step_by(step(cube.z_labels.len())) {
        if let Some((p, _)) = cam.project([-0.35, 0.0, j as f32 + 0.5]) {
            painter.text(p, Align2::CENTER_CENTER, name, font.clone(), ink);
        }
    }
    if let Some((p, _)) = cam.project([nx / 2.0, 0.0, nz + 0.95]) {
        painter.text(p, Align2::CENTER_CENTER, &cube.x_title, FontId::proportional(12.0), ink);
    }
    if !cube.z_title.is_empty()
        && let Some((p, _)) = cam.project([-0.95, 0.0, nz / 2.0])
    {
        painter.text(p, Align2::CENTER_CENTER, &cube.z_title, FontId::proportional(12.0), ink);
    }
    let all = faces(cube, &cam, height);
    for f in &all {
        painter.add(Shape::convex_polygon(f.poly.to_vec(), f.colour, Stroke::new(0.5, shade(f.colour, 0.55))));
    }
    if all.is_empty() {
        painter.text(rect.center(), Align2::CENTER_CENTER, "No numbers in this part of the data", FontId::proportional(14.0), ink);
    }
    // The bar under the pointer (nearest face first).
    if let Some(p) = hover
        && let Some(face) = all.iter().rev().find(|f| inside(&f.poly, p))
    {
        let (xi, zi) = face.bar;
        let value = cube.heights.get(zi).and_then(|l| l.get(xi)).copied().flatten().unwrap_or(0.0);
        let text = format!("{} × {}: {}", cube.x_labels.get(xi).map_or("", String::as_str), cube.z_labels.get(zi).map_or("", String::as_str), super::number(value));
        let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), ink);
        let at = Align2::LEFT_TOP.anchor_size(p + vec2(14.0, 14.0), galley.size()).expand(5.0);
        painter.rect_filled(at, 3.0, ui.visuals().window_fill);
        painter.rect_stroke(at, 3.0, Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
        painter.galley(at.min + vec2(5.0, 5.0), galley, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alfadoc_model::{DimVector, Stamp, Value};
    use std::collections::{BTreeMap, BTreeSet};

    fn store() -> (Maix, u16) {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        let by = |v: &str| -> DimVector { [("scenario".to_string(), v.to_string())].into_iter().collect() };
        for (r, c, actual, budget) in [(0u32, 0u16, 10, 12), (0, 1, 20, 18), (1, 0, 30, 33), (1, 1, 40, 41)] {
            m.set(s, r, c, &by("actual"), Value::Int(actual), &Stamp::new("t", 1)).unwrap();
            m.set(s, r, c, &by("budget"), Value::Int(budget), &Stamp::new("t", 2)).unwrap();
        }
        m.set(s, 0, 2, &DimVector::new(), Value::Text("label".into()), &Stamp::new("t", 3)).unwrap();
        (m, s)
    }

    fn values() -> Vec<String> {
        vec!["actual".into(), "budget".into()]
    }

    #[test]
    fn the_floor_is_columns_by_rows_with_the_dimension_held() {
        let (m, s) = store();
        let mut st = CubeState { key: "scenario".into(), ..Default::default() };
        let cube = build(&m, s, (0, 0, 1, 1), &st, &values());
        assert_eq!(cube.third, Along::Dim);
        assert_eq!(cube.x_labels, ["A", "B"]);
        assert_eq!(cube.z_labels, ["1", "2"]);
        assert_eq!(cube.heights, vec![vec![Some(10.0), Some(20.0)], vec![Some(30.0), Some(40.0)]]);
        assert_eq!(cube.third_label, "actual");
        st.fixed = 1;
        let cube = build(&m, s, (0, 0, 1, 1), &st, &values());
        assert_eq!(cube.heights[0][0], Some(12.0), "held at the second value");
        assert_eq!((cube.min, cube.max), (12.0, 41.0));
    }

    #[test]
    fn a_dimension_can_be_one_of_the_floor_directions() {
        let (m, s) = store();
        // Across the values of scenario, depth along the columns, held at row 2.
        let st = CubeState { key: "scenario".into(), x: Along::Dim, z: Along::Cols, fixed: 1, ..Default::default() };
        let cube = build(&m, s, (0, 0, 1, 1), &st, &values());
        assert_eq!(cube.third, Along::Rows);
        assert_eq!(cube.x_labels, ["actual", "budget"]);
        assert_eq!(cube.heights, vec![vec![Some(30.0), Some(33.0)], vec![Some(40.0), Some(41.0)]]);
        assert_eq!(cube.third_label, "2");
    }

    #[test]
    fn text_and_empty_cells_have_no_height_and_equal_directions_are_separated() {
        let (m, s) = store();
        let st = CubeState { x: Along::Cols, z: Along::Cols, key: "scenario".into(), ..Default::default() };
        assert_ne!(st.floor().0, st.floor().1);
        let plain = CubeState::default();
        let cube = build(&m, s, (0, 0, 1, 2), &plain, &[]);
        assert_eq!(cube.heights[0][2], None, "text is not a number");
        assert_eq!(cube.heights[0][0], None, "nothing is stored under the plain vector");
        assert_eq!((cube.min, cube.max), (0.0, 0.0));
        assert_eq!(cube.third_label, "(plain)");
    }

    #[test]
    fn a_big_selection_is_cut_to_the_bar_limit() {
        let (m, s) = store();
        let cube = build(&m, s, (0, 0, 9_999, 999), &CubeState::default(), &[]);
        assert_eq!((cube.x_labels.len(), cube.z_labels.len()), (MAX_SIDE, MAX_SIDE));
    }

    #[test]
    fn the_middle_of_the_floor_is_the_middle_of_the_screen() {
        let cam = Camera::new(0.7, 0.5, 1.0, [2.0, 0.0, 3.0], 5.0, Pos2::new(100.0, 80.0), 200.0);
        let (p, depth) = cam.project([2.0, 0.0, 3.0]).unwrap();
        assert!((p.x - 100.0).abs() < 1e-3 && (p.y - 80.0).abs() < 1e-3);
        assert!((depth - 13.0).abs() < 1e-3, "the camera sits 2.6 radii away");
        // Higher points are higher on the screen (smaller y).
        let (up, _) = cam.project([2.0, 2.0, 3.0]).unwrap();
        assert!(up.y < p.y);
    }

    #[test]
    fn only_faces_turned_to_the_camera_are_drawn() {
        let cam = Camera::new(0.0, 0.5, 1.0, [1.0, 0.0, 1.0], 3.0, Pos2::ZERO, 200.0);
        assert!(cam.faces_camera([1.0, 1.0, 1.0], [0.0, 1.0, 0.0]), "tops are seen from above");
        assert!(cam.faces_camera([1.0, 0.5, 0.0], [0.0, 0.0, -1.0]), "the side toward the camera");
        assert!(!cam.faces_camera([1.0, 0.5, 2.0], [0.0, 0.0, 1.0]), "the side away from it");
        let below = Camera::new(0.0, -0.5, 1.0, [1.0, 0.0, 1.0], 3.0, Pos2::ZERO, 200.0);
        assert!(!below.faces_camera([1.0, 1.0, 1.0], [0.0, 1.0, 0.0]), "not from below");
    }

    #[test]
    fn a_bar_chart_has_faces_far_ones_first_and_the_pointer_finds_a_bar() {
        let (m, s) = store();
        let cube = build(&m, s, (0, 0, 1, 1), &CubeState { key: "scenario".into(), ..Default::default() }, &values());
        let cam = Camera::new(-0.6, 0.55, 1.0, [1.0, 0.0, 1.0], 2.0, Pos2::new(300.0, 200.0), 400.0);
        let all = faces(&cube, &cam, 1.2);
        assert!(all.len() >= 8, "each of four bars shows at least its top and one side");
        assert!(all.windows(2).all(|w| w[0].depth >= w[1].depth));
        let nearest = all.last().unwrap();
        let mid = Pos2::new(nearest.poly.iter().map(|p| p.x).sum::<f32>() / 4.0, nearest.poly.iter().map(|p| p.y).sum::<f32>() / 4.0);
        assert!(inside(&nearest.poly, mid));
        assert!(!inside(&nearest.poly, Pos2::new(-500.0, -500.0)));
    }

    #[test]
    fn two_dimensions_with_their_plain_make_the_floor_and_nothing_crosses() {
        let mut m = Maix::default();
        let s = m.add_sheet("S");
        let one = |k: &str, v: &str| -> DimVector { [(k.to_string(), v.to_string())].into_iter().collect() };
        let both = |a: &str, b: &str| -> DimVector { [("default".to_string(), a.to_string()), ("alfa".to_string(), b.to_string())].into_iter().collect() };
        let stamp = Stamp::new("t", 1);
        m.set(s, 0, 0, &DimVector::new(), Value::Int(1), &stamp).unwrap();
        m.set(s, 0, 0, &one("default", "d1"), Value::Int(10), &stamp).unwrap();
        m.set(s, 0, 0, &one("default", "d2"), Value::Int(20), &stamp).unwrap();
        m.set(s, 0, 0, &one("alfa", "alfa1"), Value::Int(100), &stamp).unwrap();
        m.set(s, 0, 0, &one("alfa", "alfa2"), Value::Int(200), &stamp).unwrap();
        m.set(s, 0, 0, &both("d1", "alfa1"), Value::Int(1000), &stamp).unwrap();
        m.set(s, 1, 0, &one("default", "d1"), Value::Int(5), &stamp).unwrap();
        for (k, v) in [("default", "d1"), ("default", "d2"), ("alfa", "alfa1"), ("alfa", "alfa2")] {
            m.declare_dimension(k, Some(v));
        }
        let catalog: BTreeMap<String, BTreeSet<String>> = m.dimension_catalog();
        let st = CubeState { mode: Mode::Dimensions, x_key: "default".into(), z_key: "alfa".into(), ..Default::default() };
        // Places: plain, d1, d2 across; plain, alfa1, alfa2 deep. One cell selected: A1.
        let cube = build_dims(&m, s, (0, 0, 0, 0), &st, &catalog);
        assert_eq!(cube.x_labels, ["plain", "d1", "d2"]);
        assert_eq!(cube.z_labels, ["plain", "alfa1", "alfa2"]);
        assert_eq!(cube.heights[0], vec![Some(1.0), Some(10.0), Some(20.0)], "alfa plain: the plain value and each default value");
        assert_eq!(cube.heights[1], vec![Some(100.0), Some(1000.0), None], "alfa1: alone, with d1, and no cell with d2");
        assert_eq!(cube.heights[2], vec![Some(200.0), None, None]);
        // Two rows: the sum adds A1 and A2 where both exist.
        let cube = build_dims(&m, s, (0, 0, 1, 0), &st, &catalog);
        assert_eq!(cube.heights[0][1], Some(15.0), "10 + 5");
        let counted = build_dims(&m, s, (0, 0, 1, 0), &CubeState { agg: Agg::Count, ..st.clone() }, &catalog);
        assert_eq!(counted.heights[0][1], Some(2.0));
        let average = build_dims(&m, s, (0, 0, 1, 0), &CubeState { agg: Agg::Average, ..st.clone() }, &catalog);
        assert_eq!(average.heights[0][1], Some(7.5));
        // One dimension only: a single row of places.
        let one_d = build_dims(&m, s, (0, 0, 0, 0), &CubeState { z_key: String::new(), ..st }, &catalog);
        assert_eq!(one_d.z_labels, ["plain"]);
        assert_eq!(one_d.heights.len(), 1);
    }
}
