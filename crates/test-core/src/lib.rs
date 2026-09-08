//! The PHPUnit test tool window's Qt-free half (the PHP tooling plan's D
//! phase): a test run's live tree, the two output-format parsers that fill
//! it, and converting a failure into a diagnostic.
//!
//! Qt-free and tokio-free by design (`docs/architecture/layering.md`'s
//! `test-core` row): no `#[qobject]`, no `async fn`, long work runs on a
//! `std::thread` via [`runner::run`] and reports back through an ordinary
//! `Send` trait object, exactly as `analysis-core` and `build-core` do —
//! which is what lets `ui-shell` (D4) forward a result through
//! `CxxQtThread::queue()` without this crate ever knowing that type
//! exists.
//!
//! Not `analysis-core`: a test run is a live per-node tree with rerun
//! selectors, a shape neither an analyzer's flat finding list nor a
//! build's flat diagnostic list has (see the plan and ADR-0048).
//!
//! ## Modules
//!
//! - [`tree`] — [`tree::TestTree`], [`tree::TestId`], [`tree::TestStatus`]:
//!   the suite/class/method hierarchy, filled incrementally.
//! - [`teamcity`] — the primary streaming format, PHPUnit's `--teamcity`.
//! - [`junit`] — the batch fallback format, `--log-junit`.
//! - [`runner`] — spawning the process and turning its output into tree
//!   events, over pipes rather than a PTY (same reasoning as analyzers).
//! - [`diagnostics`] — a failing test becomes a `diagnostics_core::
//!   Diagnostic` (D3), published into the one shared Problems model.

mod diagnostics;
mod junit;
mod runner;
mod teamcity;
mod tree;

pub use diagnostics::diagnostics_by_file;
pub use junit::{parse as parse_junit_xml, JUnitTestCase, ParseError as JUnitParseError};
pub use runner::{run, RunFailure, TestRunHandle, TestSink};
pub use teamcity::{TeamCityEvent, TeamCityParser};
pub use tree::{NodeKind, TestCounts, TestFailure, TestId, TestNode, TestStatus, TestTree};
