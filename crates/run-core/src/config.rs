//! The run configuration model, and the launch spec it produces (F4-3).

use std::path::{Path, PathBuf};

use crate::macros::{self, MacroContext};
use crate::toolchain::ToolchainId;

/// A project-defined launch target.
///
/// This is exactly `app_config::RunConfigSetting` — the persisted shape —
/// rather than a second struct kept in sync with a manual mapping (see
/// `run-core`'s `Cargo.toml` and `app_config::RunConfigSetting`'s doc
/// comment). Everything this module adds lives on [`RunConfigExt`], since an
/// inherent `impl` is not available on a type this crate does not own.
pub type RunConfig = app_config::RunConfigSetting;

/// Where a launch's output goes. The shared, debugger-agnostic half of what
/// would eventually become a DAP `launch` request body — see
/// [`LaunchSpec`]'s doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleKind {
    /// Attached to a PTY (today's only consumer: `Supervisor`). Programs
    /// buffer and colour their output differently when stdout is a tty, so
    /// this is what makes a run console look like running the command by
    /// hand.
    Pty,
    /// Plain stdout/stderr pipes. Not used yet — reserved for a future
    /// `dap-core`, which talks to a debug adapter rather than a shell.
    Pipes,
}

/// What it takes to launch a process, independent of *why* it is being
/// launched. `RunConfig::to_launch_spec` produces one for a run; a future
/// `dap-core` would turn the same shape into a DAP `launch` request body.
/// Deliberately minimal and debugger-agnostic — no breakpoints, no
/// attach-vs-launch distinction, nothing DAP-specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub console: ConsoleKind,
    /// Set only when this launch was wrapped to run inside a container run
    /// target (C8): the local project root <-> the container's mount root,
    /// so a console's file links and a build's diagnostic paths resolve a
    /// path the wrapped program reports (e.g. `/workspace/src/x.rs:12`)
    /// back to the project file it actually names. `None` for every other
    /// launch — the same "absent means nothing to translate" shape
    /// `process_exec::host::ExecHost::Local` gives path translation
    /// (ADR-0052).
    pub path_map: Option<container_core::target::PathMap>,
}

/// Methods on [`RunConfig`] — an extension trait rather than an inherent
/// `impl` because `RunConfig` is a type alias for `app_config`'s struct.
pub trait RunConfigExt {
    /// Turn this configuration into what it takes to launch it, expanding
    /// every macro (see [`crate::macros`]) in `cwd`, `args` and env values
    /// against `project_root`.
    fn to_launch_spec(&self, project_root: &Path) -> LaunchSpec;

    /// The same, with the file a run-from-context launch started from, so
    /// `$FILE_PATH$` and its siblings resolve.
    fn to_launch_spec_in(&self, context: &MacroContext) -> LaunchSpec;

    /// The build tool this configuration belongs to, or `None` for a
    /// hand-written one or an identifier this version does not know.
    fn toolchain(&self) -> Option<ToolchainId>;
}

impl RunConfigExt for RunConfig {
    fn to_launch_spec(&self, project_root: &Path) -> LaunchSpec {
        self.to_launch_spec_in(&MacroContext::for_project(project_root))
    }

    fn to_launch_spec_in(&self, context: &MacroContext) -> LaunchSpec {
        // A container-kind configuration compiles to an entirely different
        // shape (`docker`/`podman`/`compose` argv through
        // `container_core::run_config`, not this configuration's own
        // `program`/`args`) — see `crate::container_run`. An unrecognised
        // or absent `kind` is a plain process, exactly what this method
        // always did before C5 (ADR-0056's "unknown reads as the default"
        // rule, same as `toolchain`).
        let containers = context.containers.clone().unwrap_or_default();
        // `Some("sql-script")` is spelled out on its own rather than
        // folded into `_` below: a `sql-script` configuration has no
        // process launch of its own at all (ADR-0056 shape) — it runs a
        // `.sql` file against a data source, not a program.
        // `RunService::launch` (`ui-shell::bridge::run::sql_script`)
        // recognises this kind and never calls `to_launch_spec`/
        // `to_launch_spec_in` for it; reaching `process_launch_spec`'s
        // fallback below with this kind's empty `program` is inert, not a
        // crash, if some future caller ever forgets that.
        #[allow(clippy::match_same_arms)]
        match self.kind.as_deref() {
            Some("container-image") => {
                crate::container_run::image_launch_spec(self, context, &containers)
            }
            Some("containerfile") => {
                crate::container_run::containerfile_launch_spec(self, context, &containers)
            }
            Some("compose") => {
                crate::container_run::compose_launch_spec(self, context, &containers)
            }
            Some("sql-script") => None,
            _ => None,
        }
        .unwrap_or_else(|| {
            let spec = process_launch_spec(self, context);
            // Run targets (C8) only apply to a plain process configuration
            // — a container-kind one's launch already *is* a container
            // launch (`RunConfigSetting::run_on`'s own doc comment). An
            // unresolvable target (blank/unknown id, or a `cwd` outside the
            // project) falls back to the unwrapped local spec here — the
            // same "unknown reads as the least surprising default" rule
            // `toolchain` and `kind` themselves follow; `RunService::launch`
            // is where the interactive path checks this ahead of time and
            // reports it instead of silently running locally (see
            // `crate::container_target::validate_run_on`).
            self.run_on
                .as_deref()
                .and_then(|run_on| crate::container_target::target_id(run_on))
                .and_then(|target_id| {
                    crate::container_target::wrap_process_spec(
                        &spec,
                        target_id,
                        context,
                        &containers,
                    )
                    .ok()
                    .flatten()
                })
                .unwrap_or(spec)
        })
    }

    fn toolchain(&self) -> Option<ToolchainId> {
        self.toolchain.as_deref().and_then(ToolchainId::from_id)
    }
}

/// The plain process launch every configuration compiled to before C5, and
/// what one with no (or an unrecognised) `kind` still compiles to.
fn process_launch_spec(config: &RunConfig, context: &MacroContext) -> LaunchSpec {
    LaunchSpec {
        program: config.program.clone(),
        args: config
            .args
            .iter()
            .map(|arg| macros::expand(arg, context))
            .collect(),
        cwd: config
            .cwd
            .as_deref()
            .map(|cwd| PathBuf::from(macros::expand(cwd, context))),
        env: config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), macros::expand(v, context)))
            .collect(),
        console: ConsoleKind::Pty,
        path_map: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RunConfig {
        RunConfig {
            id: "run-1".into(),
            name: "cargo run".into(),
            program: "cargo".into(),
            args: vec!["run".into()],
            ..RunConfig::default()
        }
    }

    #[test]
    fn project_dir_expands_in_cwd() {
        let cfg = RunConfig {
            cwd: Some("$PROJECT_DIR/subdir".into()),
            ..config()
        };
        let spec = cfg.to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(spec.cwd, Some(PathBuf::from("/home/me/project/subdir")));
    }

    #[test]
    fn project_dir_expands_in_env_values() {
        let cfg = RunConfig {
            env: vec![("DATA_DIR".into(), "$PROJECT_DIR/data".into())],
            ..config()
        };
        let spec = cfg.to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(
            spec.env,
            vec![("DATA_DIR".to_string(), "/home/me/project/data".to_string())]
        );
    }

    #[test]
    fn no_token_means_no_change() {
        let cfg = RunConfig {
            cwd: Some("/absolute/path".into()),
            ..config()
        };
        let spec = cfg.to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(spec.cwd, Some(PathBuf::from("/absolute/path")));
    }

    #[test]
    fn absent_cwd_stays_absent() {
        let spec = config().to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(spec.cwd, None);
    }

    #[test]
    fn a_sql_script_kind_falls_back_to_an_inert_empty_program() {
        // `RunService::launch` never actually calls this for a
        // `sql-script` configuration (this module's own doc comment) —
        // this only proves the fallback stays harmless if some future
        // caller forgets that.
        let cfg = RunConfig {
            program: String::new(),
            kind: Some("sql-script".to_string()),
            ..config()
        };
        let spec = cfg.to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(spec.program, "");
    }

    #[test]
    fn renaming_does_not_touch_the_id() {
        let mut cfg = config();
        let id_before = cfg.id.clone();
        cfg.name = "a whole new name".into();
        cfg.program = "make".into();
        assert_eq!(cfg.id, id_before);
    }

    #[test]
    fn macros_expand_in_arguments_too() {
        let cfg = RunConfig {
            args: vec!["--manifest-path".into(), "$PROJECT_DIR$/Cargo.toml".into()],
            ..config()
        };
        let spec = cfg.to_launch_spec(Path::new("/home/me/project"));
        assert_eq!(spec.args[1], "/home/me/project/Cargo.toml");
    }

    #[test]
    fn file_macros_resolve_only_with_a_context_file() {
        let cfg = RunConfig {
            args: vec!["$FILE_PATH$".into()],
            ..config()
        };
        let from_toolbar = cfg.to_launch_spec(Path::new("/p"));
        assert_eq!(from_toolbar.args, vec!["$FILE_PATH$"]);

        let from_context = cfg.to_launch_spec_in(&MacroContext::for_file("/p", "/p/src/main.rs"));
        assert_eq!(from_context.args, vec!["/p/src/main.rs"]);
    }

    #[test]
    fn a_known_toolchain_id_resolves_and_an_unknown_one_is_none() {
        let cfg = RunConfig {
            toolchain: Some("cargo".into()),
            ..config()
        };
        assert_eq!(cfg.toolchain(), Some(ToolchainId::Cargo));

        let cfg = RunConfig {
            toolchain: Some("bazel".into()),
            ..config()
        };
        assert_eq!(cfg.toolchain(), None);
        assert_eq!(config().toolchain(), None);
    }

    #[test]
    fn a_configuration_is_neither_temporary_nor_parallel_by_default() {
        assert!(!config().temporary);
        assert!(!config().allow_parallel);
    }

    #[test]
    fn console_kind_defaults_to_pty() {
        let spec = config().to_launch_spec(Path::new("/p"));
        assert_eq!(spec.console, ConsoleKind::Pty);
    }
}
