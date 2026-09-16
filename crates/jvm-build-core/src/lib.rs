//! Gradle and Maven project sync (the jvm-build-tools plan, ADR-0057).
//!
//! Qt-free, like every crate below `ui-shell`. Delegates and never models
//! (extends [`build-core`](../../build-core)'s own rule, ADR-0040): a
//! [`model::BuildModel`] is exactly what the build tool itself reported —
//! no IDE-owned notion of an output path, an artifact, or a "correct"
//! dependency version.
//!
//! The model types ([`model`]), the reload policy and trust gate
//! ([`sync`]), a temporary run-config builder ([`run`]), the
//! process-invoking Gradle and Maven providers ([`gradle`], [`maven`]) and
//! the dock's tree shaping ([`view`]) are phases A/B; phase D adds
//! build-file editing assistance ([`editing`]) and the dependency
//! analyzer ([`deps`]) — see `docs/architecture/jvm-build-tools-plan.md`
//! for the Progress table.

/// The dependency analyzer (D8): the Dependencies subtree's scope filter
/// and "Conflicts only" toggle, and "Go to declaration".
pub mod deps;
/// Build-file editing assistance (phase D): caret-context classification,
/// the local repository index, a Maven Central client, completion and
/// "newer version" hints.
pub mod editing;
pub mod gradle;
pub mod maven;
pub mod model;
pub mod run;
pub mod sync;
/// Shaping a synced model into the Build Tools dock's tree (B2).
pub mod view;
