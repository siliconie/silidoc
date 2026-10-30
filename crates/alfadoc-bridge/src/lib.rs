//! The bridge between Alfadoc and the three engines.
//!
//! The engines (WordCraft, GridCraft, DeckCraft) stay untouched upstream code. This crate defines
//! the small traits Alfadoc needs from them ([`Engine`], [`RegionAccess`]); per-engine adapter
//! crates implement the traits by calling each engine's stable command ids. Everything above the
//! adapters (the dispatcher, link syncing) is engine-agnostic and tested here with in-memory fakes.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod dispatch;
pub mod engine;
pub mod link_sync;
pub mod maix_access;
pub mod textcell;

pub use dispatch::{Change, Dispatcher};
pub use engine::{BridgeError, CellChange, Engine, EngineKind, Region, RegionAccess, TableChoice, preview_of};
pub use link_sync::{Outcome, resolve_cell, sync_link};
pub use maix_access::MaixAccess;
