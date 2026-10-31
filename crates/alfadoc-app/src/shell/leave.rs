//! Leaving, dropping and exporting: the "Save changes?" question when the window is closed (or a
//! dropped package would replace unsaved work), files dropped on the window, and the Export
//! dialog (Office file or PDF, optionally flattened for the current dimensions).

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use alfadoc_adapter_deck::DeckEngine;
#[cfg(not(target_arch = "wasm32"))]
use alfadoc_adapter_grid::GridEngine;
use alfadoc_adapter_word::WordEngine;
use alfadoc_bridge::Engine;
use serde_json::json;

use super::{FileAction, IMPORT_EXTS, Shell, Tab};
#[cfg(not(target_arch = "wasm32"))]
use crate::package;
#[cfg(not(target_arch = "wasm32"))]
use crate::protect::Unlock;

/// What the "Save changes?" window is guarding.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) enum Leave {
    /// Closing the window.
    Close,
    /// Opening a package that was dropped on the window.
    Open { name: String, bytes: Vec<u8>, path: Option<std::path::PathBuf> },
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Default)]
pub(super) struct LeaveState {
    pub(super) ask: Option<Leave>,
    /// The user decided: the next close request goes through.
    pub(super) approved_close: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum ExportKind {
    #[default]
    Office,
    Pdf,
}

/// The Export window's choices.
#[derive(Default)]
pub(super) struct ExportDialog {
    pub(super) open: bool,
    pub(super) kind: ExportKind,
    /// Leave out what belongs to other values of the dimensions (Document and Slides).
    pub(super) flatten: bool,
}

/// What a flattened export removed, so it can be put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flat {
    Nothing,
    Word(usize),
    Deck(usize),
}

#[cfg(not(target_arch = "wasm32"))]
enum Choice {
    Save,
    Discard,
    Cancel,
}

/// The lower-case extension of a file name.
fn extension(name: &str) -> String {
    name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

impl Shell {
    /// Records the saved state once the editors have settled after a Save or Open.
    pub(super) fn note_baseline(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if self.recovery.baseline_due(now) {
            let signature = self.edit_signature();
            self.recovery.set_baseline(signature);
        } else if self.recovery.baseline_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(1600));
        }
    }

    /// Whether the work differs from what was last saved or opened.
    pub(super) fn is_dirty(&mut self) -> bool {
        let signature = self.edit_signature();
        self.recovery.is_dirty(signature)
    }

    /// Every pass: dropped files, the hover hint, a close request and the questions they raise.
    pub(super) fn leave_step(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("alfadoc_drop_hint")));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Drop an .alfadoc file to open it,\nor an Excel, Word or PowerPoint file to import it", egui::FontId::proportional(22.0), egui::Color32::WHITE);
        }
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for file in dropped {
            self.accept_drop(ctx, file);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if ctx.input(|i| i.viewport().close_requested()) && !self.leave.approved_close && (self.leave.ask.is_some() || self.is_dirty()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.leave.ask.is_none() {
                self.leave.ask = Some(Leave::Close);
            }
        }
        self.leave_window(ctx);
    }

    fn accept_drop(&mut self, ctx: &egui::Context, file: egui::DroppedFileHandle) {
        let name = file.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let ext = extension(&name);
        let is_package = ext == "alfadoc";
        if !is_package && !IMPORT_EXTS.contains(&ext.as_str()) {
            self.say(ctx, format!("Alfadoc cannot open .{ext} files. Drop an .alfadoc file or an Excel, Word or PowerPoint file."));
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The browser hands over the bytes later; they join the files its pickers deliver.
            let inbox = self.web_inbox.clone();
            let ctx = ctx.clone();
            let kind = if is_package { super::Incoming::Open } else { super::Incoming::Import };
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(bytes) = file.bytes_async().await {
                    inbox.borrow_mut().push((kind, name, bytes));
                    ctx.request_repaint();
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = match file.bytes() {
                Ok(b) => b,
                Err(e) => {
                    self.say(ctx, format!("Failed: {e}"));
                    return;
                }
            };
            if !is_package {
                let result = self.import_bytes(ctx, &name, &bytes);
                self.report(ctx, result);
            } else if self.is_dirty() {
                self.leave.ask = Some(Leave::Open { name, bytes, path: Some(file.path().to_path_buf()) });
            } else {
                self.open_loaded(ctx, name, bytes, Some(file.path().to_path_buf()));
            }
        }
    }

    /// Opens package bytes (asking for the passphrase first when they are encrypted).
    #[cfg(not(target_arch = "wasm32"))]
    fn open_loaded(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>, path: Option<PathBuf>) {
        if alfadoc_model::security::is_encrypted(&bytes) {
            self.unlock = Some(Unlock { name, path, bytes, pass: String::new(), error: None, recovered: None });
            return;
        }
        let result = self.load_package(ctx, &name, &bytes).map(|message| {
            self.file = Some(path.unwrap_or_else(|| PathBuf::from(&name)));
            message
        });
        self.report(ctx, result);
    }

    #[cfg(target_arch = "wasm32")]
    fn leave_window(&mut self, _ctx: &egui::Context) {}

    /// "Save changes?" with Save, Don't save and Cancel.
    #[cfg(not(target_arch = "wasm32"))]
    fn leave_window(&mut self, ctx: &egui::Context) {
        let Some(ask) = &self.leave.ask else { return };
        let what = match ask {
            Leave::Close => "before closing",
            Leave::Open { .. } => "before opening the dropped file",
        };
        let name = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
        let mut choice = None;
        egui::Window::new("Unsaved changes").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(format!("Save the changes to {name} {what}?"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(Choice::Save);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && choice.is_none() {
            choice = Some(Choice::Cancel);
        }
        match choice {
            None => {}
            Some(Choice::Cancel) => self.leave.ask = None,
            Some(Choice::Discard) => self.leave_proceed(ctx),
            Some(Choice::Save) => match self.save_package(false) {
                // Saved: carry on with what was asked.
                Ok(Some(message)) => {
                    self.say(ctx, message);
                    self.set_title(ctx);
                    self.leave_proceed(ctx);
                }
                // The save dialog was cancelled: the question stays.
                Ok(None) => {}
                Err(e) => self.say(ctx, format!("Failed: {e}")),
            },
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn leave_proceed(&mut self, ctx: &egui::Context) {
        match self.leave.ask.take() {
            Some(Leave::Close) => {
                self.leave.approved_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(Leave::Open { name, bytes, path }) => self.open_loaded(ctx, name, bytes, path),
            None => {}
        }
    }

    /// The Export window: the file type and, for a document or slides, whether to leave out what
    /// belongs to other dimension values.
    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export.open {
            return;
        }
        let tab = self.active;
        let dims = alfadoc_model::dims::canonical(&self.package.maix.active_dims).replace('|', ", ");
        let mut go = false;
        let mut cancel = false;
        let mut open = true;
        egui::Window::new("Export").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let office = match tab {
                Tab::Sheet => "Excel workbook (.xlsx)",
                Tab::Document => "Word document (.docx)",
                Tab::Slides => "PowerPoint presentation (.pptx)",
                Tab::Data | Tab::Links => "",
            };
            if office.is_empty() {
                ui.label("Export applies to the Sheet, Document and Slides tabs.");
                cancel = ui.button("Close").clicked();
                return;
            }
            ui.radio_value(&mut self.export.kind, ExportKind::Office, office);
            if cfg!(not(target_arch = "wasm32")) {
                ui.radio_value(&mut self.export.kind, ExportKind::Pdf, "PDF");
            } else {
                self.export.kind = ExportKind::Office;
            }
            ui.add_space(6.0);
            let can_flatten = matches!(tab, Tab::Document | Tab::Slides);
            let label = if dims.is_empty() { "Leave out content marked for other dimensions".to_string() } else { format!("Only what belongs to the current dimensions ({dims})") };
            ui.add_enabled_ui(can_flatten, |ui| {
                ui.checkbox(&mut self.export.flatten, label).on_hover_text("Blocks and shapes marked for other values are left out of the file. Your document is not changed.").on_disabled_hover_text("Sheets are exported as they are.");
            });
            if can_flatten && self.export.flatten {
                ui.weak("Marks that belong to the current dimensions stay in the file as hidden tags.");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                go = ui.button("Export…").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if !open || cancel {
            self.export.open = false;
        } else if go {
            self.export.open = false;
            self.want = Some(FileAction::Export);
        }
    }

    /// Takes out what belongs to other dimension values, if the Export window asked for it.
    fn flatten_begin(&mut self) -> Result<Flat, String> {
        if !self.export.flatten {
            return Ok(Flat::Nothing);
        }
        let dims = self.package.maix.active_dims.clone();
        match self.active {
            Tab::Document => match self.word.as_mut() {
                Some(app) => WordEngine::borrowed(&mut app.session).keep_variants(&dims).map(Flat::Word).map_err(|e| e.to_string()),
                None => Ok(Flat::Nothing),
            },
            Tab::Slides => match self.deck.as_mut() {
                Some(app) => DeckEngine::borrowed(&mut app.session).drop_other_variants(&dims).map(Flat::Deck).map_err(|e| e.to_string()),
                None => Ok(Flat::Nothing),
            },
            Tab::Sheet | Tab::Data | Tab::Links => Ok(Flat::Nothing),
        }
    }

    /// Puts back what [`flatten_begin`](Self::flatten_begin) took out.
    fn flatten_end(&mut self, flat: Flat) {
        match flat {
            Flat::Word(n) if n > 0 => {
                if let Some(app) = self.word.as_mut() {
                    // One undo brings back every block that was taken out.
                    let _ = WordEngine::borrowed(&mut app.session).execute("edit.undo", &json!({}));
                }
            }
            Flat::Deck(n) if n > 0 => {
                if let Some(app) = self.deck.as_mut() {
                    let _ = DeckEngine::borrowed(&mut app.session).undo_steps(n);
                }
            }
            Flat::Word(_) | Flat::Deck(_) | Flat::Nothing => {}
        }
    }

    /// Runs `job` with the flattened document (when asked for) and puts the document back after.
    /// A document that was saved stays "saved": taking things out and back is not an edit.
    fn flattened<T>(&mut self, job: impl FnOnce(&mut Shell) -> Result<T, String>) -> Result<T, String> {
        let was_clean = !self.is_dirty();
        let flat = self.flatten_begin()?;
        let result = job(self);
        self.flatten_end(flat);
        if flat != Flat::Nothing && was_clean {
            let signature = self.edit_signature();
            self.recovery.set_baseline(signature);
        }
        result
    }

    /// The current editor's document as an Office file: its bytes and extension.
    pub(super) fn export_bytes(&mut self) -> Result<(Vec<u8>, &'static str), String> {
        self.flattened(|shell| shell.export_plain())
    }

    /// Writes the current editor's document as a PDF (where the engine lays it out for print).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn export_pdf(&mut self) -> Result<Option<String>, String> {
        if matches!(self.active, Tab::Data | Tab::Links) {
            return Err("Export applies to the Sheet, Document and Slides tabs.".into());
        }
        let stem = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
        let Some(mut path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem}.pdf")).save_file() else {
            return Ok(None);
        };
        if path.extension().is_none() {
            path.set_extension("pdf");
        }
        let tmp = std::env::temp_dir().join(format!("alfadoc-export-{}-{}.pdf", std::process::id(), package::now_ms()));
        let tmp_text = tmp.to_string_lossy().into_owned();
        let written = self.flattened(|shell| shell.engine_pdf(&tmp_text));
        let result = written.and_then(|()| std::fs::read(&tmp).map_err(|e| e.to_string())).and_then(|bytes| {
            let out_tmp = path.with_extension("pdf.tmp");
            std::fs::write(&out_tmp, &bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&out_tmp, &path).map_err(|e| e.to_string())
        });
        let _ = std::fs::remove_file(&tmp);
        result?;
        Ok(Some(format!("Exported {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()))))
    }

    /// Asks the active editor's engine to write its PDF to `path`.
    #[cfg(not(target_arch = "wasm32"))]
    fn engine_pdf(&mut self, path: &str) -> Result<(), String> {
        let missing = || "Nothing to export in this editor yet.".to_string();
        match self.active {
            Tab::Sheet => GridEngine::borrowed(&mut self.grid.as_mut().ok_or_else(missing)?.session).execute("file.exportPdf", &json!({ "path": path })).map(|_| ()).map_err(|e| e.to_string()),
            Tab::Document => WordEngine::borrowed(&mut self.word.as_mut().ok_or_else(missing)?.session).execute("file.exportPdf", &json!({ "path": path })).map(|_| ()).map_err(|e| e.to_string()),
            Tab::Slides => DeckEngine::borrowed(&mut self.deck.as_mut().ok_or_else(missing)?.session).execute("file.export", &json!({ "path": path, "format": "pdf" })).map(|_| ()).map_err(|e| e.to_string()),
            Tab::Data | Tab::Links => Err("Export applies to the Sheet, Document and Slides tabs.".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_lower_case_and_may_be_missing() {
        assert_eq!(extension("Budget.XLSX"), "xlsx");
        assert_eq!(extension("a.b.alfadoc"), "alfadoc");
        assert_eq!(extension("README"), "");
    }

    #[test]
    fn every_importable_type_is_lower_case() {
        assert!(IMPORT_EXTS.iter().all(|e| *e == e.to_ascii_lowercase()));
    }
}
