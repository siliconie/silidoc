//! Sync primitives: three-way merge, debouncing and echo suppression.
//!
//! No I/O and no clocks: callers pass `now` in, so everything here is deterministic and testable.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod debounce;
pub mod merge;

pub use debounce::{Debouncer, EchoGuard};
pub use merge::{Conflict, Merge, Policy, merge3, one_way};
