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

use std::collections::{HashMap, HashSet};

use alfadoc_adapter_deck::DeckEngine;
use alfadoc_adapter_grid::GridEngine;
use alfadoc_adapter_word::WordEngine;
use alfadoc_bridge::{MaixAccess, RegionAccess, TableChoice, sync_link};
use alfadoc_model::links::link_tag;
use alfadoc_model::{Anchor, Endpoint, Link, SyncMode, Target, Value};
use alfadoc_sync::{Debouncer, Policy};
use deckcraft_engine::model::ShapeKind;
use wordcraft_engine::doc::Document;

use crate::external::{self, ExternalFile, Kind, Lend, Spot};
use crate::data_view::column_name;
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
    /// An `.xlsx`, `.docx` or `.pptx` file on disk.
    File,
}

impl Place {
    const ALL: [Place; 5] = [Place::Sheet, Place::Document, Place::Slides, Place::Data, Place::File];

    fn part(self) -> &'static str {
        match self {
            Place::Sheet => package::SHEET_PART,
            Place::Document => package::DOC_PART,
            Place::Slides => package::DECK_PART,
            Place::Data => DATA_PART,
            Place::File => "",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Place::Sheet => "Sheet",
            Place::Document => "Document",
            Place::Slides => "Slides",
            Place::Data => "Data",
            Place::File => "File",
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
    /// The file path, used when `place` is a file.
    pub path: String,
    /// Which table (1, 2, …) of a `.docx` or `.pptx` file, used when `place` is a file.
    pub table: String,
    /// Which table of the open document or deck (1, 2, …), or 0 for the one that is selected.
    pub pick: usize,
    /// A document end that is a field in a sentence (the selected words) rather than a table.
    pub field: bool,
}

#[derive(Clone, Debug)]
pub struct LinkDraft {
    pub open: bool,
    pub a: Side,
    pub b: Side,
    pub mode: SyncMode,
    pub message: String,
    /// The link whose conflicts are being settled, by id.
    pub resolving: Option<String>,
}

impl Default for LinkDraft {
    fn default() -> Self {
        let side = |place| Side { place, sheet: String::new(), range: String::new(), path: String::new(), table: "1".into(), pick: 0, field: false };
        LinkDraft { open: false, a: side(Place::Sheet), b: side(Place::Slides), mode: SyncMode::Mirror, message: String::new(), resolving: None }
    }
}

/// One cell that both ends of a link changed, kept for the conflict dialog.
#[derive(Clone, Debug)]
pub struct Clash {
    pub row: u32,
    pub col: u16,
    pub base: String,
    pub a: String,
    pub b: String,
}

impl Clash {
    fn of(key: (u32, u16), base: Option<&Value>, a: Option<&Value>, b: Option<&Value>) -> Clash {
        let text = |v: Option<&Value>| v.map_or_else(|| "(empty)".to_string(), alfadoc_bridge::textcell::to_text);
        Clash { row: key.0, col: key.1, base: text(base), a: text(a), b: text(b) }
    }

    pub(crate) fn address(&self) -> String {
        format!("{}{}", crate::data_view::column_name(self.col), self.row + 1)
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
    pub conflicts: HashMap<String, Vec<Clash>>,
    /// Bumped whenever a sync pass ran or a link changed, so the Links tab knows to look again.
    pub epoch: u64,
    /// What happened to each link in this session, oldest first (not saved in the file).
    pub log: HashMap<String, Vec<LogLine>>,
    /// The last sync error per link id; gone after a sync that worked.
    pub errors: HashMap<String, String>,
    /// Links whose editors are not both open yet.
    pub idle: HashSet<String>,
}

/// One line of a link's activity list.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub at: u64,
    pub text: String,
}

/// Most lines kept per link.
const LOG_LIMIT: usize = 40;

impl Watch {
    /// Adds a line to a link's activity list.
    pub fn note(&mut self, id: &str, at: u64, text: impl Into<String>) {
        let text = text.into();
        let lines = self.log.entry(id.to_string()).or_default();
        if text.starts_with("Error") && lines.last().is_some_and(|l| l.text == text) {
            return;
        }
        lines.push(LogLine { at, text });
        if lines.len() > LOG_LIMIT {
            lines.remove(0);
        }
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Forgets what was kept about the links of a package that has been replaced.
    pub fn reset_links(&mut self) {
        self.conflicts.clear();
        self.log.clear();
        self.errors.clear();
        self.idle.clear();
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Forgets everything kept about a link that is gone.
    pub fn forget(&mut self, id: &str) {
        self.conflicts.remove(id);
        self.log.remove(id);
        self.errors.remove(id);
        self.idle.remove(id);
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Asks for a sync pass soon (after a package was opened, say).
    pub fn request_sync(&mut self, now: u64) {
        self.debounce.event(now);
    }
}

impl Default for Watch {
    fn default() -> Self {
        Watch { grid: None, deck: None, word: None, data: 0, debounce: Debouncer::new(QUIET_MS, MAX_WAIT_MS), last_poll: 0.0, conflicts: HashMap::new(), epoch: 0, log: HashMap::new(), errors: HashMap::new(), idle: HashSet::new() }
    }
}

enum Action {
    Create,
    FillFromSelection(bool),
    SyncNow(usize, Policy),
    Delete(usize),
    Resolve(usize, u32, u16, bool),
    ResolveAll(usize, bool),
    Review(Option<String>),
    MarkVariant,
    KeepVariants,
}

pub(crate) fn place_of_endpoint(e: &Endpoint) -> Option<Place> {
    match &e.target {
        Target::Internal { part } => Place::ALL.into_iter().find(|p| p.part() == part && *p != Place::File),
        Target::External { .. } => Some(Place::File),
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

pub(crate) fn mode_name(mode: SyncMode) -> &'static str {
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
            self.watch.epoch = self.watch.epoch.wrapping_add(1);
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
        let outside = self.package.links.iter().flat_map(|l| [&l.a, &l.b]).any(|e| match &e.target {
            Target::External { path, .. } => self.externals.get(path).is_some_and(|x| x.changed_on_disk(std::path::Path::new(path))),
            Target::Internal { .. } => false,
        });
        changed |= outside;
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
    pub(crate) fn sync_all(&mut self, now: u64, policy: Policy) -> (usize, usize, Vec<String>) {
        let (mut wrote, mut conflicts, mut errors) = (0, 0, vec![]);
        for pass in 0..MAX_PASSES {
            let (mut wrote_now, mut conflicts_now) = (0, 0);
            for i in 0..self.package.links.len() {
                if self.package.links.get(i).is_some_and(|l| l.paused) {
                    continue;
                }
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

    /// Runs `f` on link `index` with access to both of its ends. `Ok(None)` when one of the
    /// editors has not been opened yet. The link (with its new snapshot) is kept only when `f`
    /// succeeds; a changed linked file is written back either way.
    pub(crate) fn run_link<R>(&mut self, index: usize, now: u64, f: impl FnOnce(&mut Link, &mut dyn RegionAccess, &mut dyn RegionAccess) -> Result<R, alfadoc_bridge::BridgeError>) -> Result<Option<R>, String> {
        let Some(mut link) = self.package.links.get(index).cloned() else {
            return Ok(None);
        };
        let (Some(pa), Some(pb)) = (place_of_endpoint(&link.a), place_of_endpoint(&link.b)) else {
            return Ok(None);
        };
        if pa == pb {
            return Err("both ends of a link must be in different places".into());
        }
        let ext_path = [&link.a, &link.b].into_iter().find_map(|e| match &e.target {
            Target::External { path, .. } => Some(path.clone()),
            Target::Internal { .. } => None,
        });
        let mut ext = match &ext_path {
            Some(path) => Some(self.external_for(path)?),
            None => None,
        };
        let outcome: Result<Option<R>, String> = 'run: {
            let Shell { grid, word, deck, package, .. } = self;
            let mut slots: [Option<Box<dyn RegionAccess + '_>>; 5] = [
                grid.as_mut().map(|a| Box::new(GridEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                word.as_mut().map(|a| Box::new(WordEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                deck.as_mut().map(|a| Box::new(DeckEngine::borrowed(&mut a.session)) as Box<dyn RegionAccess + '_>),
                Some(Box::new(MaixAccess::new(&mut package.maix, DATA_AUTHOR, now)) as Box<dyn RegionAccess + '_>),
                ext.as_mut().map(|x| Box::new(Lend(x)) as Box<dyn RegionAccess + '_>),
            ];
            let slot = |p: Place| match p {
                Place::Sheet => 0,
                Place::Document => 1,
                Place::Slides => 2,
                Place::Data => 3,
                Place::File => 4,
            };
            let (Some(mut a), Some(mut b)) = (slots[slot(pa)].take(), slots[slot(pb)].take()) else {
                break 'run Ok(None);
            };
            f(&mut link, a.as_mut(), b.as_mut()).map(Some).map_err(|e| format!("{}: {e}", link.id))
        };
        // Write a changed file back, then keep its session for the next pass.
        if let (Some(path), Some(mut x)) = (ext_path, ext) {
            let saved = if x.is_dirty() { x.save(std::path::Path::new(&path)) } else { Ok(()) };
            self.externals.insert(path, x);
            saved?;
        }
        let Some(done) = outcome? else {
            return Ok(None);
        };
        if let Some(slot) = self.package.links.get_mut(index) {
            *slot = link;
        }
        Ok(Some(done))
    }

    /// Syncs link `index`; `Ok(None)` when one of its editors has not been opened yet.
    pub(crate) fn sync_one(&mut self, index: usize, policy: Policy, now: u64) -> Result<Option<(usize, usize)>, String> {
        let id = self.package.links.get(index).map(|l| l.id.clone()).unwrap_or_default();
        let outcome = match self.run_link(index, now, |link, a, b| sync_link(link, a, b, policy, now)) {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                self.watch.idle.insert(id);
                return Ok(None);
            }
            Err(e) => {
                self.watch.note(&id, now, format!("Error: {e}"));
                self.watch.errors.insert(id, e.clone());
                return Err(e);
            }
        };
        self.watch.idle.remove(&id);
        self.watch.errors.remove(&id);
        let clashes: Vec<Clash> = outcome.conflicts.iter().map(|c| Clash::of(c.key, c.base.as_ref(), c.a.as_ref(), c.b.as_ref())).collect();
        let count = clashes.len();
        if outcome.wrote_a + outcome.wrote_b > 0 || count > 0 {
            let mut parts = vec![];
            if outcome.wrote_a > 0 {
                parts.push(format!("{} cell(s) written to A", outcome.wrote_a));
            }
            if outcome.wrote_b > 0 {
                parts.push(format!("{} cell(s) written to B", outcome.wrote_b));
            }
            if count > 0 {
                parts.push(format!("{count} conflict(s)"));
            }
            self.watch.note(&id, now, parts.join(", "));
        }
        self.watch.conflicts.insert(id, clashes);
        Ok(Some((outcome.wrote_a + outcome.wrote_b, count)))
    }

    /// Settles one conflicting cell of link `index` with the chosen side's value.
    pub(crate) fn resolve_clash(&mut self, index: usize, row: u32, col: u16, keep_a: bool) -> Result<(), String> {
        let now = package::now_ms();
        self.run_link(index, now, |link, a, b| alfadoc_bridge::resolve_cell(link, a, b, (row, col), keep_a, now))?;
        if let Some(id) = self.package.links.get(index).map(|l| l.id.clone())
            && let Some(list) = self.watch.conflicts.get_mut(&id)
        {
            list.retain(|c| !(c.row == row && c.col == col));
        }
        Ok(())
    }

    /// The private session for a linked file: the cached one, or a fresh read when the file was
    /// changed outside Alfadoc (or has not been read yet).
    fn external_for(&mut self, path: &str) -> Result<ExternalFile, String> {
        if let Some(x) = self.externals.remove(path)
            && !x.changed_on_disk(std::path::Path::new(path))
        {
            return Ok(x);
        }
        ExternalFile::open(std::path::Path::new(path))
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
            Place::File => {
                let path = side.path.trim().to_string();
                let kind = external::kind_of(&path).ok_or("Choose an .xlsx, .docx or .pptx file.")?;
                let spot = if kind == Kind::Xlsx {
                    if side.sheet.trim().is_empty() || side.range.trim().is_empty() {
                        return Err("Enter the sheet name and the cell range in the file, for example Sheet1 and A1:C10.".into());
                    }
                    Spot::Cells { sheet: side.sheet.clone(), range: side.range.clone() }
                } else {
                    let n = side.table.trim().parse::<usize>().ok().filter(|n| *n >= 1).ok_or("Enter the table number in the file: 1 for the first table, 2 for the second…")?;
                    Spot::Table(n)
                };
                let mut x = self.external_for(&path)?;
                let anchor = x.anchor_for(id, &spot)?;
                x.mark_dirty();
                let saved = x.save(std::path::Path::new(&path));
                self.externals.insert(path.clone(), x);
                saved?;
                let sha256 = std::fs::read(&path).map(|b| alfadoc_model::container::sha256_hex(&b)).map_err(|e| e.to_string())?;
                return Ok(Endpoint { target: Target::External { path, sha256 }, anchor });
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
                let mut engine = WordEngine::borrowed(&mut app.session);
                if side.field {
                    engine.insert_field(&tag).map_err(|e| format!("Select the words in the document that should stay live first ({e})."))?;
                } else if side.pick > 0 {
                    engine.tag_table(side.pick, &tag).map_err(|e| e.to_string())?;
                } else {
                    engine.tag_selected_table(&tag).map_err(|_| "Put the caret inside a table in the document first, or pick a table from the list.".to_string())?;
                }
                Anchor::WordControl { tag }
            }
            Place::Slides => {
                let app = self.deck.as_mut().ok_or_else(|| missing(Place::Slides))?;
                if side.pick > 0 {
                    let slide_id = DeckEngine::borrowed(&mut app.session).name_nth_table(side.pick, &name).map_err(|e| e.to_string())?;
                    return Ok(Endpoint { target: Target::Internal { part: side.place.part().into() }, anchor: Anchor::DeckShape { slide_id, shape_name: name } });
                }
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
        self.package.links.push(Link::new(id.clone(), a, b, draft.mode, package::now_ms()));
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
        if place == Place::Data {
            let found = self.data.selection().and_then(|(id, r)| {
                let name = self.package.maix.sheet(id).ok()?.name.clone();
                Some((name, format!("{}{}:{}{}", column_name(r.1), r.0 + 1, column_name(r.3), r.2 + 1)))
            });
            let side = if side_b { &mut self.draft.b } else { &mut self.draft.a };
            match found {
                Some((sheet, range)) => {
                    side.sheet = sheet;
                    side.range = range;
                }
                None => self.draft.message = "Open the Data tab and select cells first.".into(),
            }
            return;
        }
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

    /// Lists the cells both ends changed, with both values, so each can be settled on its own.
    pub(crate) fn conflict_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.draft.resolving.clone() else {
            return;
        };
        let Some(index) = self.package.links.iter().position(|l| l.id == id) else {
            self.draft.resolving = None;
            return;
        };
        let clashes = self.watch.conflicts.get(&id).cloned().unwrap_or_default();
        let (a, b) = (describe(&self.package.links[index].a, &self.package.maix), describe(&self.package.links[index].b, &self.package.maix));
        let mut open = true;
        let mut actions: Vec<Action> = vec![];
        egui::Window::new("Resolve conflicts").open(&mut open).default_width(520.0).show(ctx, |ui| {
            if clashes.is_empty() {
                ui.label("Nothing left to settle.");
                return;
            }
            ui.label(format!("{} cell(s) were changed on both sides. Choose which value each cell keeps.", clashes.len()));
            ui.horizontal(|ui| {
                if ui.button("Keep all from A").clicked() {
                    actions.push(Action::ResolveAll(index, true));
                }
                if ui.button("Keep all from B").clicked() {
                    actions.push(Action::ResolveAll(index, false));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                egui::Grid::new(("clash_grid", id.as_str())).striped(true).num_columns(5).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.strong("Cell");
                    ui.strong(format!("A · {a}"));
                    ui.strong(format!("B · {b}"));
                    ui.strong("Was");
                    ui.strong("");
                    ui.end_row();
                    for c in &clashes {
                        ui.label(c.address());
                        ui.label(&c.a);
                        ui.label(&c.b);
                        ui.weak(&c.base);
                        ui.horizontal(|ui| {
                            if ui.small_button("Keep A").clicked() {
                                actions.push(Action::Resolve(index, c.row, c.col, true));
                            }
                            if ui.small_button("Keep B").clicked() {
                                actions.push(Action::Resolve(index, c.row, c.col, false));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        });
        if !open {
            self.draft.resolving = None;
        }
        for action in actions {
            match action {
                Action::Resolve(i, row, col, keep_a) => {
                    if let Err(e) = self.resolve_clash(i, row, col, keep_a) {
                        self.say(ctx, e);
                    }
                }
                Action::ResolveAll(i, keep_a) => {
                    let cells: Vec<(u32, u16)> = clashes.iter().map(|c| (c.row, c.col)).collect();
                    for (row, col) in cells {
                        if let Err(e) = self.resolve_clash(i, row, col, keep_a) {
                            self.say(ctx, e);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The ranges of the data store that links keep in step, for the Data tab to tint.
    pub(crate) fn linked_data_ranges(&self) -> Vec<(u16, (u32, u16, u32, u16))> {
        self.package
            .links
            .iter()
            .flat_map(|l| [&l.a, &l.b])
            .filter_map(|e| match &e.anchor {
                Anchor::Maix { sheet, top, left, bottom, right, .. } => Some((*sheet, (*top, *left, *bottom, *right))),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn link_window(&mut self, ctx: &egui::Context) {
        if !self.draft.open {
            return;
        }
        let mut open = true;
        let mut actions: Vec<Action> = vec![];
        let mut draft = self.draft.clone();
        let word_choices = self.word.as_mut().map(|a| WordEngine::borrowed(&mut a.session).table_choices()).unwrap_or_default();
        let deck_choices = self.deck.as_mut().and_then(|a| DeckEngine::borrowed(&mut a.session).table_choices().ok()).unwrap_or_default();
        let active_dims = alfadoc_model::dims::canonical(&self.package.maix.active_dims).replace('|', ", ");
        let data_sheets: Vec<String> = self.package.maix.sheets.iter().map(|s| s.name.clone()).collect();
        let links: Vec<(String, String, String, SyncMode, u64, usize)> = self
            .package
            .links
            .iter()
            .map(|l| {
                (l.id.clone(), describe(&l.a, &self.package.maix), describe(&l.b, &self.package.maix), l.mode, l.base.as_ref().map_or(0, |b| b.taken_at), self.watch.conflicts.get(&l.id).map_or(0, Vec::len))
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
                    Place::File => {
                        ui.horizontal(|ui| {
                            ui.label("File");
                            ui.add(egui::TextEdit::singleline(&mut side.path).desired_width(220.0));
                            #[cfg(not(target_arch = "wasm32"))]
                            if ui.button("Browse").clicked()
                                && let Some(p) = rfd::FileDialog::new().add_filter("Excel, Word or PowerPoint", &["xlsx", "docx", "pptx"]).pick_file()
                            {
                                side.path = p.to_string_lossy().into_owned();
                            }
                        });
                        if matches!(external::kind_of(&side.path), Some(Kind::Docx | Kind::Pptx)) {
                            ui.horizontal(|ui| {
                                ui.label("Table number");
                                ui.add(egui::TextEdit::singleline(&mut side.table).desired_width(50.0));
                                ui.weak("1 = the first table in the file");
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("Sheet");
                                ui.add(egui::TextEdit::singleline(&mut side.sheet).desired_width(110.0));
                                ui.label("Range");
                                ui.add(egui::TextEdit::singleline(&mut side.range).desired_width(90.0));
                            });
                        }
                    }
                    Place::Data => {
                        ui.horizontal(|ui| {
                            ui.label("Data sheet");
                            ui.add(egui::TextEdit::singleline(&mut side.sheet).desired_width(110.0));
                            if !data_sheets.is_empty() {
                                egui::ComboBox::from_id_salt(("data_sheet_pick", label)).selected_text("pick").show_ui(ui, |ui| {
                                    for name in &data_sheets {
                                        if ui.selectable_label(side.sheet == *name, name).clicked() {
                                            side.sheet = name.clone();
                                        }
                                    }
                                });
                            }
                            ui.label("Range");
                            ui.add(egui::TextEdit::singleline(&mut side.range).desired_width(90.0));
                            if ui.button("Use selection").on_hover_text("Take the cells selected in the Data tab").clicked() {
                                actions.push(Action::FillFromSelection(is_b));
                            }
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
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut side.field, false, "A table");
                            ui.radio_value(&mut side.field, true, "Words in a sentence");
                        });
                        if side.field {
                            ui.weak("Select the words in the document that should stay live, then Create link. Link them to a single cell.");
                        } else {
                            table_picker(ui, ("doc_table", label), &mut side.pick, &word_choices, "The table the caret is in");
                        }
                    }
                    Place::Slides => {
                        table_picker(ui, ("deck_table", label), &mut side.pick, &deck_choices, "The table selected on the slide");
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
            ui.collapsing("Variant blocks (audience, language…)", |ui| {
                if active_dims.is_empty() {
                    ui.weak("Choose dimensions first (Data tab ▸ Dimensions), for example audience = client.");
                } else {
                    ui.label(format!("Current: {}", active_dims));
                    ui.horizontal(|ui| {
                        if ui.button("Mark the selected words").on_hover_text("They will belong to the current dimensions").clicked() {
                            actions.push(Action::MarkVariant);
                        }
                        if ui.button("Keep only these blocks").on_hover_text("Delete the variant blocks that belong to other values (Undo brings them back)").clicked() {
                            actions.push(Action::KeepVariants);
                        }
                    });
                }
            });
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
                        if ui.small_button("Resolve…").clicked() {
                            actions.push(Action::Review(Some(id.clone())));
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
        draft.resolving = self.draft.resolving.take();
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
                Action::Review(id) => self.draft.resolving = id,
                Action::MarkVariant => {
                    let dims = self.package.maix.active_dims.clone();
                    self.draft.message = match self.word.as_mut() {
                        None => "Open the Document tab first.".to_string(),
                        Some(app) => match WordEngine::borrowed(&mut app.session).mark_variant(&dims) {
                            Ok(()) => "Marked. The selected words now belong to these dimensions.".to_string(),
                            Err(e) => format!("Select some words in the document first ({e})."),
                        },
                    };
                }
                Action::KeepVariants => {
                    let dims = self.package.maix.active_dims.clone();
                    self.draft.message = match self.word.as_mut() {
                        None => "Open the Document tab first.".to_string(),
                        Some(app) => match WordEngine::borrowed(&mut app.session).keep_variants(&dims) {
                            Ok(0) => "Nothing to remove: every block belongs to these dimensions.".to_string(),
                            Ok(n) => format!("Removed {n} block(s) for other dimensions. Undo in the Document brings them back."),
                            Err(e) => e.to_string(),
                        },
                    };
                }
                Action::Resolve(i, row, col, keep_a) => {
                    if let Err(e) = self.resolve_clash(i, row, col, keep_a) {
                        self.draft.message = e;
                    }
                }
                Action::ResolveAll(i, keep_a) => {
                    let id = self.package.links.get(i).map(|l| l.id.clone()).unwrap_or_default();
                    let cells: Vec<(u32, u16)> = self.watch.conflicts.get(&id).map(|v| v.iter().map(|c| (c.row, c.col)).collect()).unwrap_or_default();
                    for (row, col) in cells {
                        if let Err(e) = self.resolve_clash(i, row, col, keep_a) {
                            self.draft.message = e;
                            break;
                        }
                    }
                }
                Action::Delete(i) => {
                    if i < self.package.links.len() {
                        let gone = self.package.links.remove(i);
                        self.watch.forget(&gone.id);
                    }
                }
            }
        }
    }
}

/// A list of the tables of a document or deck; 0 means the one that is selected.
fn table_picker(ui: &mut egui::Ui, id: (&str, &str), pick: &mut usize, choices: &[TableChoice], selected_text: &str) {
    let current = if *pick == 0 { selected_text.to_string() } else { choices.iter().find(|c| c.nth == *pick).map_or_else(|| format!("Table {pick}"), TableChoice::label) };
    egui::ComboBox::from_id_salt(id).selected_text(current).width(320.0).show_ui(ui, |ui| {
        ui.selectable_value(pick, 0, selected_text);
        for c in choices {
            ui.selectable_value(pick, c.nth, c.label());
        }
    });
}

/// What one end of a link points at, in words: `Data: Costs!A1:C10`, `Sheet range`, `report.docx`.
pub(crate) fn describe(e: &Endpoint, maix: &alfadoc_model::Maix) -> String {
    match (&e.target, &e.anchor) {
        (Target::External { path, .. }, _) => std::path::Path::new(path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned()),
        (_, Anchor::Maix { sheet, top, left, bottom, right, .. }) => {
            let name = maix.sheet(*sheet).map_or_else(|_| "?".to_string(), |s| s.name.clone());
            format!("Data: {name}!{}", range_text((*top, *left, *bottom, *right)))
        }
        (_, Anchor::GridName { .. }) => "Sheet range".to_string(),
        (_, Anchor::WordControl { .. }) => "Document table or field".to_string(),
        (_, Anchor::DeckShape { .. }) => "Slide table".to_string(),
    }
}

/// `(0, 0, 9, 2)` as `A1:C10`.
pub(crate) fn range_text(r: (u32, u16, u32, u16)) -> String {
    let corner = |row: u32, col: u16| format!("{}{}", column_name(col), row + 1);
    if (r.0, r.1) == (r.2, r.3) { corner(r.0, r.1) } else { format!("{}:{}", corner(r.0, r.1), corner(r.2, r.3)) }
}

/// `HH:MM:SS` (UTC) of a millisecond timestamp.
pub(crate) fn clock(ms: u64) -> String {
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
        for place in Place::ALL.into_iter().filter(|p| *p != Place::File) {
            let e = Endpoint { target: Target::Internal { part: place.part().into() }, anchor: Anchor::GridName { name: "n".into() } };
            assert_eq!(place_of_endpoint(&e), Some(place));
        }
    }

    #[test]
    fn an_external_target_is_a_file_end() {
        let e = Endpoint { target: Target::External { path: "a.xlsx".into(), sha256: String::new() }, anchor: Anchor::GridName { name: "n".into() } };
        assert_eq!(place_of_endpoint(&e), Some(Place::File));
    }

    #[test]
    fn ranges_print_as_cell_names() {
        assert_eq!(range_text((0, 0, 9, 2)), "A1:C10");
        assert_eq!(range_text((6, 1, 6, 1)), "B7");
        assert_eq!(parse_range(&range_text((1, 26, 4, 27))), Some((1, 26, 4, 27)));
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
