//! One palette for every tab.
//!
//! GridCraft is the reference: its colours, light and dark, are what the Data and Links tabs
//! wear. WordCraft and DeckCraft each have a hook (`theme::set_skin`) that makes their own
//! `Tokens` use a shared palette, and this installs the Sheet's palette into both. Without it
//! (any other host) the engines look exactly as they always did.
//!
//! Fonts are not part of the skin: each engine still loads its own interface faces.

use gridcraft_ui_egui::theme::Tokens;

/// The colours both hooks share, read off GridCraft's tokens for one mode.
#[derive(Clone, Copy)]
struct Palette {
    window: egui::Color32,
    surface: egui::Color32,
    canvas: egui::Color32,
    border: egui::Color32,
    text: egui::Color32,
    text_dim: egui::Color32,
    text_disabled: egui::Color32,
    accent: egui::Color32,
    accent_deep: egui::Color32,
    accent_soft: egui::Color32,
    on_accent: egui::Color32,
    hover: egui::Color32,
    pressed: egui::Color32,
    input: egui::Color32,
    input_border: egui::Color32,
}

/// Body text size of the Sheet's interface, in points.
const BODY: f32 = 13.0;

impl Palette {
    /// `deep` is the light theme's dark accent: it carries white text in both modes.
    fn of(t: &Tokens, deep: egui::Color32) -> Palette {
        Palette {
            window: t.window,
            surface: t.ribbon,
            canvas: t.desk,
            border: t.ribbon_border,
            text: t.text,
            text_dim: t.text_dim,
            text_disabled: t.text_disabled,
            accent: t.accent,
            accent_deep: deep,
            accent_soft: t.accent_soft,
            on_accent: t.on_accent,
            hover: t.hover,
            pressed: t.pressed,
            input: t.input_bg,
            input_border: t.input_border,
        }
    }
}

fn palettes() -> (Palette, Palette) {
    let (light, dark) = (Tokens::light(), Tokens::dark());
    let deep = light.accent_dark;
    (Palette::of(&light, deep), Palette::of(&dark, deep))
}

fn word_skin() -> wordcraft_ui_egui::theme::Skin {
    use wordcraft_ui_egui::theme::{Skin, SkinColors};
    let build = |p: Palette| SkinColors { window: p.window, surface: p.surface, canvas: p.canvas, border: p.border, text: p.text, text_dim: p.text_dim, text_disabled: p.text_disabled, accent: p.accent, accent_deep: p.accent_deep, accent_soft: p.accent_soft, on_accent: p.on_accent, hover: p.hover, pressed: p.pressed, input: p.input, input_border: p.input_border };
    let (light, dark) = palettes();
    Skin { light: build(light), dark: build(dark), body_size: BODY }
}

fn deck_skin() -> deckcraft_ui_egui::theme::Skin {
    use deckcraft_ui_egui::theme::{Skin, SkinColors};
    let build = |p: Palette| SkinColors { window: p.window, surface: p.surface, canvas: p.canvas, border: p.border, text: p.text, text_dim: p.text_dim, text_disabled: p.text_disabled, accent: p.accent, accent_deep: p.accent_deep, accent_soft: p.accent_soft, on_accent: p.on_accent, hover: p.hover, pressed: p.pressed, input: p.input, input_border: p.input_border };
    let (light, dark) = palettes();
    Skin { light: build(light), dark: build(dark), body_size: BODY }
}

/// Installs the shared palette. Call before any engine draws, once per process.
pub fn install() {
    wordcraft_ui_egui::theme::set_skin(Some(word_skin()));
    deckcraft_ui_egui::theme::set_skin(Some(deck_skin()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engines_get_the_sheets_accent_in_each_mode() {
        let (word, deck) = (word_skin(), deck_skin());
        assert_eq!(word.light.accent, Tokens::light().accent);
        assert_eq!(word.dark.accent, Tokens::dark().accent);
        assert_eq!(deck.light.accent, word.light.accent);
        assert_eq!(deck.dark.surface, Tokens::dark().ribbon);
    }

    #[test]
    fn the_deep_accent_is_the_same_dark_shade_in_both_modes() {
        let (word, _) = (word_skin(), deck_skin());
        assert_eq!(word.light.accent_deep, word.dark.accent_deep);
    }
}
