//! The Alfadoc data model: the `.alfadoc` container, the maix store, dimensions and links.
//!
//! Pure data and (de)serialisation. No UI, no engine: every crate above this one builds on it.
//! Nothing here panics on hostile input; sizes and entry counts are capped (see [`container`]).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod container;
pub mod dims;
pub mod error;
pub mod format;
pub mod formula;
pub mod links;
pub mod maix;
pub mod security;
pub mod shift;
pub mod variant;

pub use container::{Alfadoc, Manifest};
pub use dims::{DimVector, dim_hash};
pub use error::{Error, Result};
pub use links::{Anchor, Endpoint, Link, Snapshot, SyncMode, Target};
pub use format::{Axis, CellStyle, HAlign, Layout, Rect, Shift, SheetFormat};
pub use maix::{Maix, Stamp, Value};
