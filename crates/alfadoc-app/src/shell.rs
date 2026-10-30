//! The Alfadoc window: a tab bar over one of the three engines' full editors.
//!
//! * Engines are created **lazily**, the first time their tab is opened, so start-up pays only for
//!   the tab you begin on (WordCraft's font database is the heavy one).
//! * Only the active engine runs `logic` and `ui`; the others keep their documents in memory.
//! * The engines share one egui context, so the shell swaps their fonts/colours/zoom on every tab
//!   change ([`ContextState`]).

use crate::ctxstate::ContextState;
use crate::hosts;

use deckcraft_ui_egui::SlideApp;
use gridcraft_ui_egui::SheetApp;
use wordcraft_ui_egui::WordApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Sheet,
    Document,
    Slides,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Sheet, Tab::Document, Tab::Slides];

    fn index(self) -> usize {
        match self {
            Tab::Sheet => 0,
            Tab::Document => 1,
            Tab::Slides => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Sheet => "Sheet",
            Tab::Document => "Document",
            Tab::Slides => "Slides",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Sheet => "Spreadsheet (GridCraft)  Alt+1",
            Tab::Document => "Document (WordCraft)  Alt+2",
            Tab::Slides => "Presentation (DeckCraft)  Alt+3",
        }
    }

    fn key(self) -> egui::Key {
        match self {
            Tab::Sheet => egui::Key::Num1,
            Tab::Document => egui::Key::Num2,
            Tab::Slides => egui::Key::Num3,
        }
    }

    /// `--tab sheet|doc|slides` picks the tab to start on.
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Tab {
        while let Some(arg) = args.next() {
            if arg == "--tab" {
                return match args.next().as_deref() {
                    Some("doc" | "document" | "word") => Tab::Document,
                    Some("slides" | "deck" | "slide") => Tab::Slides,
                    _ => Tab::Sheet,
                };
            }
        }
        Tab::Sheet
    }
}

pub struct Shell {
    active: Tab,
    /// A tab change asked for during `ui`, applied at the start of the next `logic` pass.
    pending: Option<Tab>,
    grid: Option<SheetApp>,
    word: Option<WordApp>,
    deck: Option<SlideApp>,
    /// The look each engine left on the context when its tab was last active.
    saved: [Option<ContextState>; 3],
    /// Frames drawn so far: fonts exist on the context only after the first one.
    frames: u64,
}

impl Shell {
    pub fn new(start: Tab) -> Shell {
        Shell { active: start, pending: None, grid: None, word: None, deck: None, saved: [None, None, None], frames: 0 }
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

    /// Draws the tab bar; returns the tab the user asked for, by click or Alt+1..3.
    fn tab_bar(&self, ui: &mut egui::Ui) -> Option<Tab> {
        let mut asked = ui.ctx().input_mut(|i| Tab::ALL.into_iter().find(|t| i.consume_key(egui::Modifiers::ALT, t.key())));
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
        });
        self.frames = self.frames.saturating_add(1);
    }
}
