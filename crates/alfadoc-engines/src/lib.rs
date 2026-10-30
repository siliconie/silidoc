//! The three engines as one dependency set.
//!
//! In M1 this crate gains the per-engine adapters (`Engine` + `RegionAccess` implementations) and
//! the contract tests that pin the upstream surface Alfadoc uses (docs/UPSTREAM.md). For now it
//! only proves that GridCraft, WordCraft and DeckCraft resolve and unify into one build.
