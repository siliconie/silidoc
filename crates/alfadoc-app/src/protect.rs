//! Passphrase protection in the window (`alfadoc_model::security`): encrypt the open document,
//! unlock a protected file, sign a copy and check a signature.
//!
//! A protected document stays protected: every Save, and every recovery copy, is written
//! encrypted. The passphrase itself is not kept, only the key made from it.

use std::path::PathBuf;

use alfadoc_model::security;
#[cfg(not(target_arch = "wasm32"))]
use alfadoc_model::security::Verdict;

#[cfg(not(target_arch = "wasm32"))]
use crate::package;
use crate::recovery;
use crate::shell::Shell;

/// A protected file waiting for its passphrase.
pub struct Unlock {
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
    pub pass: String,
    pub error: Option<String>,
    /// The recovery copy this file came from; it is deleted once the file has been opened.
    pub recovered: Option<recovery::Found>,
}

/// The Protect window's fields.
#[derive(Default)]
pub struct Protect {
    pub open: bool,
    pass: String,
    again: String,
    signer: String,
    sign_pass: String,
    message: String,
}

/// Shortest passphrase the window accepts.
const MIN_LEN: usize = 8;

#[derive(Clone, Copy)]
enum Act {
    Protect,
    Remove,
    #[cfg(not(target_arch = "wasm32"))]
    SignCopy,
    #[cfg(not(target_arch = "wasm32"))]
    Check,
}

impl Shell {
    pub(crate) fn protect_window(&mut self, ctx: &egui::Context) {
        if !self.protect.open {
            return;
        }
        let mut open = true;
        let mut act: Option<Act> = None;
        let protected = self.key.is_some();
        egui::Window::new("Protect").open(&mut open).default_width(400.0).show(ctx, |ui| {
            let p = &mut self.protect;
            ui.strong("Encrypt this document");
            if protected {
                ui.label("This document is protected. Every Save writes an encrypted file, and so do the recovery copies.");
                if ui.button("Remove protection").clicked() {
                    act = Some(Act::Remove);
                }
            } else {
                ui.label(format!("Choose a passphrase of at least {MIN_LEN} characters. Nobody, including us, can open the file without it."));
                ui.add(egui::TextEdit::singleline(&mut p.pass).password(true).hint_text("Passphrase"));
                ui.add(egui::TextEdit::singleline(&mut p.again).password(true).hint_text("Passphrase again"));
                if ui.button("Protect with this passphrase").clicked() {
                    act = Some(Act::Protect);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                ui.separator();
                ui.strong("Sign a copy, or check a signed file");
                ui.label("A signature shows that a file has not changed since it was signed. Only people who know the passphrase can sign or check.");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.signer).hint_text("Signed by").desired_width(120.0));
                    ui.add(egui::TextEdit::singleline(&mut p.sign_pass).password(true).hint_text("Signing passphrase").desired_width(150.0));
                });
                ui.horizontal(|ui| {
                    if ui.button("Save a signed copy…").clicked() {
                        act = Some(Act::SignCopy);
                    }
                    if ui.button("Check a file…").clicked() {
                        act = Some(Act::Check);
                    }
                });
            }
            if !p.message.is_empty() {
                ui.separator();
                ui.label(&p.message);
            }
        });
        self.protect.open = open;
        let Some(act) = act else {
            return;
        };
        self.protect.message = match self.protect_act(act) {
            Ok(text) => text,
            Err(e) => e,
        };
        self.set_title(ctx);
    }

    fn protect_act(&mut self, act: Act) -> Result<String, String> {
        match act {
            Act::Protect => {
                let (pass, again) = (self.protect.pass.clone(), self.protect.again.clone());
                if pass.chars().count() < MIN_LEN {
                    return Err(format!("Use at least {MIN_LEN} characters."));
                }
                if pass != again {
                    return Err("The two passphrases are different.".into());
                }
                self.key = Some(security::Protector::new(&pass).map_err(|e| e.to_string())?);
                self.protect.pass.clear();
                self.protect.again.clear();
                self.recovery.mark_unsaved();
                Ok("Protected. Save to write the encrypted file.".into())
            }
            Act::Remove => {
                self.key = None;
                self.recovery.mark_unsaved();
                Ok("Protection removed. The next Save is not encrypted.".into())
            }
            #[cfg(not(target_arch = "wasm32"))]
            Act::SignCopy => {
                if self.key.is_some() {
                    return Err("Remove protection first: an encrypted file cannot be signed. Sign the unprotected copy.".into());
                }
                let (signer, pass) = (self.protect.signer.trim().to_string(), self.protect.sign_pass.clone());
                if signer.is_empty() || pass.is_empty() {
                    return Err("Enter who signs and a signing passphrase.".into());
                }
                let stem = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
                let Some(mut path) = rfd::FileDialog::new().add_filter("Alfadoc", &["alfadoc"]).set_file_name(format!("{stem}-signed.alfadoc")).save_file() else {
                    return Ok(String::new());
                };
                if path.extension().is_none() {
                    path.set_extension("alfadoc");
                }
                let bytes = self.package_bytes(&stem)?;
                let signed = security::sign(&bytes, &signer, &pass, package::now_ms()).map_err(|e| e.to_string())?;
                std::fs::write(&path, signed).map_err(|e| e.to_string())?;
                Ok(format!("Saved a copy signed by {signer}."))
            }
            #[cfg(not(target_arch = "wasm32"))]
            Act::Check => {
                let pass = self.protect.sign_pass.clone();
                let Some(path) = rfd::FileDialog::new().add_filter("Alfadoc", &["alfadoc"]).pick_file() else {
                    return Ok(String::new());
                };
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                if security::is_encrypted(&bytes) {
                    return Err("That file is encrypted. Open it first; an encrypted file carries no signature.".into());
                }
                Ok(match security::verify(&bytes, &pass).map_err(|e| e.to_string())? {
                    Verdict::NotSigned => "That file is not signed.".to_string(),
                    Verdict::Valid { signer, signed_at } => format!("Signed by {signer}, {}. Nothing has changed since.", recovery::ago(package::now_ms(), signed_at)),
                    Verdict::Mismatch { signer } => format!("The signature by {signer} does not match: the file was changed, or this is not the passphrase it was signed with."),
                })
            }
        }
    }

    /// Asks for the passphrase of a protected file that was just picked.
    pub(crate) fn unlock_window(&mut self, ctx: &egui::Context) {
        let Some(unlock) = self.unlock.as_mut() else {
            return;
        };
        let (mut go, mut cancel) = (false, false);
        egui::Window::new("Protected file").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(format!("{} is protected. Enter its passphrase to open it.", unlock.name));
            let field = ui.add(egui::TextEdit::singleline(&mut unlock.pass).password(true).hint_text("Passphrase"));
            if !field.has_focus() && unlock.error.is_none() {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            if let Some(e) = &unlock.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.unlock = None;
            return;
        }
        if !go {
            return;
        }
        let Some(mut unlock) = self.unlock.take() else {
            return;
        };
        let opened = security::open(&unlock.bytes, &unlock.pass).map_err(|e| e.to_string());
        match opened {
            Ok((plain, key)) => match self.load_package(ctx, &unlock.name, &plain) {
                Ok(message) => {
                    self.file = unlock.path.take();
                    self.key = Some(key);
                    match unlock.recovered.take() {
                        // Recovered work is still unsaved: a new copy follows, then the old one goes.
                        Some(found) => {
                            self.recovery.mark_unsaved();
                            found.discard();
                        }
                        None => self.recovery.saved(),
                    }
                    if let Some(m) = message {
                        self.say(ctx, m);
                    }
                    self.set_title(ctx);
                }
                Err(e) => self.say(ctx, format!("Failed: {e}")),
            },
            Err(e) => {
                unlock.error = Some(e);
                unlock.pass.clear();
                self.unlock = Some(unlock);
            }
        }
    }
}
