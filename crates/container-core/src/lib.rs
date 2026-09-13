//! Docker/Podman integration, CLI-driven (ADR-0055).
//!
//! Qt-free, no tokio: every operation is a blocking `process_exec::run`/
//! `spawn` call or a `pty_core` session, exactly as `analysis-core` and
//! `test-core` already do — reported back to `ui-shell` through a
//! `Send` closure it forwards onto `CxxQtThread::queue()`.
//!
//! This crate lands in phases (see `docs/architecture/containers-plan.md`).
//! C1 — this one — is the foundation: [`connection`] turns a persisted
//! [`app_config::ContainerConnectionSetting`] into a runnable [`Invocation`
//! (crate::connection::Invocation)], [`discovery`] finds connections the
//! user has not typed in yet, and [`probe`] is "Test connection". C2 adds
//! [`model`]/[`snapshot`] (typed `inspect` output and one engine's whole
//! state), [`watcher`] (the per-connection thread that follows `events`)
//! and [`tree`] (the dock's flattened rows). Operations, run
//! configurations, registries, run targets and recreate/editing land in
//! later tasks and are not implemented yet.

/// Connections: [`connection::Engine`], [`connection::ConnectionKind`], and
/// [`connection::Invocation`] — a persisted connection turned into an
/// actual command line.
pub mod connection;
/// Finding connections the user has not typed in by hand: CLI contexts,
/// Podman connections/machines, and well-known socket presets.
pub mod discovery;
/// The Files tab (C3): `ls -la` (GNU and busybox) parsing and a minimal
/// single-entry tar reader for `cp <id>:<path> -`.
pub mod files;
/// Typed, lenient views over `inspect` JSON: [`model::Container`],
/// [`model::Image`], [`model::Volume`], [`model::Network`], [`model::Pod`].
pub mod model;
/// Container lifecycle operations (C3): start/stop/restart/remove/pause/
/// unpause/prune argv builders, the [`ops::run_op`] executor and its typed
/// [`ops::OpError`], and the `top` process-table parser.
pub mod ops;
/// "Test connection": `<cli> version --format json`, parsed into
/// [`probe::EngineInfo`] or a [`probe::ConnectionError`].
pub mod probe;
/// Streaming sessions (C3): `pty_core::ShellSpec` builders for Log/
/// Terminal/Exec/Attach, and `inspect` pretty-printing for the Inspect tab.
pub mod session;
/// One engine's whole state ([`snapshot::EngineSnapshot`]), compose
/// grouping, and the filter/search views over it.
pub mod snapshot;
/// The Containers dock's rows, flattened and ordered ([`tree::flatten`]).
pub mod tree;
/// Per-connection watcher thread: probe, snapshot, `events` → debounce →
/// re-snapshot, reported as [`watcher::ContainerEvent`]s.
pub mod watcher;
