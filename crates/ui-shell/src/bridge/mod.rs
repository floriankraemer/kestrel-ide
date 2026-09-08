// cxx-qt bridge boundary for ui-shell.
//
// Adapter layer only (ADR-0002): the QObjects here hold no domain state and
// decide nothing. They share the single `app_core::AppSession` and
// translate: slot → QString/QModelIndex → `AppSession` call → emit signal /
// refresh model. Errors cross as a typed code + message struct and tabs are
// identified by stable `TabId`s (ADR-0003).
//
// The layout is one module per feature, plus three that are shared:
//
// * [`ffi`] — the single `#[cxx_qt::bridge] mod ffi`, declarations only.
//   cxx-qt permits exactly one bridge module per crate and the shared FFI
//   structs (`FfiResult`, `FfiTextEdit`, …) are per-bridge types, so
//   splitting it would produce two unrelated sets of C++ types. It is
//   therefore exempt from the per-file size ceiling: its size is the size of
//   the seam, not a symptom of a module doing too much.
// * [`registry`] — the process-wide handles the adapters share, because
//   cxx-qt builds QObjects through `Default` with no injection point.
// * [`convert`] — translation helpers more than one feature module needs.
//
// Everything else is one feature's `…Rust` state struct and its `impl
// ffi::…` blocks. `ai` is two files because one would be over the ceiling:
// `chat` is the panel surface (conversation, attachments, applying an
// answer, history), `agent` is a run (the approval gate, `run_ask` /
// `run_agent`, and what the worker queues back onto the Qt thread).
//
// This crate is one rustc compilation unit, and nearly every module above
// references types `ffi` generates, so editing any one of them recompiles
// the whole crate — measured at ~17-23s per edit even for a one-line change
// in a 123-line leaf file, regardless of ccache/sccache/mold (all already
// in place, none of them touch this). Splitting the crate to shrink that
// isn't a quick option: it would mean multiple independent cxx-qt bridges
// (one per crate), each redeclaring the shared FFI types (`FfiResult`,
// `TabId`, …) identically, plus untangling QObject parent/child ownership
// that currently crosses these modules — an architecture-level rework, not
// a build tweak. Investigated 2026-08-30; re-check only if cxx-qt itself
// grows multi-bridge or incremental support for this pattern.

pub mod ai;
pub mod analysis;
pub mod app_info;
pub mod build;
pub mod convert;
pub mod debug;
pub mod diagnostics;
pub mod editor;
pub mod editor_ops;
pub mod errors;
pub mod ffi;
pub mod icons;
pub mod language;
pub mod layouts;
pub mod plugins;
pub mod preview;
pub mod registry;
pub mod run;
pub mod search;
pub mod settings;
pub mod terminal;
pub mod testing;
pub mod theme;
pub mod tree;
pub mod vcs;
pub mod window_state;

pub use ffi::run_app;
