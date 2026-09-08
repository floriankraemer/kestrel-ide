//! Analyzer definitions, detection, scheduling, and output parsing (the
//! PHP tooling plan's B phase).
//!
//! Qt-free by design (`docs/architecture/layering.md`'s `analysis-core`
//! row): this crate knows nothing about Qt or a QObject, and takes no
//! tokio — long work runs on a `std::thread` and reports back through an
//! ordinary `Send` closure, which is what lets `ui-shell` (B8) forward a
//! result through `CxxQtThread::queue()` without this crate ever knowing
//! that type exists.
//!
//! Not `build-core`: a build addresses a whole *project* on demand; an
//! analyzer addresses one *file* on a keystroke, with the opposite
//! invocation and cancellation shape (B5).

mod buffer;
mod checkstyle;
mod def;
mod detect;
mod scheduler;

pub use buffer::{
    degradation_reason, effective_trigger, write_temp_copy, BufferStrategy, TempCopyGuard,
    TEMP_COPY_GITIGNORE_PATTERN,
};
pub use checkstyle::{
    parse as parse_checkstyle_xml, to_diagnostics, CheckstyleFinding, ParseError,
};
pub use def::{AnalyzerDef, Trigger};
pub use detect::{composer_require_dev, find_config_file, find_program, status, AnalyzerStatus};
pub use scheduler::{RunFailure, RunOutput, RunResult, Scheduler};
