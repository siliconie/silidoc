use std::collections::BTreeMap;

use alfadoc_model::{Anchor, Value};
use serde_json::Value as Json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("bad parameters: {0}")]
    BadParams(String),
    #[error("this engine cannot do that: {0}")]
    Unsupported(String),
    #[error("the engine refused: {0}")]
    Engine(String),
    #[error("anchor not found: {0}")]
    AnchorNotFound(String),
    #[error(transparent)]
    Model(#[from] alfadoc_model::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Word,
    Grid,
    Deck,
}

/// What Alfadoc needs from an engine's command surface. Every engine already routes all actions
/// through one `execute(id, json)` entry point; adapters forward to it.
pub trait Engine {
    fn kind(&self) -> EngineKind;
    fn execute(&mut self, command: &str, params: &Json) -> Result<Json, BridgeError>;
    /// Whether `command` may change the document. Used to decide which executed commands count as
    /// "a change" for live sync. WordCraft publishes this per command; for engines that do not,
    /// adapters answer `true` for everything except known queries (a false positive only costs one
    /// no-op sync pass).
    fn is_mutating(&self, command: &str) -> bool;
    /// A counter the engine bumps whenever the document changes, if it keeps one (GridCraft's
    /// `DocState::revision`). When present it is the authority on "did that command change
    /// anything", so a command that turned out to be a no-op does not trigger a sync pass.
    fn revision(&self) -> Option<u64> {
        None
    }
}

/// A table in an editor, as offered by the "pick a table" list. `nth` counts from 1 in document
/// order, the same count the adapters' `tag_table` / `name_nth_table` take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableChoice {
    pub nth: usize,
    pub rows: usize,
    pub cols: usize,
    /// The first cells of the first row, to tell tables apart.
    pub preview: String,
    /// Where it is: empty in a document, `Slide 3` in a presentation.
    pub place: String,
}

impl TableChoice {
    /// `Table 2 · 3×2 · Item · Qty` (with the place first when there is one).
    pub fn label(&self) -> String {
        let mut out = String::new();
        if !self.place.is_empty() {
            out.push_str(&self.place);
            out.push_str(" · ");
        }
        out.push_str(&format!("Table {} · {}×{}", self.nth, self.rows, self.cols));
        if !self.preview.is_empty() {
            out.push_str(" · ");
            out.push_str(&self.preview);
        }
        out
    }
}

/// The first three non-empty cell texts joined with ` · `, cut to 40 characters.
pub fn preview_of(cells: impl Iterator<Item = String>) -> String {
    let parts: Vec<String> = cells.map(|c| c.trim().replace('\n', " ")).filter(|c| !c.is_empty()).take(3).collect();
    let text = parts.join(" · ");
    if text.chars().count() > 40 { format!("{}…", text.chars().take(39).collect::<String>()) } else { text }
}

/// The contents of a linked region as a grid of values, **relative to the anchor's top-left**.
/// A Word table, a sheet range and a slide table all read and write as `Region`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub cells: BTreeMap<(u32, u16), Value>,
}

/// One cell to set (`Some`) or clear (`None`), relative to the anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct CellChange {
    pub row: u32,
    pub col: u16,
    pub value: Option<Value>,
}

/// Reading and writing the cells behind an [`Anchor`], in whichever engine (or the maix store)
/// owns it. Implementations write through the engine's own commands so undo, journaling and
/// recalculation all keep working.
pub trait RegionAccess {
    fn read_region(&self, anchor: &Anchor) -> Result<Region, BridgeError>;
    fn write_changes(&mut self, anchor: &Anchor, changes: &[CellChange]) -> Result<(), BridgeError>;
}
