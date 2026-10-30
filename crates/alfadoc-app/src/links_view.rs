//! The Links tab: every link of the package in one place, with its status, the details of both
//! ends, a live side-by-side comparison, the conflicts and the controls (rename, direction,
//! pause, sync once, make one end equal the other, swap, remove) and what happened to it so far.
//!
//! Creating a link still happens in the Links window (the **Link…** button), which can use the
//! editors' selections; this tab is for looking after the links that exist.

use std::collections::{BTreeMap, BTreeSet};

use alfadoc_bridge::textcell::to_text;
use alfadoc_model::{Anchor, Endpoint, Link, SyncMode, Target, Value};
use alfadoc_sync::Policy;
use egui::{Color32, RichText};

use crate::data_view::column_name;
use crate::linking::{Clash, LogLine, Place, clock, describe, mode_name, place_of_endpoint, range_text};
use crate::package;
use crate::shell::{Shell, Tab};

type Grid = BTreeMap<(u32, u16), Value>;

/// Most comparison rows drawn at once.
const ROW_LIMIT: usize = 300;
/// Longest cell text shown in the comparison.
const TEXT_LIMIT: usize = 60;

/// What the tab remembers between frames (nothing of it is saved in the file).
#[derive(Default)]
pub struct LinksTab {
    selected: Option<String>,
    filter: String,
    only_differences: bool,
    preview: Option<Preview>,
    confirm: Option<(String, Confirm)>,
    notice: String,
}

/// A step that must be confirmed with a second click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    Remove,
    /// Overwrite B with what A has.
    BFromA,
    /// Overwrite A with what B has.
    AFromB,
}

/// How one cell stands between the two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellState {
    Same,
    AChanged,
    BChanged,
    Conflict,
    AOverwrites,
    BOverwrites,
}

impl CellState {
    fn text(self) -> &'static str {
        match self {
            CellState::Same => "In step",
            CellState::AChanged => "A changed → goes to B",
            CellState::BChanged => "B changed → goes to A",
            CellState::Conflict => "Conflict",
            CellState::AOverwrites => "A → B",
            CellState::BOverwrites => "B → A",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    key: (u32, u16),
    a: String,
    b: String,
    base: String,
    state: CellState,
}

impl Row {
    fn address(&self) -> String {
        format!("{}{}", column_name(self.key.1), self.key.0 + 1)
    }
}

/// What reading one end of the link gave.
#[derive(Clone, Debug, Default)]
struct EndStat {
    error: Option<String>,
    filled: usize,
    rows: u32,
    cols: u16,
}

impl EndStat {
    fn of(result: &Result<Grid, String>) -> EndStat {
        match result {
            Ok(g) => EndStat { error: None, filled: g.len(), rows: g.keys().map(|k| k.0 + 1).max().unwrap_or(0), cols: g.keys().map(|k| k.1 + 1).max().unwrap_or(0) },
            Err(e) => EndStat { error: Some(e.clone()), ..EndStat::default() },
        }
    }

    fn size_text(&self) -> String {
        if self.filled == 0 { "No cells with content".to_string() } else { format!("{} × {} · {} cell(s) with content", self.rows, self.cols, self.filled) }
    }
}

/// Both ends of a link as they were when last looked at.
struct Preview {
    id: String,
    epoch: u64,
    a: EndStat,
    b: EndStat,
    rows: Vec<Row>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Good,
    Note,
    Warn,
    Bad,
}

struct Status {
    level: Level,
    text: String,
}

/// One entry of the list on the left.
struct Item {
    id: String,
    title: String,
    subtitle: String,
    status: Status,
}

/// Everything the right-hand side shows about the selected link.
struct Detail {
    link: Link,
    status: Status,
    a_desc: String,
    b_desc: String,
    a_info: Vec<(&'static str, String)>,
    b_info: Vec<(&'static str, String)>,
    a_tab: Option<Tab>,
    b_tab: Option<Tab>,
    conflicts: Vec<Clash>,
    log: Vec<LogLine>,
}

/// What a click asked for, carried out after the frame is drawn.
enum Do {
    NewLink,
    SyncAll,
    Show(Tab),
    Refresh,
    Select(String),
    Rename(String, String),
    Mode(String, SyncMode),
    Pause(String, bool),
    SyncOnce(String),
    Swap(String),
    Overwrite(String, bool),
    Remove(String),
    Resolve(String, u32, u16, bool),
    ResolveAll(String, bool),
}

// ---------------------------------------------------------------- pure helpers

fn show(v: Option<&Value>) -> String {
    v.map_or_else(|| "(empty)".to_string(), to_text)
}

fn state_of(mode: SyncMode, a: Option<&Value>, b: Option<&Value>, base: Option<&Value>) -> CellState {
    if a == b {
        return CellState::Same;
    }
    match mode {
        SyncMode::AToB => CellState::AOverwrites,
        SyncMode::BToA => CellState::BOverwrites,
        SyncMode::Mirror => {
            if a == base {
                CellState::BChanged
            } else if b == base {
                CellState::AChanged
            } else {
                CellState::Conflict
            }
        }
    }
}

/// Lines up the two ends and the last agreed contents, one row per cell that has content on
/// either end.
fn compare(mode: SyncMode, base: &Grid, a: &Grid, b: &Grid) -> Vec<Row> {
    let keys: BTreeSet<(u32, u16)> = a.keys().chain(b.keys()).chain(base.keys()).copied().collect();
    keys.into_iter()
        .filter_map(|key| {
            let (va, vb, vbase) = (a.get(&key), b.get(&key), base.get(&key));
            if va.is_none() && vb.is_none() {
                return None;
            }
            Some(Row { key, a: show(va), b: show(vb), base: show(vbase), state: state_of(mode, va, vb, vbase) })
        })
        .collect()
}

fn short(text: &str) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() > TEXT_LIMIT { format!("{}…", flat.chars().take(TEXT_LIMIT - 1).collect::<String>()) } else { flat }
}

/// A moment as `2026-03-01T13:45:09Z`.
fn stamp(ms: u64) -> String {
    to_text(&Value::Date(i64::try_from(ms).unwrap_or(0)))
}

fn tab_of(place: Option<Place>) -> Option<Tab> {
    match place? {
        Place::Sheet => Some(Tab::Sheet),
        Place::Document => Some(Tab::Document),
        Place::Slides => Some(Tab::Slides),
        Place::Data => Some(Tab::Data),
        Place::File => None,
    }
}

/// The facts about one end, as label/value pairs.
fn endpoint_info(e: &Endpoint, maix: &alfadoc_model::Maix) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = vec![];
    match &e.target {
        Target::Internal { part } => out.push(("Part", part.clone())),
        Target::External { path, sha256 } => {
            out.push(("File", path.clone()));
            if !sha256.is_empty() {
                out.push(("Hash at last sync", sha256.chars().take(12).collect()));
            }
        }
    }
    match &e.anchor {
        Anchor::Maix { sheet, top, left, bottom, right, dims } => {
            out.push(("Data sheet", maix.sheet(*sheet).map_or_else(|_| "?".to_string(), |s| s.name.clone())));
            out.push(("Range", range_text((*top, *left, *bottom, *right))));
            if !dims.is_empty() {
                out.push(("Dimensions", alfadoc_model::dims::canonical(dims).replace('|', ", ")));
            }
        }
        Anchor::WordControl { tag } => out.push(("Content control tag", tag.clone())),
        Anchor::GridName { name } => out.push(("Defined name", name.clone())),
        Anchor::DeckShape { slide_id, shape_name } => {
            out.push(("Slide", slide_id.clone()));
            out.push(("Shape", shape_name.clone()));
        }
    }
    out
}

fn mode_help(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Mirror => "A change on either side is copied to the other. A cell changed on both sides is a conflict you settle.",
        SyncMode::AToB => "A is the source. Anything changed in B is overwritten with what A has.",
        SyncMode::BToA => "B is the source. Anything changed in A is overwritten with what B has.",
    }
}

fn level_color(ui: &egui::Ui, level: Level) -> Color32 {
    match level {
        Level::Good => Color32::from_rgb(60, 160, 90),
        Level::Note => ui.visuals().weak_text_color(),
        Level::Warn => ui.visuals().warn_fg_color,
        Level::Bad => ui.visuals().error_fg_color,
    }
}

// ---------------------------------------------------------------- gathering

impl Shell {
    fn link_status(&self, link: &Link) -> Status {
        let conflicts = self.watch.conflicts.get(&link.id).map_or(0, Vec::len);
        let (level, text) = if link.paused {
            (Level::Note, "Paused: not syncing by itself".to_string())
        } else if let Some(e) = self.watch.errors.get(&link.id) {
            (Level::Bad, format!("Problem: {e}"))
        } else if self.watch.idle.contains(&link.id) {
            (Level::Note, "Waiting for its editor to be opened".to_string())
        } else if conflicts > 0 {
            (Level::Warn, format!("{conflicts} conflict(s) to settle"))
        } else if let Some(base) = &link.base {
            (Level::Good, format!("In step · synced {}", clock(base.taken_at)))
        } else {
            (Level::Note, "Never synced".to_string())
        };
        Status { level, text }
    }

    fn link_title(&self, link: &Link) -> String {
        if link.name.trim().is_empty() { format!("{} ↔ {}", describe(&link.a, &self.package.maix), describe(&link.b, &self.package.maix)) } else { link.name.trim().to_string() }
    }

    fn link_items(&self, filter: &str) -> Vec<Item> {
        let needle = filter.trim().to_lowercase();
        self.package
            .links
            .iter()
            .filter_map(|l| {
                let ends = format!("{} ↔ {}", describe(&l.a, &self.package.maix), describe(&l.b, &self.package.maix));
                let title = self.link_title(l);
                if !needle.is_empty() && ![&title, &ends, &l.id].iter().any(|t| t.to_lowercase().contains(&needle)) {
                    return None;
                }
                let subtitle = if l.name.trim().is_empty() { mode_name(l.mode).to_string() } else { format!("{ends} · {}", mode_name(l.mode)) };
                Some(Item { id: l.id.clone(), title, subtitle, status: self.link_status(l) })
            })
            .collect()
    }

    fn link_detail(&self, id: &str) -> Option<Detail> {
        let link = self.package.links.iter().find(|l| l.id == id)?.clone();
        let maix = &self.package.maix;
        Some(Detail {
            status: self.link_status(&link),
            a_desc: describe(&link.a, maix),
            b_desc: describe(&link.b, maix),
            a_info: endpoint_info(&link.a, maix),
            b_info: endpoint_info(&link.b, maix),
            a_tab: tab_of(place_of_endpoint(&link.a)),
            b_tab: tab_of(place_of_endpoint(&link.b)),
            conflicts: self.watch.conflicts.get(id).cloned().unwrap_or_default(),
            log: self.watch.log.get(id).cloned().unwrap_or_default(),
            link,
        })
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.package.links.iter().position(|l| l.id == id)
    }

    /// Reads both ends of link `id` and lines them up against the last agreed contents.
    fn read_preview(&mut self, id: &str) -> Option<Preview> {
        let index = self.index_of(id)?;
        let epoch = self.watch.epoch;
        let (mode, base) = {
            let link = self.package.links.get(index)?;
            (link.mode, link.base.as_ref().map(alfadoc_model::Snapshot::to_map).unwrap_or_default())
        };
        let read = self.run_link(index, package::now_ms(), |link, a, b| {
            let ra = a.read_region(&link.a.anchor).map(|r| r.cells).map_err(|e| e.to_string());
            let rb = b.read_region(&link.b.anchor).map(|r| r.cells).map_err(|e| e.to_string());
            Ok((ra, rb))
        });
        let (ra, rb) = match read {
            Ok(Some(pair)) => pair,
            Ok(None) => (Err("Open its editor first".to_string()), Err("Open its editor first".to_string())),
            Err(e) => (Err(e.clone()), Err(e)),
        };
        let rows = match (&ra, &rb) {
            (Ok(a), Ok(b)) => compare(mode, &base, a, b),
            _ => vec![],
        };
        Some(Preview { id: id.to_string(), epoch, a: EndStat::of(&ra), b: EndStat::of(&rb), rows })
    }

    // ------------------------------------------------------------ the tab

    pub(crate) fn links_tab(&mut self, ui: &mut egui::Ui) {
        let mut view = std::mem::take(&mut self.links_view);
        let items = self.link_items(&view.filter);
        if view.selected.as_ref().is_none_or(|s| self.index_of(s).is_none()) {
            view.selected = items.first().map(|i| i.id.clone());
        }
        let selected = view.selected.clone();
        let stale = view.preview.as_ref().is_none_or(|p| selected.as_deref() != Some(p.id.as_str()) || p.epoch != self.watch.epoch);
        if stale {
            view.preview = selected.as_deref().and_then(|id| self.read_preview(id));
        }
        let detail = selected.as_deref().and_then(|id| self.link_detail(id));
        let mut todo: Vec<Do> = vec![];
        self.header(ui, &mut view, &mut todo);
        ui.separator();
        egui::Panel::left("links_list").exact_size(300.0).frame(egui::Frame::NONE).show(ui, |ui| list_ui(ui, &view, &items, &mut todo));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| match &detail {
            Some(d) => detail_ui(ui, &mut view, d, &mut todo),
            None => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.weak(if self.package.links.is_empty() { "No links yet. Use Link… to join a table or range in one editor to another." } else { "No link matches the filter." });
                });
            }
        });
        self.links_view = view;
        self.apply(ui.ctx(), todo);
    }

    fn header(&self, ui: &mut egui::Ui, view: &mut LinksTab, todo: &mut Vec<Do>) {
        let links = &self.package.links;
        let paused = links.iter().filter(|l| l.paused).count();
        let conflicts: usize = links.iter().map(|l| self.watch.conflicts.get(&l.id).map_or(0, Vec::len)).sum();
        let problems = links.iter().filter(|l| self.watch.errors.contains_key(&l.id)).count();
        ui.horizontal(|ui| {
            ui.heading("Links");
            ui.weak(format!("{} link(s) · {} paused · {} conflict(s) · {} problem(s)", links.len(), paused, conflicts, problems));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Link…").on_hover_text("Make a new link").clicked() {
                    todo.push(Do::NewLink);
                }
                if ui.button("Sync all now").on_hover_text("Sync every link that is not paused").clicked() {
                    todo.push(Do::SyncAll);
                }
                ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Filter").desired_width(160.0));
            });
        });
        if !view.notice.is_empty() {
            ui.label(RichText::new(view.notice.clone()).italics());
        }
    }

    // ------------------------------------------------------------ doing

    fn apply(&mut self, ctx: &egui::Context, todo: Vec<Do>) {
        if todo.is_empty() {
            return;
        }
        ctx.request_repaint();
        let now = package::now_ms();
        let mut view = std::mem::take(&mut self.links_view);
        for step in todo {
            match step {
                Do::Select(id) => {
                    view.selected = Some(id);
                    view.confirm = None;
                }
                Do::NewLink => self.draft.open = true,
                Do::Show(tab) => self.pending = Some(tab),
                Do::Refresh => view.preview = None,
                Do::SyncAll => {
                    let (wrote, conflicts, errors) = self.sync_all(now, Policy::Manual);
                    self.watch.epoch = self.watch.epoch.wrapping_add(1);
                    view.notice = match errors.first() {
                        Some(e) => format!("Problem: {e}"),
                        None => format!("Synced: {wrote} cell(s) written, {conflicts} conflict(s)."),
                    };
                }
                other => self.apply_to_link(ctx, &mut view, other, now),
            }
        }
        self.links_view = view;
    }

    /// The steps that act on one link.
    fn apply_to_link(&mut self, ctx: &egui::Context, view: &mut LinksTab, step: Do, now: u64) {
        match step {
            Do::Rename(id, name) => {
                if let Some(i) = self.index_of(&id)
                    && let Some(l) = self.package.links.get_mut(i)
                {
                    l.name = name;
                }
            }
            Do::Mode(id, mode) => {
                if let Some(i) = self.index_of(&id)
                    && let Some(l) = self.package.links.get_mut(i)
                {
                    l.mode = mode;
                    self.watch.note(&id, now, format!("Direction set to {}", mode_name(mode)));
                    self.watch.request_sync(now);
                }
            }
            Do::Pause(id, paused) => {
                if let Some(i) = self.index_of(&id)
                    && let Some(l) = self.package.links.get_mut(i)
                {
                    l.paused = paused;
                    self.watch.note(&id, now, if paused { "Paused" } else { "Resumed" });
                    if !paused {
                        self.watch.request_sync(now);
                    }
                }
            }
            Do::SyncOnce(id) => {
                if let Some(i) = self.index_of(&id) {
                    view.notice = match self.sync_one(i, Policy::Manual, now) {
                        Ok(Some((w, c))) => format!("Synced: {w} cell(s) written, {c} conflict(s)."),
                        Ok(None) => "Open both editors first.".to_string(),
                        Err(e) => e,
                    };
                    self.watch.epoch = self.watch.epoch.wrapping_add(1);
                }
            }
            Do::Swap(id) => self.swap_ends(&id, now),
            Do::Overwrite(id, keep_a) => view.notice = self.overwrite(view.preview.as_ref(), &id, keep_a, now),
            Do::Remove(id) => {
                if let Some(i) = self.index_of(&id) {
                    self.package.links.remove(i);
                    self.watch.forget(&id);
                    view.selected = None;
                    view.preview = None;
                    view.notice = "Link removed. Its tag stays in the file but nothing uses it.".to_string();
                }
            }
            Do::Resolve(id, row, col, keep_a) => {
                if let Some(i) = self.index_of(&id)
                    && let Err(e) = self.resolve_clash(i, row, col, keep_a)
                {
                    self.say(ctx, e);
                }
                self.watch.epoch = self.watch.epoch.wrapping_add(1);
            }
            Do::ResolveAll(id, keep_a) => {
                let cells: Vec<(u32, u16)> = self.watch.conflicts.get(&id).map(|v| v.iter().map(|c| (c.row, c.col)).collect()).unwrap_or_default();
                if let Some(i) = self.index_of(&id) {
                    for (row, col) in cells {
                        if let Err(e) = self.resolve_clash(i, row, col, keep_a) {
                            self.say(ctx, e);
                            break;
                        }
                    }
                }
                self.watch.epoch = self.watch.epoch.wrapping_add(1);
            }
            Do::Select(_) | Do::NewLink | Do::Show(_) | Do::Refresh | Do::SyncAll => {}
        }
    }

    /// Swaps A and B. What was agreed stays valid, so nothing is copied.
    fn swap_ends(&mut self, id: &str, now: u64) {
        let Some(i) = self.index_of(id) else {
            return;
        };
        if let Some(l) = self.package.links.get_mut(i) {
            std::mem::swap(&mut l.a, &mut l.b);
            l.mode = match l.mode {
                SyncMode::Mirror => SyncMode::Mirror,
                SyncMode::AToB => SyncMode::BToA,
                SyncMode::BToA => SyncMode::AToB,
            };
        }
        self.watch.conflicts.insert(id.to_string(), vec![]);
        self.watch.note(id, now, "A and B swapped");
        self.watch.request_sync(now);
    }

    /// Makes one end equal the other: every cell that differs is copied from `keep_a`'s side.
    fn overwrite(&mut self, preview: Option<&Preview>, id: &str, keep_a: bool, now: u64) -> String {
        let Some(index) = self.index_of(id) else {
            return "That link is gone.".to_string();
        };
        let Some(p) = preview.filter(|p| p.id == id) else {
            return "Refresh the comparison first.".to_string();
        };
        let keys: Vec<(u32, u16)> = p.rows.iter().filter(|r| r.state != CellState::Same).map(|r| r.key).collect();
        let count = keys.len();
        let done = self.run_link(index, now, |link, a, b| {
            for key in &keys {
                alfadoc_bridge::resolve_cell(link, a, b, *key, keep_a, now)?;
            }
            Ok(())
        });
        match done {
            Ok(Some(())) => {
                self.watch.conflicts.insert(id.to_string(), vec![]);
                self.watch.errors.remove(id);
                let text = if keep_a { format!("B made equal to A ({count} cell(s))") } else { format!("A made equal to B ({count} cell(s))") };
                self.watch.note(id, now, text.clone());
                text
            }
            Ok(None) => "Open both editors first.".to_string(),
            Err(e) => e,
        }
    }
}

// ---------------------------------------------------------------- drawing

fn list_ui(ui: &mut egui::Ui, view: &LinksTab, items: &[Item], todo: &mut Vec<Do>) {
    egui::ScrollArea::vertical().id_salt("links_list_scroll").auto_shrink([false, false]).show(ui, |ui| {
        for item in items {
            let selected = view.selected.as_deref() == Some(item.id.as_str());
            if ui.selectable_label(selected, RichText::new(&item.title).strong()).clicked() {
                todo.push(Do::Select(item.id.clone()));
            }
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&item.subtitle).small().weak());
                    ui.label(RichText::new(format!("● {}", item.status.text)).small().color(level_color(ui, item.status.level)));
                });
            });
            ui.add_space(4.0);
        }
    });
}

fn detail_ui(ui: &mut egui::Ui, view: &mut LinksTab, d: &Detail, todo: &mut Vec<Do>) {
    egui::ScrollArea::vertical().id_salt("links_detail_scroll").auto_shrink([false, false]).show(ui, |ui| {
        title_block(ui, d, todo);
        ui.add_space(6.0);
        ends_block(ui, view, d, todo);
        ui.separator();
        controls_block(ui, view, d, todo);
        if !d.conflicts.is_empty() {
            ui.separator();
            conflicts_block(ui, d, todo);
        }
        ui.separator();
        comparison_block(ui, view, d);
        ui.separator();
        activity_block(ui, d);
    });
}

fn title_block(ui: &mut egui::Ui, d: &Detail, todo: &mut Vec<Do>) {
    ui.horizontal(|ui| {
        ui.label("Name");
        let mut name = d.link.name.clone();
        let hint = format!("{} ↔ {}", d.a_desc, d.b_desc);
        if ui.add(egui::TextEdit::singleline(&mut name).hint_text(hint).desired_width(320.0)).changed() {
            todo.push(Do::Rename(d.link.id.clone(), name));
        }
    });
    ui.label(RichText::new(format!("● {}", d.status.text)).color(level_color(ui, d.status.level)));
    let made = if d.link.created == 0 { "made before dates were kept".to_string() } else { format!("made {}", stamp(d.link.created)) };
    let synced = d.link.base.as_ref().map_or_else(|| "never synced".to_string(), |b| format!("last synced {}", stamp(b.taken_at)));
    ui.weak(format!("Id {} · {made} · {synced}", d.link.id));
}

fn ends_block(ui: &mut egui::Ui, view: &LinksTab, d: &Detail, todo: &mut Vec<Do>) {
    let (a_stat, b_stat) = view.preview.as_ref().map_or((None, None), |p| (Some(&p.a), Some(&p.b)));
    ui.columns(2, |cols| {
        if let [left, right] = cols {
            end_card(left, "A", &d.a_desc, &d.a_info, a_stat, d.a_tab, todo);
            end_card(right, "B", &d.b_desc, &d.b_info, b_stat, d.b_tab, todo);
        }
    });
}

fn end_card(ui: &mut egui::Ui, label: &str, desc: &str, info: &[(&'static str, String)], stat: Option<&EndStat>, tab: Option<Tab>, todo: &mut Vec<Do>) {
    ui.strong(format!("End {label} · {desc}"));
    egui::Grid::new(("link_end", label)).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
        for (key, value) in info {
            ui.weak(*key);
            ui.add(egui::Label::new(value.as_str()).wrap());
            ui.end_row();
        }
        if let Some(s) = stat {
            ui.weak("Contents");
            match &s.error {
                Some(e) => ui.colored_label(ui.visuals().error_fg_color, format!("Cannot read: {e}")),
                None => ui.label(s.size_text()),
            };
            ui.end_row();
        }
    });
    if let Some(tab) = tab
        && ui.small_button("Show in editor").clicked()
    {
        todo.push(Do::Show(tab));
    }
}

fn controls_block(ui: &mut egui::Ui, view: &mut LinksTab, d: &Detail, todo: &mut Vec<Do>) {
    let id = d.link.id.as_str();
    ui.horizontal(|ui| {
        ui.label("Direction");
        let mut mode = d.link.mode;
        egui::ComboBox::from_id_salt("links_tab_mode").selected_text(mode_name(mode)).show_ui(ui, |ui| {
            for m in [SyncMode::Mirror, SyncMode::AToB, SyncMode::BToA] {
                ui.selectable_value(&mut mode, m, mode_name(m));
            }
        });
        if mode != d.link.mode {
            todo.push(Do::Mode(id.to_string(), mode));
        }
        let mut paused = d.link.paused;
        if ui.checkbox(&mut paused, "Paused").on_hover_text("A paused link is not synced by itself. Sync once still works.").changed() {
            todo.push(Do::Pause(id.to_string(), paused));
        }
    });
    ui.weak(mode_help(d.link.mode));
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Sync once").on_hover_text("Bring the two ends together now, even when paused").clicked() {
            todo.push(Do::SyncOnce(id.to_string()));
        }
        if ui.button("Refresh").on_hover_text("Read both ends again").clicked() {
            todo.push(Do::Refresh);
        }
        if ui.button("Swap A ↔ B").on_hover_text("Exchange the two ends; the direction flips with them").clicked() {
            todo.push(Do::Swap(id.to_string()));
        }
    });
    ui.horizontal_wrapped(|ui| {
        if confirm_button(ui, view, id, Confirm::BFromA, "Make B equal A…", "Yes, overwrite B with A") {
            todo.push(Do::Overwrite(id.to_string(), true));
        }
        if confirm_button(ui, view, id, Confirm::AFromB, "Make A equal B…", "Yes, overwrite A with B") {
            todo.push(Do::Overwrite(id.to_string(), false));
        }
        if confirm_button(ui, view, id, Confirm::Remove, "Remove link…", "Yes, remove this link") {
            todo.push(Do::Remove(id.to_string()));
        }
    });
}

/// A button that asks for a second click before it counts. True on that second click.
fn confirm_button(ui: &mut egui::Ui, view: &mut LinksTab, id: &str, kind: Confirm, label: &str, sure: &str) -> bool {
    let armed = view.confirm.as_ref().is_some_and(|(i, k)| i == id && *k == kind);
    if !armed {
        if ui.button(label).clicked() {
            view.confirm = Some((id.to_string(), kind));
        }
        return false;
    }
    let mut done = false;
    let mut cancel = false;
    if ui.button(RichText::new(sure).color(ui.visuals().error_fg_color)).clicked() {
        done = true;
    }
    if ui.button("Cancel").clicked() {
        cancel = true;
    }
    if done || cancel {
        view.confirm = None;
    }
    done
}

fn conflicts_block(ui: &mut egui::Ui, d: &Detail, todo: &mut Vec<Do>) {
    let id = d.link.id.as_str();
    ui.horizontal(|ui| {
        ui.colored_label(ui.visuals().warn_fg_color, format!("{} cell(s) changed on both sides", d.conflicts.len()));
        if ui.small_button("Keep all from A").clicked() {
            todo.push(Do::ResolveAll(id.to_string(), true));
        }
        if ui.small_button("Keep all from B").clicked() {
            todo.push(Do::ResolveAll(id.to_string(), false));
        }
    });
    egui::Grid::new(("links_clashes", id)).striped(true).num_columns(5).spacing([14.0, 5.0]).show(ui, |ui| {
        ui.strong("Cell");
        ui.strong("A");
        ui.strong("B");
        ui.strong("Was");
        ui.strong("");
        ui.end_row();
        for c in &d.conflicts {
            ui.label(c.address());
            ui.label(short(&c.a));
            ui.label(short(&c.b));
            ui.weak(short(&c.base));
            ui.horizontal(|ui| {
                if ui.small_button("Keep A").clicked() {
                    todo.push(Do::Resolve(id.to_string(), c.row, c.col, true));
                }
                if ui.small_button("Keep B").clicked() {
                    todo.push(Do::Resolve(id.to_string(), c.row, c.col, false));
                }
            });
            ui.end_row();
        }
    });
}

fn comparison_block(ui: &mut egui::Ui, view: &mut LinksTab, d: &Detail) {
    let Some(p) = view.preview.as_ref().filter(|p| p.id == d.link.id) else {
        ui.weak("Reading both ends…");
        return;
    };
    let differing = p.rows.iter().filter(|r| r.state != CellState::Same).count();
    let total = p.rows.len();
    ui.horizontal(|ui| {
        ui.strong("Comparison");
        ui.weak(format!("{total} cell(s) · {differing} differ"));
        ui.checkbox(&mut view.only_differences, "Only differences");
    });
    let Some(p) = view.preview.as_ref() else {
        return;
    };
    if p.a.error.is_some() || p.b.error.is_some() {
        ui.colored_label(ui.visuals().error_fg_color, "One end could not be read, so the cells cannot be compared. See the details above.");
        return;
    }
    let rows: Vec<&Row> = p.rows.iter().filter(|r| !view.only_differences || r.state != CellState::Same).collect();
    if rows.is_empty() {
        ui.weak(if total == 0 { "Both ends are empty." } else { "Both ends are the same." });
        return;
    }
    ui.weak("Cells are named from the top-left corner of each linked region.");
    egui::Grid::new(("links_compare", d.link.id.as_str())).striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
        ui.strong("Cell");
        ui.strong("A");
        ui.strong("B");
        ui.strong("Last agreed");
        ui.strong("State");
        ui.end_row();
        for r in rows.iter().take(ROW_LIMIT) {
            ui.label(r.address());
            ui.label(short(&r.a));
            ui.label(short(&r.b));
            ui.weak(short(&r.base));
            let color = match r.state {
                CellState::Same => ui.visuals().weak_text_color(),
                CellState::Conflict => ui.visuals().warn_fg_color,
                CellState::AChanged | CellState::BChanged | CellState::AOverwrites | CellState::BOverwrites => ui.visuals().hyperlink_color,
            };
            ui.label(RichText::new(r.state.text()).color(color));
            ui.end_row();
        }
    });
    if rows.len() > ROW_LIMIT {
        ui.weak(format!("Showing the first {ROW_LIMIT} of {} cells.", rows.len()));
    }
}

fn activity_block(ui: &mut egui::Ui, d: &Detail) {
    ui.strong("Activity this session");
    if d.log.is_empty() {
        ui.weak("Nothing has happened to this link since Alfadoc was opened.");
        return;
    }
    for line in d.log.iter().rev() {
        ui.label(format!("{}  {}", clock(line.at), line.text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cells: &[((u32, u16), i64)]) -> Grid {
        cells.iter().map(|(k, v)| (*k, Value::Int(*v))).collect()
    }

    #[test]
    fn mirror_rows_say_which_side_changed() {
        let base = grid(&[((0, 0), 1), ((0, 1), 2), ((0, 2), 3), ((0, 3), 4)]);
        let a = grid(&[((0, 0), 1), ((0, 1), 20), ((0, 2), 3), ((0, 3), 40)]);
        let b = grid(&[((0, 0), 1), ((0, 1), 2), ((0, 2), 30), ((0, 3), 41)]);
        let rows = compare(SyncMode::Mirror, &base, &a, &b);
        let states: Vec<CellState> = rows.iter().map(|r| r.state).collect();
        assert_eq!(states, vec![CellState::Same, CellState::AChanged, CellState::BChanged, CellState::Conflict]);
        assert_eq!(rows[1].a, "20");
        assert_eq!(rows[1].base, "2");
    }

    #[test]
    fn one_way_rows_follow_the_source() {
        let a = grid(&[((0, 0), 1)]);
        let b = grid(&[((0, 0), 2)]);
        assert_eq!(compare(SyncMode::AToB, &Grid::new(), &a, &b)[0].state, CellState::AOverwrites);
        assert_eq!(compare(SyncMode::BToA, &Grid::new(), &a, &b)[0].state, CellState::BOverwrites);
    }

    #[test]
    fn a_cell_missing_on_one_side_shows_as_empty() {
        let a = grid(&[((1, 1), 5)]);
        let rows = compare(SyncMode::Mirror, &Grid::new(), &a, &Grid::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].b, "(empty)");
        assert_eq!(rows[0].state, CellState::AChanged);
        assert_eq!(rows[0].address(), "B2");
    }

    #[test]
    fn cells_empty_on_both_sides_are_left_out() {
        let base = grid(&[((0, 0), 1)]);
        assert!(compare(SyncMode::Mirror, &base, &Grid::new(), &Grid::new()).is_empty());
    }

    #[test]
    fn sizes_come_from_the_filled_cells() {
        let g = grid(&[((0, 0), 1), ((2, 1), 2)]);
        let s = EndStat::of(&Ok(g));
        assert_eq!((s.rows, s.cols, s.filled), (3, 2, 2));
        assert_eq!(EndStat::of(&Ok(Grid::new())).size_text(), "No cells with content");
        assert_eq!(EndStat::of(&Err("gone".into())).error.as_deref(), Some("gone"));
    }

    #[test]
    fn long_text_is_cut_for_the_table() {
        assert_eq!(short("a\nb"), "a b");
        let long = "x".repeat(100);
        assert_eq!(short(&long).chars().count(), TEXT_LIMIT);
        assert!(short(&long).ends_with('…'));
    }

    #[test]
    fn places_map_to_tabs() {
        assert_eq!(tab_of(Some(Place::Document)), Some(Tab::Document));
        assert_eq!(tab_of(Some(Place::File)), None);
        assert_eq!(tab_of(None), None);
    }

    #[test]
    fn an_end_lists_where_it_points() {
        let maix = alfadoc_model::Maix::default();
        let e = Endpoint { target: Target::Internal { part: "document/main".into() }, anchor: Anchor::WordControl { tag: "alfadoc:link=L1".into() } };
        let info = endpoint_info(&e, &maix);
        assert_eq!(info, vec![("Part", "document/main".to_string()), ("Content control tag", "alfadoc:link=L1".to_string())]);
        let f = Endpoint { target: Target::External { path: "a.xlsx".into(), sha256: "0123456789abcdef".into() }, anchor: Anchor::GridName { name: "N".into() } };
        let info = endpoint_info(&f, &maix);
        assert_eq!(info[1], ("Hash at last sync", "0123456789ab".to_string()));
    }

    #[test]
    fn stamps_are_readable_dates() {
        assert_eq!(stamp(0), "1970-01-01");
    }
}
