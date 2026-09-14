//! Turning a task/goal double-click into a run configuration (A6).
//!
//! `ponytail:` **temporary, to be replaced.** This is a minimal
//! `task -> RunConfig` mapping so B3 (the dock's double-click-to-run) has
//! something to call; it does not yet know about Maven profiles (`-P`), a
//! module-qualified goal (`:app:test` vs. a bare `test` run from the
//! module's own directory), or `run_core::macros` expansion in `cwd`. The
//! upgrade path is B3 itself, which owns the tool window this exists to
//! feed and can shape the request around what the UI actually offers
//! (profile checkboxes, an "Execute…" line edit).

use run_core::toolchain::{gradle_program, maven_program};
use run_core::RunConfig;

use crate::model::{BuildModel, Task, Tool};

/// Options a run picks up from the Build Tools dock's toolbar toggles.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub offline: bool,
    pub skip_tests: bool,
    pub extra_args: Vec<String>,
}

/// Build the temporary [`RunConfig`] a task/goal double-click launches
/// (ADR-0039's `remember_temporary`, the same cap and eviction rule every
/// other run-from-context entry point uses).
pub fn task_config(model: &BuildModel, task: &Task, opts: &RunOptions) -> RunConfig {
    let program = match model.tool {
        Tool::Gradle => gradle_program(&model.root),
        Tool::Maven => maven_program(&model.root),
    };

    let mut args = vec![task.path.clone()];
    match model.tool {
        Tool::Gradle => {
            if opts.offline {
                args.push("--offline".to_string());
            }
            if opts.skip_tests {
                args.push("-x".to_string());
                args.push("test".to_string());
            }
        }
        Tool::Maven => {
            if opts.offline {
                args.push("-o".to_string());
            }
            if opts.skip_tests {
                args.push("-DskipTests".to_string());
            }
        }
    }
    args.extend(opts.extra_args.iter().cloned());

    RunConfig {
        id: id_for_task(model, task),
        name: task.path.clone(),
        program,
        args,
        cwd: Some(model.root.display().to_string()),
        toolchain: Some(model.tool.toolchain_id().to_string()),
        target: Some(task.path.clone()),
        temporary: true,
        ..RunConfig::default()
    }
}

/// Stable across repeated double-clicks of the same task, so
/// `run_core::context::remember_temporary` reuses one entry instead of
/// growing a new one every time.
fn id_for_task(model: &BuildModel, task: &Task) -> String {
    format!(
        "jvm-build-tools:{}:{}",
        model.tool.toolchain_id(),
        task.path
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::*;

    fn model(tool: Tool) -> BuildModel {
        BuildModel {
            tool,
            root: PathBuf::from("/proj"),
            modules: vec![],
            tasks: vec![],
            warnings: vec![],
            synced_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn task(path: &str) -> Task {
        Task {
            path: path.to_string(),
            name: path.trim_start_matches(':').to_string(),
            group: String::new(),
            description: String::new(),
        }
    }

    #[test]
    fn a_gradle_task_runs_through_the_wrapper_with_offline_and_skip_tests() {
        let config = task_config(
            &model(Tool::Gradle),
            &task(":app:build"),
            &RunOptions {
                offline: true,
                skip_tests: true,
                extra_args: vec![],
            },
        );
        assert_eq!(config.program, "gradle"); // no gradlew in the fixture root
        assert_eq!(config.args, vec![":app:build", "--offline", "-x", "test"]);
        assert_eq!(config.toolchain.as_deref(), Some("gradle"));
        assert_eq!(config.target.as_deref(), Some(":app:build"));
        assert!(config.temporary);
    }

    #[test]
    fn a_maven_goal_uses_maven_specific_flags() {
        let config = task_config(
            &model(Tool::Maven),
            &task("test"),
            &RunOptions {
                offline: true,
                skip_tests: true,
                extra_args: vec!["-B".to_string()],
            },
        );
        assert_eq!(config.args, vec!["test", "-o", "-DskipTests", "-B"]);
        assert_eq!(config.toolchain.as_deref(), Some("maven"));
    }

    #[test]
    fn double_clicking_the_same_task_produces_the_same_id() {
        let m = model(Tool::Gradle);
        let t = task(":app:test");
        let a = task_config(&m, &t, &RunOptions::default());
        let b = task_config(&m, &t, &RunOptions::default());
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn different_tasks_get_different_ids() {
        let m = model(Tool::Gradle);
        let a = task_config(&m, &task(":app:test"), &RunOptions::default());
        let b = task_config(&m, &task(":app:build"), &RunOptions::default());
        assert_ne!(a.id, b.id);
    }
}
