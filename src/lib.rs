//! `claude-cockpit` — a live, read-only terminal dashboard for Claude Code and
//! your dev environment.
//!
//! This library hosts every module the `claude-cockpit` binary is built from
//! (collectors, render stacks, the UI state machine). It exists primarily so
//! the binary has a documented library target; the public surface is internal
//! and not a stability guarantee.

pub mod app;
pub mod collect;
pub mod config;
pub mod event;
pub mod graph;
pub mod layout;
pub mod refresh;
pub mod render;
pub mod theme;
pub mod trend;
pub mod util;
pub mod widget;
