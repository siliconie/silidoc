//! The three engines as one dependency set, with their adapters re-exported under one roof.
//!
//! Each adapter lives in its own crate (`alfadoc-adapter-grid`, and the Word and Deck adapters
//! to come) so it can be built and tested without the other engines' UI stacks. This crate is what
//! the application depends on, and it proves that all three engines unify into one build.

pub use alfadoc_adapter_deck as deck;
pub use alfadoc_adapter_grid as grid;
pub use alfadoc_adapter_word as word;
