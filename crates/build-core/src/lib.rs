//! Building a project (B1): invoking the project's own build tool and
//! reading its diagnostics.
//!
//! Build is delegated, never modelled (ADR-0040). This crate knows how to
//! start `cargo`, `cmake`, `mvn` or `gradle` — through the one toolchain
//! table in `run_core::toolchain` (ADR-0039), never a second one — and how
//! to turn what they print back into diagnostics. It owns no notion of a
//! compiler, an output folder, a module path or an artifact: those are the
//! build tool's, and asking the IDE to hold a second opinion about them is
//! how the two drift apart.
//!
//! Qt-free by design (see `docs/architecture/layering.md`'s `build-core`
//! row), and thread-free: a build runs on whatever thread the adapter gives
//! it, exactly as `run-core` does.

pub mod cargo_json;
pub mod diagnostics;
pub mod error;
pub mod parser;
pub mod runner;
pub mod spec;
pub mod text;

pub use diagnostics::{severity_from_word, BuildDiagnostic, Severity};
pub use error::BuildError;
pub use parser::DiagnosticParser;
pub use runner::{BuildHandle, BuildOutcome, BuildSink};
pub use spec::{buildable_toolchain, BuildKind, BuildSpec};
