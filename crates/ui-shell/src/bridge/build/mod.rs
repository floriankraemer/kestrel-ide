//! Building the project (B1-6): `BuildService`, the QObject that runs a
//! build and publishes what it said.
//!
//! # Threading
//!
//! One thread per build, not one worker for all of them — a build is a
//! single long process read to completion, so there is no queue of short
//! operations to serialize the way `RunService` has. Running the steps and
//! reading them is `build_core::runner`'s, which blocks on whatever thread
//! it is given; this adapter gives it one and turns its callbacks into
//! signals. The before-launch path (B2) runs the same function on the run
//! worker's thread, which is why it lives there and not here.
//!
//! Translation only, per `docs/architecture/layering.md`: which steps a
//! build runs and what its output means are `build-core`'s (ADR-0040).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;

use build_core::{BuildDiagnostic, BuildKind, BuildSpec};
use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

/// Rust side of the `BuildService` QObject.
#[derive(Default)]
pub struct BuildServiceRust {
    next_id: Cell<u64>,
    builds: RefCell<HashMap<u64, build_core::BuildHandle>>,
    /// Everything the last build said, in the order it said it. Replaced
    /// wholesale when a new build starts: a diagnostic from a build two
    /// edits ago is worse than no diagnostic, because it looks current.
    /// Kept as a running accumulator (not exposed to the view — see
    /// `DiagnosticsService`) so a republish after a later chunk can regroup
    /// everything the build has said so far by file, per `republish`.
    diagnostics: RefCell<Vec<BuildDiagnostic>>,
    /// Which tool produced the diagnostics currently held — the `source`
    /// column the Problems dock already shows for a language server's, and
    /// half of this build's key into the shared store (see `source_key`).
    source: RefCell<String>,
    /// The one Problems model (ADR-0046), shared with `LanguageService`,
    /// `DiagnosticsService` and `AiChat`. Every row this adapter publishes
    /// is keyed under [`source_key`], so a build's diagnostics for a file
    /// never clobber (or get clobbered by) a language server's for the
    /// same file.
    store: SharedDiagnostics,
}

fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

fn no_project() -> ffi::FfiResult {
    ffi::FfiResult {
        code: errors::CODE_NO_PROJECT,
        message: QString::from("no project is open"),
    }
}

fn to_ffi_result(err: &build_core::BuildError) -> ffi::FfiResult {
    ffi::FfiResult {
        code: err.code(),
        message: QString::from(err.to_string().as_str()),
    }
}

/// The shared diagnostics store's key for one toolchain's rows (ADR-0046):
/// distinct from a language server's (`language::lsp_source_key`) so a
/// build's rows for a file and a server's for the same file coexist.
fn source_key(toolchain: &str) -> String {
    format!("build:{toolchain}")
}

/// A build diagnostic converted to the shared model. A build reports where
/// a problem starts, 1-based, and never where it ends; `diagnostics-core`
/// counts from 0, so the position is shifted rather than carried through
/// unconverted — the bug this phase's regression test exists to catch, per
/// `docs/architecture/php-tooling-plan.md`'s phase A.
fn to_diagnostic_core(diagnostic: &BuildDiagnostic, source: &str) -> diagnostics_core::Diagnostic {
    diagnostics_core::Diagnostic {
        range: diagnostics_core::Range {
            start: diagnostics_core::Position {
                line: diagnostic.line.saturating_sub(1),
                character: diagnostic.column.saturating_sub(1),
            },
            // A build never reports where a problem ends; `DiagnosticStore`
            // treats a missing end as a point at `start`, exactly as it
            // already did for a build diagnostic's `end_line`/`end_column`.
            end: None,
        },
        severity: diagnostic.severity,
        message: diagnostic.message.clone(),
        source: source.to_string(),
        raw: None,
    }
}

/// Regroup everything this build has said so far by file and publish it
/// under this build's source key. Called after every chunk of diagnostics,
/// not just at the end, so the Problems dock and the editor's squiggles
/// fill in while the build is still running (ADR-0040 §5) — and a full
/// regroup rather than an incremental one because a later chunk can add a
/// diagnostic to a file an earlier chunk already reported on.
fn republish(service: &ffi::BuildService) {
    let source = service.source.borrow().clone();
    let key = source_key(&source);
    let mut grouped: HashMap<String, Vec<diagnostics_core::Diagnostic>> = HashMap::new();
    for diagnostic in service.diagnostics.borrow().iter() {
        let uri = diagnostics_core::uri_from_path(&diagnostic.path.display().to_string());
        grouped
            .entry(uri)
            .or_default()
            .push(to_diagnostic_core(diagnostic, &source));
    }
    let mut store = service.store.borrow_mut();
    for (uri, diagnostics) in grouped {
        store.replace(&key, &uri, diagnostics);
    }
}

impl ffi::BuildService {
    pub fn build(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(BuildKind::Build)
    }

    pub fn rebuild(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(BuildKind::Rebuild)
    }

    pub fn build_target(self: Pin<&mut Self>, target: &QString) -> ffi::FfiResult {
        self.start(BuildKind::Target(target.to_string()))
    }

    fn start(mut self: Pin<&mut Self>, kind: BuildKind) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let toolchain = match build_core::buildable_toolchain(&root) {
            Ok(toolchain) => toolchain,
            Err(err) => return to_ffi_result(&err),
        };
        let steps = match BuildSpec::new(toolchain, kind, &root).steps() {
            Ok(steps) => steps,
            Err(err) => return to_ffi_result(&err),
        };

        // A new build's problems replace the last one's: a diagnostic from a
        // build two edits ago looks current and is not.
        self.diagnostics.borrow_mut().clear();
        *self.source.borrow_mut() = toolchain.as_str().to_string();
        self.store
            .borrow_mut()
            .clear_source(&source_key(toolchain.as_str()));

        let build_id = self.next_id.get() + 1;
        self.next_id.set(build_id);
        let handle = build_core::BuildHandle::new();
        self.builds.borrow_mut().insert(build_id, handle.clone());

        let qt_thread = self.as_mut().qt_thread();
        let command = QString::from(build_core::runner::command_line(&steps[0]).as_str());
        self.as_mut().build_started(build_id, command);
        self.as_mut().diagnostics_changed();

        let source = toolchain.as_str().to_string();
        std::thread::spawn(move || {
            let mut sink = QtSink {
                build_id,
                source,
                qt_thread: qt_thread.clone(),
            };
            let outcome = build_core::runner::run(&handle, &steps, toolchain, &root, &mut sink);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::BuildService>| {
                service.builds.borrow_mut().remove(&build_id);
                service.as_mut().build_finished(build_id, outcome.exit_code);
            });
        });
        ffi::FfiResult::default()
    }

    pub fn stop(self: Pin<&mut Self>, build_id: u64) {
        // Cloned out of the map before stopping: `BuildHandle::stop` takes a
        // lock the build's own thread also uses, and holding the `RefCell`
        // borrow across it would keep the map borrowed while that thread may
        // be queueing a removal onto this one.
        let handle = self.builds.borrow().get(&build_id).cloned();
        if let Some(handle) = handle {
            handle.stop();
        }
    }

    pub fn is_building(&self) -> bool {
        !self.builds.borrow().is_empty()
    }
}

/// The `BuildSink` that turns a running build's chunks into Qt signals.
/// Every callback is queued rather than called: this runs on the build's own
/// thread.
struct QtSink {
    build_id: u64,
    source: String,
    qt_thread: CxxQtThread<ffi::BuildService>,
}

impl build_core::BuildSink for QtSink {
    fn output(&mut self, text: &str) {
        let build_id = self.build_id;
        let text = text.to_string();
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::BuildService>| {
                service
                    .as_mut()
                    .build_output(build_id, QString::from(text.as_str()));
            });
    }

    fn diagnostics(&mut self, diagnostics: Vec<BuildDiagnostic>) {
        let source = self.source.clone();
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::BuildService>| {
                service.diagnostics.borrow_mut().extend(diagnostics);
                *service.source.borrow_mut() = source;
                republish(&service);
                service.as_mut().diagnostics_changed();
            });
    }
}
