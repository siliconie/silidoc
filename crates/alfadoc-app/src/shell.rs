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
use crate::linking::{LinkDraft, Watch};
use crate::package;
use alfadoc_model::Alfadoc;
use std::path::PathBuf;

use deckcraft_ui_egui::SlideApp;
use gridcraft_ui_egui::SheetApp;
use wordcraft_ui_egui::WordApp;

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
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Sheet, Tab::Document, Tab::Slides, Tab::Data];

    pub(crate) fn index(self) -> usize {
        match self {
            Tab::Sheet => 0,
            Tab::Document => 1,
            Tab::Slides => 2,
            Tab::Data => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Sheet => "Sheet",
            Tab::Document => "Document",
            Tab::Slides => "Slides",
            Tab::Data => "Data",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Sheet => "Spreadsheet (GridCraft)  Alt+1",
            Tab::Document => "Document (WordCraft)  Alt+2",
            Tab::Slides => "Presentation (DeckCraft)  Alt+3",
            Tab::Data => "Shared data store (maix)  Alt+4",
        }
    }

    fn key(self) -> egui::Key {
        match self {
            Tab::Sheet => egui::Key::Num1,
            Tab::Document => egui::Key::Num2,
            Tab::Slides => egui::Key::Num3,
            Tab::Data => egui::Key::Num4,
        }
    }

    /// `--tab sheet|doc|slides` picks the tab to start on.
    /// The first argument that is not an option (or an option's value): a file to open.
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

    pub fn from_args(mut args: impl Iterator<Item = String>) -> Tab {
        while let Some(arg) = args.next() {
            if arg == "--tab" {
                return match args.next().as_deref() {
                    Some("doc" | "document" | "word") => Tab::Document,
                    Some("slides" | "deck" | "slide") => Tab::Slides,
                    Some("data" | "maix") => Tab::Data,
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
    pending: Option<Tab>,
    pub(crate) grid: Option<SheetApp>,
    pub(crate) word: Option<WordApp>,
    pub(crate) deck: Option<SlideApp>,
    /// The look each engine left on the context when its tab was last active.
    saved: [Option<ContextState>; 4],
    /// Frames drawn so far: fonts exist on the context only after the first one.
    frames: u64,
    /// The `.alfadoc` package this window is editing (links, maix and untouched parts live here).
    pub(crate) package: Alfadoc,
    file: Option<PathBuf>,
    want: Option<FileAction>,
    /// A short message in the tab bar and the time (egui clock) it disappears.
    toast: Option<(String, f64)>,
    toast_text: Option<String>,
    /// A package to open as soon as the first frame has been drawn (`alfadoc file.alfadoc`).
    startup: Option<PathBuf>,
    pub(crate) data: DataView,
    pub(crate) draft: LinkDraft,
    pub(crate) watch: Watch,
}

impl Shell {
    pub fn new(start: Tab, startup: Option<PathBuf>) -> Shell {
        Shell { active: start, pending: None, grid: None, word: None, deck: None, saved: [None, None, None, None], frames: 0, package: package::new_package(), file: None, want: None, toast: None, toast_text: None, startup, data: DataView::default(), draft: LinkDraft::default(), watch: Watch::default() }
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

    fn set_title(&self, ctx: &egui::Context) {
        let name = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} - Alfadoc")));
    }

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

    fn report(&mut self, ctx: &egui::Context, result: Result<Option<String>, String>) {
        match result {
            Ok(Some(message)) => self.say(ctx, message),
            Ok(None) => {}
            Err(e) => self.say(ctx, format!("Failed: {e}")),
        }
        self.set_title(ctx);
    }

    /// Collects what the opened editors hold into the package and writes it.
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
        if let Some(app) = self.grid.as_mut() {
            self.package.parts.insert(package::SHEET_PART.into(), package::save_sheet(app)?);
        }
        if let Some(app) = self.word.as_ref() {
            self.package.parts.insert(package::DOC_PART.into(), package::save_document(app)?);
        }
        if let Some(app) = self.deck.as_mut() {
            self.package.parts.insert(package::DECK_PART.into(), package::save_deck(app)?);
        }
        self.package.manifest.updated_at = package::now_ms();
        if let Some(stem) = path.file_stem() {
            self.package.manifest.title = stem.to_string_lossy().into_owned();
        }
        let bytes = self.package.to_bytes().map_err(|e| e.to_string())?;
        // Write beside the target and rename, so a failure never leaves a half-written file.
        let tmp = path.with_extension("alfadoc.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.file = Some(path);
        Ok(Some(format!("Saved {name}")))
    }

    /// Brings a spreadsheet, document or presentation file into the editor that reads it.
    /// The file is not linked: it is copied in and becomes part of the package on Save.
    fn import_file(&mut self, ctx: &egui::Context) -> Result<Option<String>, String> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Office files", &["xlsx", "xlsm", "xltx", "xls", "xlsb", "ods", "csv", "tsv", "docx", "docm", "dotx", "doc", "odt", "rtf", "pptx", "potx", "ppsx"])
            .pick_file()
        else {
            return Ok(None);
        };
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let ext = path.extension().map_or_else(String::new, |e| e.to_string_lossy().into_owned());
        let tab = package::editor_for(&ext).ok_or_else(|| format!("Alfadoc cannot import .{ext} files."))?;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        self.switch(ctx, tab);
        match tab {
            Tab::Sheet => self.grid.as_mut().map_or(Ok(()), |app| package::load_sheet(app, &name, &bytes))?,
            Tab::Document => self.word.as_mut().map_or(Ok(()), |app| package::load_document(app, &name, &bytes))?,
            Tab::Slides => self.deck.as_mut().map_or(Ok(()), |app| package::load_deck(app, &name, &bytes))?,
            Tab::Data => {}
        }
        Ok(Some(format!("Imported {name}")))
    }

    /// Writes the current editor's document as an Office file.
    fn export_file(&mut self) -> Result<Option<String>, String> {
        let (label, ext) = match self.active {
            Tab::Sheet => ("Excel workbook", "xlsx"),
            Tab::Document => ("Word document", "docx"),
            Tab::Slides => ("PowerPoint presentation", "pptx"),
            Tab::Data => return Err("Export applies to the Sheet, Document and Slides tabs.".into()),
        };
        let stem = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
        let Some(mut path) = rfd::FileDialog::new().add_filter(label, &[ext]).set_file_name(format!("{stem}.{ext}")).save_file() else {
            return Ok(None);
        };
        if path.extension().is_none() {
            path.set_extension(ext);
        }
        let bytes = match self.active {
            Tab::Sheet => self.grid.as_mut().map(package::save_sheet),
            Tab::Document => self.word.as_ref().map(package::save_document),
            Tab::Slides => self.deck.as_mut().map(package::save_deck),
            Tab::Data => None,
        }
        .ok_or("Nothing to export in this editor yet.")??;
        let tmp = path.with_extension(format!("{ext}.tmp"));
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(Some(format!("Exported {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()))))
    }

    /// Loads a package into the editors, tab by tab, so each one's look is saved and restored the
    /// way a normal tab change does it.
    fn open_package(&mut self, ctx: &egui::Context, path: Option<PathBuf>) -> Result<Option<String>, String> {
        let picked = path.or_else(|| rfd::FileDialog::new().add_filter("Alfadoc", &["alfadoc"]).pick_file());
        let Some(path) = picked else {
            return Ok(None);
        };
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let package = Alfadoc::from_bytes(&bytes).map_err(|e| e.to_string())?;
        let mut first: Option<Tab> = None;
        for (tab, part) in [(Tab::Sheet, package::SHEET_PART), (Tab::Document, package::DOC_PART), (Tab::Slides, package::DECK_PART)] {
            let Some(data) = package.parts.get(part) else { continue };
            self.switch(ctx, tab);
            match tab {
                Tab::Sheet => {
                    if let Some(app) = self.grid.as_mut() {
                        package::load_sheet(app, "Alfadoc.xlsx", data)?;
                    }
                }
                Tab::Document => {
                    if let Some(app) = self.word.as_mut() {
                        package::load_document(app, "Alfadoc.docx", data)?;
                    }
                }
                Tab::Slides => {
                    if let Some(app) = self.deck.as_mut() {
                        package::load_deck(app, "Alfadoc.pptx", data)?;
                    }
                }
                Tab::Data => {}
            }
            first.get_or_insert(tab);
        }
        if let Some(tab) = first {
            self.switch(ctx, tab);
        }
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.package = package;
        self.file = Some(path);
        Ok(Some(format!("Opened {name}")))
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
                if ui.button("Links").on_hover_text("Link tables and ranges between the editors").clicked() {
                    self.draft.open = !self.draft.open;
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
        if self.frames > 0
            && let Some(path) = self.startup.take()
        {
            let result = self.open_package(ctx, Some(path));
            self.report(ctx, result);
        }
        if let Some(action) = self.want.take() {
            self.file_action(ctx, action);
        }
        self.expire_toast(ctx);
        self.watch_links(ctx);
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
            Tab::Data => {}
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
            Tab::Data => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(tab) = self.tab_bar(ui)
            && tab != self.active
        {
            self.pending = Some(tab);
            ui.ctx().request_repaint();
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| match self.active {
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
                self.data.show(ui, &mut self.package.maix, package::now_ms());
            }
        });
        self.link_window(ui.ctx());
        self.frames = self.frames.saturating_add(1);
    }
}
