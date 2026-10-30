//! The Alfadoc window: a tab bar over one of the three engines' full editors.
//!
//! * Engines are created **lazily**, the first time their tab is opened, so start-up pays only for
//!   the tab you begin on (WordCraft's font database is the heavy one).
//! * Only the active engine runs `logic` and `ui`; the others keep their documents in memory.
//! * The engines share one egui context, so the shell swaps their fonts/colours/zoom on every tab
//!   change ([`ContextState`]).

use crate::ctxstate::ContextState;
use crate::hosts;
use crate::data_view::DataView;
use crate::links_view::LinksTab;
use crate::linking::{LinkDraft, Watch};
use crate::package;
use crate::protect::{Protect, Unlock};
use crate::recovery::{self, Recovery};
use alfadoc_model::Alfadoc;
use std::path::PathBuf;

use deckcraft_ui_egui::SlideApp;
use gridcraft_ui_egui::SheetApp;
use wordcraft_ui_egui::WordApp;

/// What a file picked in the browser is for.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Open,
    Import,
}

/// The file types Import takes.
const IMPORT_EXTS: &[&str] = &["xlsx", "xlsm", "xltx", "xls", "xlsb", "ods", "csv", "tsv", "docx", "docm", "dotx", "doc", "odt", "rtf", "pptx", "potx", "ppsx"];

/// A File action asked for in `ui` and carried out in the next `logic` pass (it needs the context
/// to switch tabs while loading).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileAction {
    Open,
    Save,
    SaveAs,
    Import,
    Export,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Sheet,
    Document,
    Slides,
    /// The package's shared data store (maix) as a grid.
    Data,
    /// The list of links with their details and controls.
    Links,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Sheet, Tab::Document, Tab::Slides, Tab::Data, Tab::Links];

    pub(crate) fn index(self) -> usize {
        match self {
            Tab::Sheet => 0,
            Tab::Document => 1,
            Tab::Slides => 2,
            Tab::Data => 3,
            Tab::Links => 4,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Sheet => "Sheet",
            Tab::Document => "Document",
            Tab::Slides => "Slides",
            Tab::Data => "Data",
            Tab::Links => "Links",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Sheet => "Spreadsheet (GridCraft)  Alt+1",
            Tab::Document => "Document (WordCraft)  Alt+2",
            Tab::Slides => "Presentation (DeckCraft)  Alt+3",
            Tab::Data => "Shared data store (maix)  Alt+4",
            Tab::Links => "All links: status, details and controls  Alt+5",
        }
    }

    fn key(self) -> egui::Key {
        match self {
            Tab::Sheet => egui::Key::Num1,
            Tab::Document => egui::Key::Num2,
            Tab::Slides => egui::Key::Num3,
            Tab::Data => egui::Key::Num4,
            Tab::Links => egui::Key::Num5,
        }
    }

    /// `--tab sheet|doc|slides` picks the tab to start on.
    /// The first argument that is not an option (or an option's value): a file to open.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn file_from_args(mut args: impl Iterator<Item = String>) -> Option<PathBuf> {
        while let Some(arg) = args.next() {
            if arg == "--tab" {
                args.next();
            } else if !arg.starts_with('-') {
                return Some(PathBuf::from(arg));
            }
        }
        None
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Tab {
        while let Some(arg) = args.next() {
            if arg == "--tab" {
                return match args.next().as_deref() {
                    Some("doc" | "document" | "word") => Tab::Document,
                    Some("slides" | "deck" | "slide") => Tab::Slides,
                    Some("data" | "maix") => Tab::Data,
                    Some("links" | "link") => Tab::Links,
                    _ => Tab::Sheet,
                };
            }
        }
        Tab::Sheet
    }
}

pub struct Shell {
    pub(crate) active: Tab,
    /// A tab change asked for during `ui`, applied at the start of the next `logic` pass.
    pub(crate) pending: Option<Tab>,
    pub(crate) grid: Option<SheetApp>,
    pub(crate) word: Option<WordApp>,
    pub(crate) deck: Option<SlideApp>,
    /// The look each engine left on the context when its tab was last active.
    saved: [Option<ContextState>; 5],
    /// Frames drawn so far: fonts exist on the context only after the first one.
    frames: u64,
    /// The `.alfadoc` package this window is editing (links, maix and untouched parts live here).
    pub(crate) package: Alfadoc,
    pub(crate) file: Option<PathBuf>,
    want: Option<FileAction>,
    /// A short message in the tab bar and the time (egui clock) it disappears.
    toast: Option<(String, f64)>,
    toast_text: Option<String>,
    /// A package to open as soon as the first frame has been drawn (`alfadoc file.alfadoc`).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    startup: Option<PathBuf>,
    /// Files the browser's pickers delivered, waiting for the next `logic` pass.
    #[cfg(target_arch = "wasm32")]
    web_inbox: std::rc::Rc<std::cell::RefCell<Vec<(Incoming, String, Vec<u8>)>>>,
    /// Linked `.xlsx` files on disk, opened in private sessions, by path.
    pub(crate) externals: std::collections::HashMap<String, crate::external::ExternalFile>,
    pub(crate) data: DataView,
    /// Keeps a copy of unsaved work so a crash loses nothing (`recovery.rs`).
    pub(crate) recovery: Recovery,
    /// The key of a passphrase-protected document: Save and recovery copies are encrypted with it.
    pub(crate) key: Option<alfadoc_model::security::Protector>,
    /// A protected file that is waiting for its passphrase.
    pub(crate) unlock: Option<Unlock>,
    pub(crate) protect: Protect,
    /// Copies left by a window that did not close normally, offered at start-up.
    recover_prompt: Vec<recovery::Found>,
    recover_checked: bool,
    /// Length of the data store's history when formulas were last worked out.
    calc_seen: usize,
    recovery_word: Option<wordcraft_engine::doc::Document>,
    recovery_word_edits: u64,
    /// Lets print jobs reach the spooler before the process ends.
    #[cfg(not(target_arch = "wasm32"))]
    _print_guard: crate::platform::print::ExitGuard,
    pub(crate) draft: LinkDraft,
    pub(crate) watch: Watch,
    pub(crate) links_view: LinksTab,
}

impl Shell {
    pub fn new(start: Tab, startup: Option<PathBuf>) -> Shell {
        Shell { active: start, pending: None, grid: None, word: None, deck: None, saved: [None, None, None, None, None], frames: 0, package: package::new_package(), file: None, want: None, toast: None, toast_text: None, startup, #[cfg(target_arch = "wasm32")] web_inbox: Default::default(), externals: std::collections::HashMap::new(), data: DataView::default(), recovery: Recovery::new(package::new_id()), key: None, unlock: None, protect: Protect::default(), recover_prompt: vec![], recover_checked: false, calc_seen: 0, recovery_word: None, recovery_word_edits: 0, #[cfg(not(target_arch = "wasm32"))] _print_guard: crate::platform::print::ExitGuard, draft: LinkDraft::default(), watch: Watch::default(), links_view: LinksTab::default() }
    }

    fn ensure(&mut self, ctx: &egui::Context, tab: Tab) {
        match tab {
            Tab::Sheet if self.grid.is_none() => self.grid = Some(hosts::grid_app(ctx)),
            Tab::Document if self.word.is_none() => self.word = Some(hosts::word_app()),
            Tab::Slides if self.deck.is_none() => self.deck = Some(hosts::deck_app(ctx)),
            _ => {}
        }
    }

    fn switch(&mut self, ctx: &egui::Context, next: Tab) {
        if next == self.active {
            return;
        }
        if self.frames > 0 {
            self.saved[self.active.index()] = Some(ContextState::capture(ctx));
        }
        self.active = next;
        self.ensure(ctx, next);
        if let Some(state) = &self.saved[next.index()] {
            state.restore(ctx);
        }
        ctx.request_repaint();
    }

    pub(crate) fn say(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        self.toast_text = Some(text.into());
        self.toast = Some((String::new(), now + 5.0));
        ctx.request_repaint_after(std::time::Duration::from_secs(5));
    }

    fn expire_toast(&mut self, ctx: &egui::Context) {
        if let Some((_, until)) = &self.toast
            && ctx.input(|i| i.time) >= *until
        {
            self.toast = None;
            self.toast_text = None;
        }
    }

    pub(crate) fn set_title(&self, ctx: &egui::Context) {
        let name = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} - Alfadoc")));
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn file_action(&mut self, ctx: &egui::Context, action: FileAction) {
        let result = match action {
            FileAction::Open => self.open_package(ctx, None),
            FileAction::Save => self.save_package(false),
            FileAction::SaveAs => self.save_package(true),
            FileAction::Import => self.import_file(ctx),
            FileAction::Export => self.export_file(),
        };
        self.report(ctx, result);
    }

    /// In the browser a file is picked (and delivered later, into `web_inbox`) or downloaded.
    #[cfg(target_arch = "wasm32")]
    fn file_action(&mut self, ctx: &egui::Context, action: FileAction) {
        let result = match action {
            FileAction::Open => {
                self.web_pick(ctx, "Alfadoc", &["alfadoc"], Incoming::Open);
                Ok(None)
            }
            FileAction::Import => {
                self.web_pick(ctx, "Office files", IMPORT_EXTS, Incoming::Import);
                Ok(None)
            }
            FileAction::Save | FileAction::SaveAs => {
                let name = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled.alfadoc".to_string(), |n| n.to_string_lossy().into_owned());
                self.package_bytes(&name).map(|bytes| {
                    web_download(name.clone(), bytes);
                    self.file = Some(PathBuf::from(&name));
                    Some(format!("Saving {name}"))
                })
            }
            FileAction::Export => self.export_bytes().map(|(bytes, ext)| {
                let stem = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
                let name = format!("{stem}.{ext}");
                web_download(name.clone(), bytes);
                Some(format!("Exporting {name}"))
            }),
        };
        self.report(ctx, result);
    }

    #[cfg(target_arch = "wasm32")]
    fn web_pick(&self, ctx: &egui::Context, label: &'static str, exts: &'static [&'static str], kind: Incoming) {
        let inbox = self.web_inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter(label, exts).pick_file().await else {
                return;
            };
            let bytes = file.read().await;
            inbox.borrow_mut().push((kind, file.file_name(), bytes));
            ctx.request_repaint();
        });
    }

    /// Files that arrived from the browser's pickers since the last pass.
    #[cfg(target_arch = "wasm32")]
    fn take_web_files(&mut self, ctx: &egui::Context) {
        let arrived = std::mem::take(&mut *self.web_inbox.borrow_mut());
        for (kind, name, bytes) in arrived {
            let result = match kind {
                Incoming::Open => self.load_package(ctx, &name, &bytes).map(|message| {
                    self.file = Some(PathBuf::from(&name));
                    message
                }),
                Incoming::Import => self.import_bytes(ctx, &name, &bytes),
            };
            self.report(ctx, result);
        }
    }

    fn report(&mut self, ctx: &egui::Context, result: Result<Option<String>, String>) {
        match result {
            Ok(Some(message)) => self.say(ctx, message),
            Ok(None) => {}
            Err(e) => self.say(ctx, format!("Failed: {e}")),
        }
        self.set_title(ctx);
    }

    /// Collects what the opened editors hold into the package and writes it.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_package(&mut self, ask: bool) -> Result<Option<String>, String> {
        let path = match (&self.file, ask) {
            (Some(p), false) => p.clone(),
            _ => {
                let start = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled.alfadoc".to_string(), |n| n.to_string_lossy().into_owned());
                let Some(mut chosen) = rfd::FileDialog::new().add_filter("Alfadoc", &["alfadoc"]).set_file_name(start).save_file() else {
                    return Ok(None);
                };
                if chosen.extension().is_none() {
                    chosen.set_extension("alfadoc");
                }
                chosen
            }
        };
        let file_name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let bytes = self.package_bytes(&file_name)?;
        // Write beside the target and rename, so a failure never leaves a half-written file.
        let tmp = path.with_extension("alfadoc.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.file = Some(path);
        self.recovery.saved();
        Ok(Some(format!("Saved {name}")))
    }

    /// Collects what the opened editors hold into the package and returns the `.alfadoc` bytes.
    pub(crate) fn package_bytes(&mut self, file_name: &str) -> Result<Vec<u8>, String> {
        if let Some(app) = self.grid.as_mut() {
            let docs = package::save_all_sheets(app)?;
            package::put_parts(&mut self.package.parts, package::SHEET_DIR, "xlsx", docs);
        }
        if let Some(app) = self.word.as_mut() {
            let docs = package::save_all_documents(app)?;
            package::put_parts(&mut self.package.parts, package::DOC_DIR, "docx", docs);
        }
        if let Some(app) = self.deck.as_mut() {
            let docs = package::save_all_decks(app)?;
            package::put_parts(&mut self.package.parts, package::DECK_DIR, "pptx", docs);
        }
        self.package.manifest.updated_at = package::now_ms();
        let stem = file_name.strip_suffix(".alfadoc").unwrap_or(file_name);
        if !stem.is_empty() {
            self.package.manifest.title = stem.to_string();
        }
        let bytes = self.package.to_bytes().map_err(|e| e.to_string())?;
        match &self.key {
            Some(key) => key.seal(&bytes).map_err(|e| e.to_string()),
            None => Ok(bytes),
        }
    }

    /// Brings a spreadsheet, document or presentation file into the editor that reads it.
    /// The file is not linked: it is copied in and becomes part of the package on Save.
    #[cfg(not(target_arch = "wasm32"))]
    fn import_file(&mut self, ctx: &egui::Context) -> Result<Option<String>, String> {
        let Some(path) = rfd::FileDialog::new().add_filter("Office files", IMPORT_EXTS).pick_file() else {
            return Ok(None);
        };
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        self.import_bytes(ctx, &name, &bytes)
    }

    fn import_bytes(&mut self, ctx: &egui::Context, name: &str, bytes: &[u8]) -> Result<Option<String>, String> {
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
        let tab = package::editor_for(ext).ok_or_else(|| format!("Alfadoc cannot import .{ext} files."))?;
        self.switch(ctx, tab);
        match tab {
            Tab::Sheet => self.grid.as_mut().map_or(Ok(()), |app| package::load_sheet(app, name, bytes))?,
            Tab::Document => self.word.as_mut().map_or(Ok(()), |app| package::load_document(app, name, bytes))?,
            Tab::Slides => self.deck.as_mut().map_or(Ok(()), |app| package::load_deck(app, name, bytes))?,
            Tab::Data | Tab::Links => {}
        }
        Ok(Some(format!("Imported {name}")))
    }

    /// The current editor's document as an Office file: its bytes and extension.
    fn export_bytes(&mut self) -> Result<(Vec<u8>, &'static str), String> {
        let ext = match self.active {
            Tab::Sheet => "xlsx",
            Tab::Document => "docx",
            Tab::Slides => "pptx",
            Tab::Data | Tab::Links => return Err("Export applies to the Sheet, Document and Slides tabs.".into()),
        };
        let bytes = match self.active {
            Tab::Sheet => self.grid.as_mut().map(package::save_sheet),
            Tab::Document => self.word.as_ref().map(package::save_document),
            Tab::Slides => self.deck.as_mut().map(package::save_deck),
            Tab::Data | Tab::Links => None,
        }
        .ok_or("Nothing to export in this editor yet.")??;
        Ok((bytes, ext))
    }

    /// Writes the current editor's document as an Office file.
    #[cfg(not(target_arch = "wasm32"))]
    fn export_file(&mut self) -> Result<Option<String>, String> {
        let (label, ext) = match self.active {
            Tab::Sheet => ("Excel workbook", "xlsx"),
            Tab::Document => ("Word document", "docx"),
            Tab::Slides => ("PowerPoint presentation", "pptx"),
            Tab::Data | Tab::Links => return Err("Export applies to the Sheet, Document and Slides tabs.".into()),
        };
        let stem = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
        let Some(mut path) = rfd::FileDialog::new().add_filter(label, &[ext]).set_file_name(format!("{stem}.{ext}")).save_file() else {
            return Ok(None);
        };
        if path.extension().is_none() {
            path.set_extension(ext);
        }
        let (bytes, _) = self.export_bytes()?;
        let tmp = path.with_extension(format!("{ext}.tmp"));
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(Some(format!("Exported {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()))))
    }

    /// Loads a package into the editors, tab by tab, so each one's look is saved and restored the
    /// way a normal tab change does it.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_package(&mut self, ctx: &egui::Context, path: Option<PathBuf>) -> Result<Option<String>, String> {
        let picked = path.or_else(|| rfd::FileDialog::new().add_filter("Alfadoc", &["alfadoc"]).pick_file());
        let Some(path) = picked else {
            return Ok(None);
        };
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if alfadoc_model::security::is_encrypted(&bytes) {
            self.unlock = Some(Unlock { name, path: Some(path), bytes, pass: String::new(), error: None, recovered: None });
            return Ok(None);
        }
        let message = self.load_package(ctx, &name, &bytes)?;
        self.file = Some(path);
        Ok(message)
    }

    /// Replaces the open package with the one in `bytes`.
    pub(crate) fn load_package(&mut self, ctx: &egui::Context, name: &str, bytes: &[u8]) -> Result<Option<String>, String> {
        let package = Alfadoc::from_bytes(bytes).map_err(|e| e.to_string())?;
        self.key = None;
        let mut first: Option<Tab> = None;
        for (tab, dir) in [(Tab::Sheet, package::SHEET_DIR), (Tab::Document, package::DOC_DIR), (Tab::Slides, package::DECK_DIR)] {
            let found = package::parts_for(&package.parts, dir);
            if found.is_empty() {
                continue;
            }
            self.switch(ctx, tab);
            for (part_name, data) in &found {
                let file_name = part_name.rsplit('/').next().unwrap_or(part_name);
                match tab {
                    Tab::Sheet => {
                        if let Some(app) = self.grid.as_mut() {
                            package::load_sheet(app, file_name, data)?;
                        }
                    }
                    Tab::Document => {
                        if let Some(app) = self.word.as_mut() {
                            package::load_document(app, file_name, data)?;
                        }
                    }
                    Tab::Slides => {
                        if let Some(app) = self.deck.as_mut() {
                            package::load_deck(app, file_name, data)?;
                        }
                    }
                    Tab::Data | Tab::Links => {}
                }
            }
            first.get_or_insert(tab);
        }
        if let Some(tab) = first {
            self.switch(ctx, tab);
        }
        self.package = package;
        self.watch.reset_links();
        self.links_view = LinksTab::default();
        self.externals.clear();
        self.watch.request_sync(package::now_ms());
        self.recovery.saved();
        Ok(Some(format!("Opened {name}")))
    }

    /// A number that changes with every edit in any editor, link or the data store.
    fn edit_signature(&mut self) -> u64 {
        fn mix(h: u64, x: u64) -> u64 {
            (h ^ x).wrapping_mul(0x0000_0100_0000_01b3)
        }
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        if let Some(rev) = self.grid.as_ref().and_then(|a| a.session.active().map(|d| d.revision)) {
            h = mix(h, rev as u64);
        }
        if let Some(rev) = self.deck.as_ref().and_then(|a| a.session.active().map(|d| d.revision)) {
            h = mix(h, rev as u64);
        }
        if let Some(app) = self.word.as_ref() {
            let same = self.recovery_word.as_ref().is_some_and(|old| old.same_content(&app.session.doc));
            if !same {
                self.recovery_word = Some(app.session.doc.clone());
                self.recovery_word_edits += 1;
            }
        }
        h = mix(h, self.recovery_word_edits);
        h = mix(h, self.package.maix.history.len() as u64);
        h = mix(h, self.data.revision());
        mix(h, self.package.links.len() as u64)
    }

    /// Writes the recovery copy now.
    fn recovery_write(&mut self, now: f64) {
        let title = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled.alfadoc".to_string(), |n| n.to_string_lossy().into_owned());
        match self.package_bytes(&title) {
            Ok(bytes) => {
                if let Err(e) = self.recovery.write(now, package::now_ms(), &bytes, self.file.as_deref(), &title) {
                    log::warn!("could not write a recovery copy: {e}");
                }
            }
            Err(e) => log::warn!("could not prepare a recovery copy: {e}"),
        }
    }

    /// Works out the data store's formulas again when something outside the Data tab (a link, an
    /// opened file) changed it.
    fn recalc_data(&mut self, ctx: &egui::Context) {
        let seen = self.package.maix.history.len();
        if seen == self.calc_seen {
            return;
        }
        let changed = self.package.maix.recalc(&alfadoc_model::Stamp::new("alfadoc-formulas", package::now_ms()));
        self.calc_seen = self.package.maix.history.len();
        if changed > 0 {
            ctx.request_repaint();
        }
    }

    /// Called every pass: offers leftover copies once, then keeps a copy of unsaved work.
    fn recovery_step(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if !self.recover_checked && self.frames > 2 {
            self.recover_checked = true;
            self.recover_prompt = recovery::found(package::now_ms());
            if !self.recover_prompt.is_empty() {
                ctx.request_repaint();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(recovery::CHECK_SECS as u64));
        if !self.recovery.due(now) {
            return;
        }
        self.recovery.beat(now, package::now_ms());
        let signature = self.edit_signature();
        if self.recovery.wants_copy(now, signature) {
            self.recovery_write(now);
        }
    }

    /// The "Recover unsaved work" window shown at start-up.
    fn recovery_window(&mut self, ctx: &egui::Context) {
        if self.recover_prompt.is_empty() {
            return;
        }
        let now_ms = package::now_ms();
        let (mut recover, mut discard, mut discard_all, mut later) = (None, None, false, false);
        egui::Window::new("Recover unsaved work").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Alfadoc did not close normally last time. These copies of unsaved work were kept:");
            ui.add_space(6.0);
            for (i, found) in self.recover_prompt.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.strong(&found.title);
                    ui.weak(format!("kept {}", recovery::ago(now_ms, found.saved_at)));
                    if ui.button("Recover").clicked() {
                        recover = Some(i);
                    }
                    if ui.button("Discard").clicked() {
                        discard = Some(i);
                    }
                });
            }
            ui.add_space(6.0);
            ui.weak("Recover replaces what is open now.");
            ui.horizontal(|ui| {
                if ui.button("Discard all").clicked() {
                    discard_all = true;
                }
                if ui.button("Decide later").clicked() {
                    later = true;
                }
            });
        });
        if discard_all {
            for found in self.recover_prompt.drain(..) {
                found.discard();
            }
        } else if later {
            self.recover_prompt.clear();
        } else if let Some(i) = discard {
            self.recover_prompt.remove(i).discard();
        } else if let Some(i) = recover {
            let found = self.recover_prompt.remove(i);
            let bytes = found.read().map_err(|e| e.to_string());
            if let Ok(b) = &bytes
                && alfadoc_model::security::is_encrypted(b)
            {
                let path = found.original.as_ref().map(PathBuf::from);
                self.unlock = Some(Unlock { name: found.title.clone(), path, bytes: b.clone(), pass: String::new(), error: None, recovered: Some(found) });
                return;
            }
            let result = bytes.and_then(|bytes| self.load_package(ctx, &found.title, &bytes));
            match result {
                Ok(_) => {
                    self.file = found.original.as_ref().map(PathBuf::from);
                    // The recovered work is still unsaved: keep a fresh copy before dropping the old one.
                    self.recovery.mark_unsaved();
                    self.recovery_write(ctx.input(|i| i.time));
                    found.discard();
                    self.say(ctx, "Recovered. Save to keep it.");
                    self.set_title(ctx);
                }
                Err(e) => self.say(ctx, format!("Could not recover: {e}")),
            }
        }
    }

    /// Draws the tab bar; returns the tab the user asked for, by click or Alt+1..3.
    fn tab_bar(&mut self, ui: &mut egui::Ui) -> Option<Tab> {
        let mut asked = ui.ctx().input_mut(|i| Tab::ALL.into_iter().find(|t| i.consume_key(egui::Modifiers::ALT, t.key())));
        if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::S)) {
            self.want = Some(FileAction::Save);
        }
        if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::O)) {
            self.want = Some(FileAction::Open);
        }
        egui::Panel::top("alfadoc_tabs").exact_size(34.0).frame(egui::Frame::NONE.fill(ui.visuals().extreme_bg_color).inner_margin(4)).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new("Alfadoc").strong());
                ui.separator();
                for tab in Tab::ALL {
                    let selected = self.active == tab;
                    if ui.selectable_label(selected, tab.label()).on_hover_text(tab.hint()).clicked() && !selected {
                        asked = Some(tab);
                    }
                }
                ui.separator();
                if ui.button("Link…").on_hover_text("Make a new link between tables and ranges in the editors (all links are in the Links tab)").clicked() {
                    self.draft.open = !self.draft.open;
                }
                if ui.button(if self.key.is_some() { "Protect 🔒" } else { "Protect" }).on_hover_text("Encrypt this document with a passphrase, or sign a copy").clicked() {
                    self.protect.open = !self.protect.open;
                }
                if ui.button("Open").on_hover_text("Open an .alfadoc file").clicked() {
                    self.want = Some(FileAction::Open);
                }
                if ui.button("Save").on_hover_text("Save as .alfadoc  Ctrl+Alt+S").clicked() {
                    self.want = Some(FileAction::Save);
                }
                if ui.button("Import").on_hover_text("Bring an Excel, Word or PowerPoint file into its editor").clicked() {
                    self.want = Some(FileAction::Import);
                }
                if ui.button("Export").on_hover_text("Save the current editor's document as .xlsx, .docx or .pptx").clicked() {
                    self.want = Some(FileAction::Export);
                }
                if ui.button("Save As").on_hover_text("Save as a new .alfadoc file").clicked() {
                    self.want = Some(FileAction::SaveAs);
                }
                if let Some(text) = &self.toast_text {
                    ui.separator();
                    ui.label(text);
                }
            });
        });
        asked
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(next) = self.pending.take() {
            self.switch(ctx, next);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.frames > 0
            && let Some(path) = self.startup.take()
        {
            let result = self.open_package(ctx, Some(path));
            self.report(ctx, result);
        }
        if let Some(action) = self.want.take() {
            self.file_action(ctx, action);
        }
        #[cfg(target_arch = "wasm32")]
        self.take_web_files(ctx);
        self.expire_toast(ctx);
        self.watch_links(ctx);
        self.recalc_data(ctx);
        self.recovery_step(ctx);
        let active = self.active;
        self.ensure(ctx, active);
        match active {
            Tab::Sheet => {
                if let Some(app) = self.grid.as_mut() {
                    app.logic(ctx);
                }
            }
            Tab::Document => {
                if let Some(app) = self.word.as_mut() {
                    app.logic(ctx);
                }
            }
            Tab::Slides => {
                if let Some(app) = self.deck.as_mut() {
                    app.logic(ctx);
                }
            }
            Tab::Data | Tab::Links => {}
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        match self.active {
            Tab::Sheet => {
                if let Some(app) = self.grid.as_mut() {
                    app.raw_input_hook(ctx, raw);
                }
            }
            Tab::Document => {
                if let Some(app) = self.word.as_mut() {
                    app.raw_input_hook(ctx, raw);
                }
            }
            Tab::Slides => {
                if let Some(app) = self.deck.as_mut() {
                    app.raw_input_hook(ctx, raw);
                }
            }
            Tab::Data | Tab::Links => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(tab) = self.tab_bar(ui)
            && tab != self.active
        {
            self.pending = Some(tab);
            ui.ctx().request_repaint();
        }
        // The Data tab has no engine behind it to paint its background, so it fills its own with the
        // current theme's colour (a frameless panel would show the window's black clear colour).
        let frame = if matches!(self.active, Tab::Data | Tab::Links) { egui::Frame::new().fill(ui.visuals().panel_fill) } else { egui::Frame::NONE };
        egui::CentralPanel::default().frame(frame).show(ui, |ui| match self.active {
            Tab::Sheet => {
                if let Some(app) = self.grid.as_mut() {
                    app.ui(ui);
                }
            }
            Tab::Document => {
                if let Some(app) = self.word.as_mut() {
                    app.ui(ui);
                }
            }
            Tab::Slides => {
                if let Some(app) = self.deck.as_mut() {
                    app.ui(ui);
                }
            }
            Tab::Data => {
                self.data.linked = self.linked_data_ranges();
                self.data.show(ui, &mut self.package.maix, package::now_ms());
                for (sheet, axis, op) in self.data.take_shifts() {
                    self.shift_link_anchors(sheet, axis, op);
                }
            }
            Tab::Links => self.links_tab(ui),
        });
        self.link_window(ui.ctx());
        self.conflict_window(ui.ctx());
        self.protect_window(ui.ctx());
        self.unlock_window(ui.ctx());
        self.recovery_window(ui.ctx());
        self.frames = self.frames.saturating_add(1);
    }
}

/// Hands a file to the browser as a download (the save dialog of the web is a download).
#[cfg(target_arch = "wasm32")]
fn web_download(name: String, bytes: Vec<u8>) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(handle) = rfd::AsyncFileDialog::new().set_file_name(&name).save_file().await
            && let Err(e) = handle.write(&bytes).await
        {
            log::error!("could not download {name}: {e}");
        }
    });
}
