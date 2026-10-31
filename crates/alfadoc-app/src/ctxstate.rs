//! What each engine installs on the shared egui context.
//!
//! GridCraft, WordCraft and DeckCraft each set their own fonts, colours (both light and dark
//! styles) and interface zoom on the one `egui::Context` the window has. When the tab changes the
//! next engine would otherwise draw with the previous engine's look, and an engine that caches
//! "I already applied my theme" (WordCraft does) would never put it back.
//!
//! So the shell takes a snapshot of that state when it leaves a tab and puts it back when it
//! returns. This touches nothing inside the engines.

use std::sync::Arc;

use egui::{Context, FontDefinitions, Style, Theme};

pub struct ContextState {
    light: Arc<Style>,
    dark: Arc<Style>,
    fonts: FontDefinitions,
    zoom: f32,
}

impl ContextState {
    /// The context's current look. Only valid after the first frame has run (fonts exist then).
    pub fn capture(ctx: &Context) -> ContextState {
        ContextState {
            light: ctx.style_of(Theme::Light),
            dark: ctx.style_of(Theme::Dark),
            fonts: ctx.fonts_mut(|f| f.definitions().clone()),
            zoom: ctx.zoom_factor(),
        }
    }

    /// Puts the captured look back. Fonts apply from the next frame, as when an engine installs them.
    pub fn restore(&self, ctx: &Context) {
        ctx.set_style_of(Theme::Light, Arc::clone(&self.light));
        ctx.set_style_of(Theme::Dark, Arc::clone(&self.dark));
        ctx.set_fonts(self.fonts.clone());
        if (ctx.zoom_factor() - self.zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(self.zoom);
        }
    }
}

/// The interface brightness for the whole window. One choice, applied to every tab, so switching
/// tabs never flips between light and dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "Auto",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark => egui::ThemePreference::Dark,
        }
    }

    /// Whether the window is dark now; `system` is what the operating system reports, if anything.
    pub fn is_dark(self, system: Option<egui::Theme>) -> bool {
        match self {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => system == Some(egui::Theme::Dark),
        }
    }
}

/// The tabs that have no editor of their own (Data, Links) wear the Sheet's look, and are never
/// snapshotted: otherwise they would freeze whatever the previous tab happened to leave behind.
pub fn has_own_look(index: usize) -> bool {
    index < 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_appearance_decides_dark_for_every_tab() {
        assert!(Appearance::Dark.is_dark(None));
        assert!(!Appearance::Light.is_dark(Some(egui::Theme::Dark)));
        assert!(Appearance::System.is_dark(Some(egui::Theme::Dark)));
        assert!(!Appearance::System.is_dark(None));
    }

    #[test]
    fn only_the_three_editors_keep_their_own_look() {
        assert!([0, 1, 2].into_iter().all(has_own_look));
        assert!(!has_own_look(3) && !has_own_look(4));
    }
}
