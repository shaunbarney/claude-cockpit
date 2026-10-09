//! Projects: the set of repositories the cockpit can show, and how you manage it.
//!
//! The cockpit shows one project at a time. You register projects with
//! `claude-cockpit project add` (or `a` in the picker), switch with `P` / `:`,
//! and the picker suggests repos Claude Code has already worked in.

pub mod cli;
pub mod discover;
pub mod host;
pub mod model;
pub mod overview;
pub mod paths;
pub mod picker;
pub mod resolve;
pub mod store;

#[cfg(test)]
pub(crate) mod testutil;

pub use model::{Project, ProjectError, Registry};
pub use store::State;
