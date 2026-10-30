//! Live links between the editors: creating them, keeping them in step, and the Links window.
//!
//! * **Create.** The Links window joins two regions: a sheet range (made into a defined name), the
//!   table the caret is in (Document) or the selected table on a slide (Slides). The region gets a
//!   tag the file keeps, the link goes into the package's registry and a first sync runs.
//! * **Keep in step.** Every frame the shell looks at each open editor's change counter (a
//!   revision for the sheet and the slides, a content comparison for the document). When something
//!   changed and then stayed quiet for a moment, every link is synced: both ends are read, merged
//!   against the last agreed contents, and only what differs is written through the editors' own
//!   commands (so Undo works on it).
//! * **Conflicts.** A cell changed on both sides in the same quiet period is left alone on both
//!   and counted; the Links window offers "Keep A" and "Keep B".

use std::collections::HashMap;

use alfadoc_adapter_deck::DeckEngine;
use alfadoc_adapter_grid::GridEngine;
use alfadoc_adapter_word::WordEngine;
use alfadoc_bridge::{MaixAccess, RegionAccess, sync_link};
use alfadoc_model::links::link_tag;
use alfadoc_model::{Anchor, Endpoint, Link, SyncMode, Target};
use alfadoc_sync::{Debouncer, Policy};
use deckcraft_engine::model::ShapeKind;
use wordcraft_engine::doc::Document;

use crate::package;
use crate::shell::Shell;

/// Quiet time after the last edit before links sync, and the longest a steady burst of edits can
/// postpone it.
const QUIET_MS: u64 = 600;
const MAX_WAIT_MS: u64 = 3_000;
/// Most repeats of the pass over all links when a change has to travel through several of them.
const MAX_PASSES: usize = 5;
/// How often the change counters are looked at.
const POLL_SECS: f64 = 0.15;

/// Where one end of a link lives: one of the three editors, or the shared data store (maix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Sheet,
    Document,
    Slides,
    Data,
}

impl Place {
    const ALL: [Place; 4] = [Place::Sheet, Place::Document, Place::Slides, Place::Data];

    fn part(self) -> &'static str {
        match self {
            Place::Sheet => package::SHEET_PART,
            Place::Document => package::DOC_PART,
            Place::Slides => package::DECK_PART,
            Place::Data => DATA_PART,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Place::Sheet => "Sheet",
            Place::Document => "Document",
            Place::Slides => "Slides",
            Place::Data => "Data",
        }
    }
}

/// The package part name a link uses for the shared data store.
const DATA_PART: &str = "data/maix";
/// Who the data store records as the author of changes made by links.
const DATA_AUTHOR: &str = "alfadoc-links";

/// What one end of a new link points at.
#[derive(Clone, Debug)]
pub struct Side {
    pub place: Place,
    /// Sheet name and range, used for the sheet and the data store.
    pub sheet: String,
    pub range: String,
}

#[derive(Clone, Debug)]
pub struct LinkDraft {
    pub open: bool,
    pub a: Side,
    pub b: Side,
    pub mode: SyncMode,
    pub message: String,
}

impl Default for LinkDraft {
    fn default() -> Self {
        let side = |place| Side { place, sheet: String::new(), range: String::new() };
        LinkDraft { open: false, a: side(Place::Sheet), b: side(Place::Slides), mode: SyncMode::Mirror, message: String::new() }
    }
}

/// Change tracking for the editors that links live in.
pub struct Watch {
    grid: Option<u64>,
    deck: Option<u64>,
    word: Option<Document>,
    data: usize,
    debounce: Debouncer,
    last_poll: f64,
    /// Unresolved conflicts per link id, from the last sync.
    pub conflicts: HashMap<String, usize>,
}

impl Default for Watch {
    fn default() -> Self {
        Watch { grid: None, deck: None, word: None, data: 0, debounce: Debouncer::new(QUIET_MS, MAX_WAIT_MS), last_poll: 0.0, conflicts: HashMap::new() }
    }
}

enum Action {
    Create,
    FillFromSelection(bool),
    SyncNow(usize, Policy),
    Delete(usize),
}

fn place_of_endpoint(e: &Endpoint) -> Option<Place> {
    match &e.target {
        Target::Internal { part } => Place::ALL.into_iter().find(|p| p.part() == part),
        Target::External { .. } => None,
    }
}

/// `A1:C10` (or `B7`) as zero-based (top, left, bottom, right).
fn parse_range(text: &str) -> Option<(u32, u16, u32, u16)> {
    fn cell(t: &str) -> Option<(u32, u16)> {
        let t = t.trim().replace('$', "");
        let letters: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
        let digits = t.get(letters.len()..)?;
        if letters.is_empty() || letters.len() > 3 || digits.is_empty() {
            return None;
        }
        let col = letters.to_ascii_uppercase().bytes().try_fold(0u32, |acc, b| acc.checked_mul(26)?.checked_add(u32::from(b - b'A') + 1))?;
        let row: u32 = digits.parse().ok()?;
        if row == 0 {
            return None;
        }
        Some((row - 1, u16::try_from(col - 1).ok()?))
    }
    let (a, b) = text.split_once(':').unwrap_or((text, text));
    let ((r0, c0), (r1, c1)) = (cell(a)?, cell(b)?);
    Some((r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1)))
}

fn mode_name(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Mirror => "Both ways",
        SyncMode::AToB => "A to B",
        SyncMode::BToA => "B to A",
    }
}

impl Shell {
    // ------------------------------------------------------------ keeping in step

    /// Called every frame from `logic`: notices edits and syncs once they go quiet.
    pub(crate) fn watch_links(&mut self, ctx: &egui::Context) {
        if self.package.links.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
        let clock = ctx.input(|i| i.time);
        if clock - self.watch.last_poll < POLL_SECS {
            return;
        }
        self.watch.last_poll = clock;
        let now = package::now_ms();
        if self.signals_changed() {
            self.watch.debounce.event(now);
        }
        if self.watch.debounce.take(now) {
            let (wrote, conflicts, errors) = self.sync_all(now, Policy::Manual);
            self.signals_changed(); // our own writes are not new edits
            if let Some(e) = errors.first() {
                self.say(ctx, format!("Link problem: {e}"));
            } else if conflicts > 0 {
                self.say(ctx, format!("{conflicts} linked cell(s) changed on both sides. Open Links to choose."));
            } else if wrote > 0 {
                self.say(ctx, format!("Updated {wrote} linked cell(s)"));
            }
        }
    }

    /// Looks at every editor's change counter; true when any moved since the last look.
    fn signals_changed(&mut self) -> bool {
        let grid = self.grid.as_ref().and_then(|a| a.session.active().map(|d| d.revision));
        let deck = self.deck.as_ref().and_then(|a| a.session.active().map(|d| d.revision));
        let data = self.package.maix.history.len();
        let mut changed = grid != self.watch.grid || deck != self.watch.deck || data != self.watch.data;
        self.watch.data = data;
        self.watch.grid = grid;
        self.watch.deck = deck;
        if let Some(app) = self.word.as_ref() {
            let same = self.watch.word.as_ref().is_some_and(|old| old.same_content(&app.session.doc));
            if !same {
                changed = self.watch.word.is_some() || changed;
                self.watch.word = Some(app.session.doc.clone());
            }
        }
        changed
    }

    /// Syncs every link whose two editors are open. Returns (cells written, conflicts, errors).
    ///
    /// Links chain (sheet ↔ data ↔ slide): a change that reaches the data store in one link is
    /// only passed on by the next link's turn, which may already have gone by. So the pass is
    /// repeated until nothing is written, at most `MAX_PASSES` times.
    fn sync_all(&mut self, now: u64, policy: Policy) -> (usize, usize, Vec<String>) {
        let (mut wrote, mut conflicts, mut errors) = (0, 0, vec![]);
        for pass in 0..MAX_PASSES {
            let (mut wrote_now, mut conflicts_now) = (0, 0);
            for i in 0..self.package.links.len() {
                match self.sync_one(i, policy, now) {
                    Ok(Some((w, c))) => {
                        wrote_now += w;
                        conflicts_now += c;
                    }
                    Ok(None) => {}
                    Err(e) if pass == 0 => errors.push(e),
                    Err(_) => {}
                }
            }
            wrote += wrote_now;
            conflicts = conflicts_now;
            if wrote_now == 0 {
                break;
            }
        }
        (wrote, conflicts, errors)
    }

    /// Syncs link `index`; `Ok(None)` when one of its editors has not been opened yet.
    fn sync_one(&mut self, index: usize, policy: Policy, now: u64) -> Result<Option<(usize, usize)>, String> {
        let Some(mut link) = self.package.links.get(index).cloned() else {
            return Ok(None);
        };
        let (Some(pa), Some(pb)) = (place_of_endpoint(&link.a), place_of_endpoint(&link.b)) else {
            return Ok(None);
        };
        if pa == pb {
            return Err("both ends of a link must be in different places".into());
        }
        let outcome = {
            let Shell { grid, word, deck, package, .. } = self;
            let mut slots: [Option<Box<dyn RegionAccess + '_>>; 4] = [
                grid.as_mut().map(|a| Box::new(GridEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                word.as_mut().map(|a| Box::new(WordEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                deck.as_mut().map(|a| Box::new(DeckEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                Some(Box::new(MaixAccess::new(&mut package.maix, DATA_AUTHOR, now)) as Box<dyn RegionAccess + '_>),
            ];
            let slot = |p: Place| match p {
                Place::Sheet => 0,
                Place::Document => 1,
                Place::Slides => 2,
                Place::Data => 3,
            };
            let (Some(mut a), Some(mut b)) = (slots[slot(pa)].take(), slots[slot(pb)].take()) else {
                return Ok(None);
            };
            sync_link(&mut link, a.as_mut(), b.as_mut(), policy, now).map_err(|e| format!("{}: {e}", link.id))?
        };
        self.watch.conflicts.insert(link.id.clone(), outcome.conflicts.len());
        if let Some(slot) = self.package.links.get_mut(index) {
            *slot = link;
        }
        Ok(Some((outcome.wrote_a + outcome.wrote_b, outcome.conflicts.len())))
    }

    // ------------------------------------------------------------ creating

    fn endpoint_for(&mut self, side: &Side, id: &str) -> Result<Endpoint, String> {
        let tag = link_tag(id);
        let name = format!("alfadoc_{id}");
        let missing = |place: Place| format!("Open the {} tab first.", place.name());
        let anchor = match side.place {
            Place::Data => {
                let (top, left, bottom, right) = parse_range(&side.range).ok_or("Enter the data range as cells, for example A1:C10.")?;
                if side.sheet.trim().is_empty() {
                    return Err("Enter a name for the data sheet.".into());
                }
                let maix = &mut self.package.maix;
                let sheet = match maix.sheet_by_name(side.sheet.trim()) {
                    Some(s) => s.id,
                    None => maix.add_sheet(side.sheet.trim()),
                };
                Anchor::Maix { sheet, top, left, bottom, right, dims: alfadoc_model::DimVector::new() }
            }
            Place::Sheet => {
                let app = self.grid.as_mut().ok_or_else(|| missing(Place::Sheet))?;
                if side.sheet.trim().is_empty() || side.range.trim().is_empty() {
                    return Err("Enter the sheet name and the cell range, for example Sheet1 and A1:C10.".into());
                }
                GridEngine::borrowed(&mut app.session).define_region(&name, side.sheet.trim(), side.range.trim()).map_err(|e| e.to_string())?;
                Anchor::GridName { name }
            }
            Place::Document => {
                let app = self.word.as_mut().ok_or_else(|| missing(Place::Document))?;
                WordEngine::borrowed(&mut app.session).tag_selected_table(&tag).map_err(|_| "Put the caret inside a table in the document first.".to_string())?;
                Anchor::WordControl { tag }
            }
            Place::Slides => {
                let app = self.deck.as_mut().ok_or_else(|| missing(Place::Slides))?;
                let (slide_index, shape, slide_id) = {
                    let st = app.session.doc().map_err(|e| e.to_string())?;
                    let shape = st.selection.shapes.first().copied().ok_or("Select a table on a slide first.")?;
                    let slide = st.doc.slides.get(st.selection.slide).ok_or("There is no current slide.")?;
                    if !matches!(slide.shape(shape).map(|s| &s.kind), Some(ShapeKind::Table(_))) {
                        return Err("The selected shape is not a table. Select a table on the slide.".into());
                    }
                    (st.selection.slide, shape, slide.id.to_string())
                };
                DeckEngine::borrowed(&mut app.session).name_table(slide_index, shape.0, &name).map_err(|e| e.to_string())?;
                Anchor::DeckShape { slide_id, shape_name: name }
            }
        };
        Ok(Endpoint { target: Target::Internal { part: side.place.part().into() }, anchor })
    }

    fn create_link(&mut self) -> Result<String, String> {
        let draft = self.draft.clone();
        if draft.a.place == draft.b.place {
            return Err("Choose two different places for the two ends.".into());
        }
        let full = package::new_id();
        let id: String = full.chars().take(8).collect();
        let a = self.endpoint_for(&draft.a, &id)?;
        let b = self.endpoint_for(&draft.b, &id)?;
        self.package.links.push(Link { id: id.clone(), a, b, mode: draft.mode, base: None });
        let index = self.package.links.len() - 1;
        // The first sync decides what the two ends start as: A's content wins.
        let first = self.sync_one(index, Policy::PreferA, package::now_ms());
        // Pass the new content on to any other link that shares one of its ends.
        let _ = self.sync_all(package::now_ms(), Policy::Manual);
        match first {
            Ok(Some((wrote, _))) => Ok(format!("Linked. {wrote} cell(s) copied.")),
            Ok(None) => Ok("Linked.".into()),
            Err(e) => Err(e),
        }
    }

    fn fill_from_selection(&mut self, side_b: bool) {
        let place = if side_b { self.draft.b.place } else { self.draft.a.place };
        if place != Place::Sheet {
            return;
        }
        let Some(app) = self.grid.as_ref() else {
            self.draft.message = "Open the Sheet tab first.".into();
            return;
        };
        let found = app.session.doc().ok().and_then(|st| {
            let sheet = st.wb.sheet(st.wb.active_sheet())?.name.clone();
            let range = st.selection.ranges.last()?.a1();
            Some((sheet, range))
        });
        let side = if side_b { &mut self.draft.b } else { &mut self.draft.a };
        if let Some((sheet, range)) = found {
            side.sheet = sheet;
            side.range = range;
        }
    }

    // ------------------------------------------------------------ the Links window

    pub(crate) fn link_window(&mut self, ctx: &egui::Context) {
        if !self.draft.open {
            return;
        }
        let mut open = true;
        let mut actions: Vec<Action> = vec![];
        let mut draft = self.draft.clone();
        let links: Vec<(String, String, String, SyncMode, u64, usize)> = self
            .package
            .links
            .iter()
            .map(|l| {
                let name = |e: &Endpoint| place_of_endpoint(e).map_or("?", Place::name).to_string();
                (l.id.clone(), name(&l.a), name(&l.b), l.mode, l.base.as_ref().map_or(0, |b| b.taken_at), self.watch.conflicts.get(&l.id).copied().unwrap_or(0))
            })
            .collect();
        egui::Window::new("Links").open(&mut open).default_width(460.0).show(ctx, |ui| {
            ui.label("A link keeps a table or range in one editor equal to a table or range in another.");
            ui.separator();
            for (label, is_b) in [("A", false), ("B", true)] {
                let side = if is_b { &mut draft.b } else { &mut draft.a };
                ui.horizontal(|ui| {
                    ui.label(format!("End {label}:"));
                    egui::ComboBox::from_id_salt(("link_side", label)).selected_text(side.place.name()).show_ui(ui, |ui| {
                        for place in Place::ALL {
                            ui.selectable_value(&mut side.place, place, place.name());
                        }
                    });
                });
                match side.place {
                    Place::Data => {
                        ui.horizontal(|ui| {
                            ui.label("Data sheet");
                            ui.add(egui::TextEdit::singleline(&mut side.sheet).desired_width(110.0));
                            ui.label("Range");
                            ui.add(egui::TextEdit::singleline(&mut side.range).desired_width(90.0));
                        });
                    }
                    Place::Sheet => {
                        ui.horizontal(|ui| {
                            ui.label("Sheet");
                            ui.add(egui::TextEdit::singleline(&mut side.sheet).desired_width(110.0));
                            ui.label("Range");
                            ui.add(egui::TextEdit::singleline(&mut side.range).desired_width(90.0));
                            if ui.button("Use selection").clicked() {
                                actions.push(Action::FillFromSelection(is_b));
                            }
                        });
                    }
                    Place::Document => {
                        ui.weak("Uses the table the caret is in.");
                    }
                    Place::Slides => {
                        ui.weak("Uses the table selected on the current slide.");
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label("Direction:");
                egui::ComboBox::from_id_salt("link_mode").selected_text(mode_name(draft.mode)).show_ui(ui, |ui| {
                    for mode in [SyncMode::Mirror, SyncMode::AToB, SyncMode::BToA] {
                        ui.selectable_value(&mut draft.mode, mode, mode_name(mode));
                    }
                });
                if ui.button("Create link").clicked() {
                    actions.push(Action::Create);
                }
            });
            if !draft.message.is_empty() {
                ui.label(&draft.message);
            }
            ui.separator();
            if links.is_empty() {
                ui.weak("No links yet.");
            }
            for (i, (id, a, b, mode, taken_at, conflicts)) in links.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{a} ↔ {b}  ({})", mode_name(*mode)));
                    ui.weak(if *taken_at == 0 { "never synced".to_string() } else { format!("synced {}", clock(*taken_at)) });
                    if *conflicts > 0 {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("{conflicts} conflict(s)"));
                        if ui.small_button("Keep A").clicked() {
                            actions.push(Action::SyncNow(i, Policy::PreferA));
                        }
                        if ui.small_button("Keep B").clicked() {
                            actions.push(Action::SyncNow(i, Policy::PreferB));
                        }
                    }
                    if ui.small_button("Sync now").on_hover_text(id.as_str()).clicked() {
                        actions.push(Action::SyncNow(i, Policy::Manual));
                    }
                    if ui.small_button("Remove").clicked() {
                        actions.push(Action::Delete(i));
                    }
                });
            }
        });
        self.draft = draft;
        self.draft.open = open;
        for action in actions {
            match action {
                Action::Create => {
                    self.draft.message = match self.create_link() {
                        Ok(m) => m,
                        Err(e) => e,
                    };
                }
                Action::FillFromSelection(is_b) => self.fill_from_selection(is_b),
                Action::SyncNow(i, policy) => {
                    let result = self.sync_one(i, policy, package::now_ms());
                    self.draft.message = match result {
                        Ok(Some((w, c))) => format!("Synced: {w} cell(s) written, {c} conflict(s)."),
                        Ok(None) => "Open both editors first.".into(),
                        Err(e) => e,
                    };
                }
                Action::Delete(i) => {
                    if i < self.package.links.len() {
                        let gone = self.package.links.remove(i);
                        self.watch.conflicts.remove(&gone.id);
                    }
                }
            }
        }
    }
}

/// `HH:MM:SS` (UTC) of a millisecond timestamp.
fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{:02}:{:02}:{:02} UTC", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_time_of_day() {
        assert_eq!(clock(0), "00:00:00 UTC");
        assert_eq!(clock((13 * 3600 + 5 * 60 + 9) * 1000), "13:05:09 UTC");
    }

    #[test]
    fn parts_map_to_places_and_back() {
        for place in Place::ALL {
            let e = Endpoint { target: Target::Internal { part: place.part().into() }, anchor: Anchor::GridName { name: "n".into() } };
            assert_eq!(place_of_endpoint(&e), Some(place));
        }
    }

    #[test]
    fn ranges_parse_to_zero_based_rectangles() {
        assert_eq!(parse_range("A1:C10"), Some((0, 0, 9, 2)));
        assert_eq!(parse_range("b7"), Some((6, 1, 6, 1)));
        assert_eq!(parse_range("$C$3:$A$1"), Some((0, 0, 2, 2)));
        assert_eq!(parse_range("AA2"), Some((1, 26, 1, 26)));
        assert_eq!(parse_range("A0"), None);
        assert_eq!(parse_range("7"), None);
        assert_eq!(parse_range("A"), None);
    }
}
