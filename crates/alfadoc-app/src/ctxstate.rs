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
