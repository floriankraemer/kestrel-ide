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
    /// Maven only: profile ids to activate, each becoming its own `-P<id>`
    /// (B3) — Maven takes one `-P` per profile, not a comma-joined list, so
    /// this is a `Vec` rather than the single flag `offline`/`skip_tests`
    /// are.
    pub profiles: Vec<String>,
    pub extra_args: Vec<String>,
}

/// Split the Build Tools dock's "Execute…" line edit into argv (B3) —
/// shell-style whitespace splitting with single/double-quote grouping, so
/// `test --tests "com.example.FooTest"` keeps its quoted argument whole
/// rather than splitting on the space inside it. No escape-character
/// support beyond the quote itself: a task/goal line has no use for one,
/// and a naive backslash rule would be one more thing to get wrong for
/// zero real benefit here.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_token = false;

    for ch in text.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                in_token = true;
            }
            None if ch.is_whitespace() => {
                if in_token {
                    args.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            None => {
                current.push(ch);
                in_token = true;
            }
        }
    }
    if in_token {
        args.push(current);
    }
    args
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
            for profile in &opts.profiles {
                args.push(format!("-P{profile}"));
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
            root_name: "proj".to_string(),
            modules: vec![],
            tasks: vec![],
            warnings: vec![],
            profiles: vec![],
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
                ..RunOptions::default()
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
                ..RunOptions::default()
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
    fn a_maven_goal_activates_one_flag_per_profile() {
        let config = task_config(
            &model(Tool::Maven),
            &task("test"),
            &RunOptions {
                profiles: vec!["ci".to_string(), "release".to_string()],
                ..RunOptions::default()
            },
        );
        assert_eq!(config.args, vec!["test", "-Pci", "-Prelease"]);
    }

    #[test]
    fn split_args_separates_on_plain_whitespace() {
        assert_eq!(
            split_args("--tests com.example.FooTest"),
            vec!["--tests", "com.example.FooTest"]
        );
    }

    #[test]
    fn split_args_keeps_a_quoted_argument_whole() {
        assert_eq!(
            split_args(r#"--tests "com.example.FooTest""#),
            vec!["--tests", "com.example.FooTest"]
        );
        assert_eq!(split_args("-Dtest='Foo Bar'"), vec!["-Dtest=Foo Bar"]);
    }

    #[test]
    fn split_args_of_an_empty_line_is_empty() {
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn different_tasks_get_different_ids() {
        let m = model(Tool::Gradle);
        let a = task_config(&m, &task(":app:test"), &RunOptions::default());
        let b = task_config(&m, &task(":app:build"), &RunOptions::default());
        assert_ne!(a.id, b.id);
    }
}
