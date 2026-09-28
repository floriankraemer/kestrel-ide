//! One integration-test binary for app's real-binary-under-Xvfb E2E flows,
//! so the crate's dependency tree links once instead of once per file.
//! `support` holds the app-specific helpers shared across modules; the
//! harness itself stays in `crates/e2e`. See `core.rs` for what these
//! flows cover and do not.
mod support;

mod about;
mod analysis;
mod build_tools;
mod containers;
mod core;
mod database;
mod database_console;
mod diff;
mod edit;
mod editor_popups;
mod lazy_tree;
mod minimap;
mod panes;
mod preview;
mod run;
mod scope;
mod vcs;
