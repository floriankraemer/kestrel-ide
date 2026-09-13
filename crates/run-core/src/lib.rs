//! Run configurations and console (F4): the run configuration model, the
//! toolchain table, project detection of launchable targets, process
//! supervision, output batching for a run console, and `file:line` link
//! resolution in console output.
//!
//! [`toolchain`] is the repo's single source of truth for which build tool a
//! project uses; `build-core` and `dap-core` read it rather than detecting
//! again (R1-1).
//!
//! Qt-free by design (see `docs/architecture/layering.md`'s `run-core`
//! row) — `crates/ui-shell`'s future `RunService` is the only thing that
//! may call this crate from behind the FFI seam (ADR-0003).

pub mod ansi;
pub mod batching;
pub mod before_launch;
pub mod config;
/// Container-kind run configurations (C5, ADR-0056): `to_launch_spec_in`'s
/// dispatch for "container-image"/"containerfile"/"compose", the
/// containerfile auto-build before-launch task, and the compose console's
/// Stop/Down commands.
pub mod container_run;
pub mod context;
pub mod detect;
pub mod error;
pub mod links;
pub mod macros;
pub mod supervisor;
pub mod toolchain;

pub use ansi::{AnsiResolver, AnsiStripper, StyledRun, StyledText, TextStyle};
pub use batching::{BatchedOutput, OutputBatcher};
pub use before_launch::{BeforeLaunchError, BeforeLaunchTask};
pub use config::{ConsoleKind, LaunchSpec, RunConfig, RunConfigExt};
pub use container_run::{down_command, stop_command};
pub use context::{config_for_file, remember_temporary, TEMPORARY_CAP};
pub use detect::{detect, merge_detected};
pub use error::RunError;
pub use links::{resolve_link, ResolvedLink};
pub use macros::{expand as expand_macros, MacroContext};
/// How long a stopped process is given to exit on its own before it is
/// killed (R2-4).
///
/// Long enough that a program with a signal handler can flush its output
/// and remove its pid file, short enough that "Stop" still feels like it
/// stopped something. IntelliJ's own Exit/Kill pair works the same way,
/// and the escalation is what makes the soft signal safe to send first:
/// nothing survives a Stop by ignoring it, it only survives for two
/// seconds.
pub const TERMINATION_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

pub use supervisor::{ConsoleId, Supervisor};
pub use toolchain::{detect_toolchains, ToolCommand, ToolchainId};
