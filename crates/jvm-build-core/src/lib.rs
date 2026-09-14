//! Gradle and Maven project sync (the jvm-build-tools plan, ADR-0057).
//!
//! Qt-free, like every crate below `ui-shell`. Delegates and never models
//! (extends [`build-core`](../../build-core)'s own rule, ADR-0040): a
//! [`model::BuildModel`] is exactly what the build tool itself reported —
//! no IDE-owned notion of an output path, an artifact, or a "correct"
//! dependency version.
//!
//! This crate's A-phase scope (this PR) is the foundation: the model types
//! ([`model`]), the reload policy and trust gate ([`sync`]), and a
//! temporary run-config builder ([`run`]). The process-invoking Gradle and
//! Maven providers (`gradle::*`, `maven::*`, the plan's A4/A5) and the
//! editing-assistance modules (`deps`, `editing::*`, phase D) are later
//! work and are not present yet — see `docs/architecture/jvm-build-tools-plan.md`.

pub mod gradle;
pub mod maven;
pub mod model;
pub mod run;
pub mod sync;
