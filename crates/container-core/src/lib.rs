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
//! user has not typed in yet, and [`probe`] is "Test connection". Snapshot,
//! watching, operations, run configurations, registries, run targets and
//! recreate/editing land in later tasks and are not implemented yet.

/// Connections: [`connection::Engine`], [`connection::ConnectionKind`], and
/// [`connection::Invocation`] — a persisted connection turned into an
/// actual command line.
pub mod connection;
/// Finding connections the user has not typed in by hand: CLI contexts,
/// Podman connections/machines, and well-known socket presets.
pub mod discovery;
/// "Test connection": `<cli> version --format json`, parsed into
/// [`probe::EngineInfo`] or a [`probe::ConnectionError`].
pub mod probe;
