//! Project scanning for launchable targets (F4-5): names only, no side
//! effects, no invoking `cargo`/`npm`/`make`.
//!
//! # Merge rule (F4-4's "don't overwrite a user-edited config")
//!
//! [`merge_detected`] lives here rather than in `settings-model`, even
//! though `settings-model` is where a resolution rule like this normally
//! belongs (see `settings_model::editing::resolve_for_language`). The rule
//! needs the `RunConfig` type detection produces, which is `run-core`'s;
//! putting the merge in `settings-model` would mean `settings-model`
//! depending on `run-core`, which `docs/architecture/layering.md` does not
//! list and which would put a settings-page crate one hop from a process
//! supervisor for one function. Detection and its merge are both "turn a
//! project scan into persistable settings", so they stay next to each other.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::{RunConfig, RunConfigExt};
use crate::toolchain::{self, ToolchainId};

/// A detected configuration, tagged with the toolchain that produced it and
/// the target within it (R1-2) so build, debug and the editor's per-kind
/// fields all know what they are looking at without re-parsing the name.
fn make_config(
    toolchain: ToolchainId,
    target: impl Into<String>,
    id: impl Into<String>,
    name: impl Into<String>,
    program: &str,
    args: Vec<String>,
) -> RunConfig {
    RunConfig {
        id: id.into(),
        name: name.into(),
        program: program.to_string(),
        args,
        toolchain: Some(toolchain.as_str().to_string()),
        target: Some(target.into()),
        ..RunConfig::default()
    }
}

fn make_config_in_project_dir(
    toolchain: ToolchainId,
    target: impl Into<String>,
    id: impl Into<String>,
    name: impl Into<String>,
    program: &str,
    args: Vec<String>,
) -> RunConfig {
    RunConfig {
        cwd: Some("$PROJECT_DIR".into()),
        ..make_config(toolchain, target, id, name, program, args)
    }
}

/// `[[bin]]` targets and every file (or `<name>/main.rs` directory) under
/// `examples/` — cargo's own implicit example-target rule.
fn detect_cargo(project_root: &Path) -> Vec<RunConfig> {
    let Ok(text) = fs::read_to_string(project_root.join("Cargo.toml")) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };

    let mut configs = Vec::new();
    if let Some(bins) = value.get("bin").and_then(|b| b.as_array()) {
        for bin in bins {
            if let Some(name) = bin.get("name").and_then(|n| n.as_str()) {
                configs.push(make_config(
                    ToolchainId::Cargo,
                    name,
                    format!("cargo-bin-{name}"),
                    name,
                    "cargo",
                    vec!["run".into(), "--bin".into(), name.into()],
                ));
            }
        }
    }

    let examples_dir = project_root.join("examples");
    if let Ok(entries) = fs::read_dir(&examples_dir) {
        let mut names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            } else if path.is_dir() && path.join("main.rs").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        for name in names {
            configs.push(make_config(
                ToolchainId::Cargo,
                name.clone(),
                format!("cargo-example-{name}"),
                name.clone(),
                "cargo",
                vec!["run".into(), "--example".into(), name],
            ));
        }
    }
    configs
}

/// Every key under `"scripts"` in `package.json`. The runner is `yarn` when
/// a `yarn.lock` sits next to it, `pnpm` when a `pnpm-lock.yaml` does,
/// otherwise `npm` — the same lockfile-presence heuristic every JS tooling
/// author-detection script uses, since `package.json` itself names no
/// package manager.
fn detect_package_json(project_root: &Path) -> Vec<RunConfig> {
    let Ok(text) = fs::read_to_string(project_root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|s| s.as_object()) else {
        return Vec::new();
    };

    let runner = toolchain::package_manager(project_root);

    let mut names: Vec<&String> = scripts.keys().collect();
    names.sort();
    names
        .into_iter()
        .map(|script| {
            make_config(
                ToolchainId::Npm,
                script.clone(),
                format!("{runner}-{script}"),
                script.clone(),
                runner,
                vec!["run".into(), script.clone()],
            )
        })
        .collect()
}

/// Phony targets in `Makefile`/`makefile`: names listed in a `.PHONY:` line
/// are trusted outright; in its absence, a target line (`name:` not
/// containing `%`, `$`, `/`, whitespace, or a leading `.`, and not a `:=`
/// assignment) with no `.` in its name is treated as phony — a real
/// file target overwhelmingly has an extension (`app.o:`, `build/main:`)
/// while a phony one overwhelmingly does not (`build:`, `test:`, `clean:`).
/// A recipe line (indented) is never a candidate.
fn detect_makefile(project_root: &Path) -> Vec<RunConfig> {
    let makefile = ["Makefile", "makefile"]
        .into_iter()
        .map(|name| project_root.join(name))
        .find(|path| path.is_file());
    let Some(makefile) = makefile else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&makefile) else {
        return Vec::new();
    };

    let mut phony: HashSet<String> = HashSet::new();
    let mut candidates: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue; // a recipe line, never a target declaration
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(".PHONY:") {
            phony.extend(rest.split_whitespace().map(str::to_string));
            continue;
        }
        let Some(colon_idx) = line.find(':') else {
            continue;
        };
        let after = &line[colon_idx + 1..];
        if after.starts_with('=') {
            continue; // `NAME := value` / `NAME ::= value`, not a rule
        }
        let name = line[..colon_idx].trim();
        if name.is_empty() || name.starts_with('.') || name.contains(['%', '$', '/', ' ']) {
            continue;
        }
        candidates.push(name.to_string());
    }

    let mut names: Vec<String> = Vec::new();
    for name in candidates {
        let looks_phony = phony.contains(&name) || !name.contains('.');
        if looks_phony && !names.contains(&name) {
            names.push(name);
        }
    }

    names
        .into_iter()
        .map(|name| {
            make_config(
                ToolchainId::Make,
                name.clone(),
                format!("make-{name}"),
                name.clone(),
                "make",
                vec![name],
            )
        })
        .collect()
}

/// `add_executable(<name> ...)` targets in the top-level `CMakeLists.txt`.
///
/// The binary lands wherever the generator puts it, which we cannot know
/// without configuring the project, so the configuration runs `build/<name>`
/// relative to the project root — CMake's own documented convention for an
/// out-of-source build directory, and the same path
/// [`ToolchainId::build_command`] builds into.
fn detect_cmake(project_root: &Path) -> Vec<RunConfig> {
    let Ok(text) = fs::read_to_string(project_root.join("CMakeLists.txt")) else {
        return Vec::new();
    };

    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("add_executable(") else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        let name = name.trim_end_matches(')');
        // `add_executable(${PROJECT_NAME} ...)` names a variable we would
        // have to evaluate CMake to resolve; skip rather than guess.
        if name.is_empty() || name.contains('$') || names.iter().any(|n| n == name) {
            continue;
        }
        names.push(name.to_string());
    }

    names
        .into_iter()
        .map(|name| {
            make_config_in_project_dir(
                ToolchainId::Cmake,
                name.clone(),
                format!("cmake-{name}"),
                name.clone(),
                &format!("build/{name}"),
                Vec::new(),
            )
        })
        .collect()
}

/// The conventional entry points of a Python project, in the order a reader
/// would try them. Only the project root is scanned: a deeper search would
/// have to guess which of several `main.py` files is the one meant.
fn detect_python(project_root: &Path) -> Vec<RunConfig> {
    if !ToolchainId::Python.is_present(project_root) {
        return Vec::new();
    }
    ["main.py", "app.py", "manage.py", "__main__.py"]
        .into_iter()
        .filter(|name| project_root.join(name).is_file())
        .map(|name| {
            make_config_in_project_dir(
                ToolchainId::Python,
                name,
                format!("python-{name}"),
                name,
                toolchain::python_program(project_root),
                vec![name.to_string()],
            )
        })
        .collect()
}

/// One configuration per JVM build tool present. Neither `mvn exec:java` nor
/// `gradle run` can be verified without invoking the tool — both need a
/// plugin the project may not apply — but they are what a JVM project is run
/// with when it is runnable at all, and an unusable default a user edits
/// beats no entry to edit.
fn detect_jvm(project_root: &Path) -> Vec<RunConfig> {
    let mut configs = Vec::new();
    for (toolchain, id, name, args) in [
        (
            ToolchainId::Maven,
            "maven-exec",
            "mvn exec:java",
            vec!["exec:java".to_string()],
        ),
        (
            ToolchainId::Gradle,
            "gradle-run",
            "gradle run",
            vec!["run".to_string()],
        ),
    ] {
        if !toolchain.is_present(project_root) {
            continue;
        }
        // The build command's program is the wrapper-aware one; reuse it
        // rather than re-deriving `./gradlew` vs `gradle` here.
        let Some(command) = toolchain.build_command(project_root) else {
            continue;
        };
        configs.push(make_config_in_project_dir(
            toolchain,
            args[0].clone(),
            id,
            name,
            &command.program,
            args.clone(),
        ));
    }
    configs
}

/// Every launchable target this project's build files name. Order follows
/// [`ToolchainId::ALL`] — Cargo bins and examples, CMake executables, Python
/// entry points, JVM run tasks, npm/yarn/pnpm scripts, Makefile targets —
/// stable so a re-scan producing the same targets produces the same list.
pub fn detect(project_root: &Path) -> Vec<RunConfig> {
    let mut configs = detect_cargo(project_root);
    configs.extend(detect_cmake(project_root));
    configs.extend(detect_python(project_root));
    configs.extend(detect_jvm(project_root));
    configs.extend(detect_package_json(project_root));
    configs.extend(detect_makefile(project_root));

    // A detected configuration whose run command does not compile first
    // gets a Build before-launch task, which is what makes Run on a CMake
    // or Maven project run the code the user just wrote (B2-1). A toolchain
    // whose run already builds gets none: two compiles per run is worse
    // than the convenience is worth.
    for config in &mut configs {
        let toolchain = config.toolchain();
        let needs_build = toolchain
            .map(|t| t.build_command(project_root).is_some() && !t.builds_before_running())
            .unwrap_or(false);
        config.before_launch = crate::before_launch::default_tasks(config, needs_build)
            .iter()
            .map(crate::before_launch::BeforeLaunchTask::to_setting)
            .collect();
    }
    configs
}

/// Add newly detected configurations to `existing` without touching any
/// entry already there — a config (auto-detected earlier, or hand-written)
/// stays exactly as it is, matched by name. Only a detected config whose
/// name has no match in `existing` is appended.
///
/// This is deliberately conservative rather than tracking provenance: a
/// renamed binary produces an "orphaned" entry under its old name rather
/// than resurrecting or overwriting anything, which is the failure mode the
/// plan calls out ("my settings got wiped") staying impossible by
/// construction.
pub fn merge_detected(existing: &[RunConfig], detected: Vec<RunConfig>) -> Vec<RunConfig> {
    let mut merged = existing.to_vec();
    let existing_names: HashSet<&str> = existing.iter().map(|c| c.name.as_str()).collect();
    for config in detected {
        if !existing_names.contains(config.name.as_str()) {
            merged.push(config);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/detect")
            .join(name)
    }

    #[test]
    fn cargo_bins_and_examples_are_detected() {
        let configs = detect_cargo(&fixture("cargo_project"));
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        // Bins in declaration order, then examples (file and `<name>/main.rs`
        // directory forms both) sorted.
        assert_eq!(names, vec!["main", "tool", "demo", "demo_dir"]);
        assert_eq!(configs[0].args, vec!["run", "--bin", "main"]);
        assert_eq!(configs[2].args, vec!["run", "--example", "demo"]);
        assert_eq!(configs[3].args, vec!["run", "--example", "demo_dir"]);
    }

    #[test]
    fn a_cargo_toml_with_no_bin_or_examples_yields_nothing() {
        let configs = detect_cargo(&fixture("cargo_project_empty"));
        assert!(configs.is_empty());
    }

    #[test]
    fn npm_scripts_are_detected_in_sorted_order() {
        let configs = detect_package_json(&fixture("npm_project"));
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["build", "start", "test"]);
        assert_eq!(configs[0].program, "npm");
        assert_eq!(configs[0].args, vec!["run", "build"]);
    }

    #[test]
    fn yarn_lockfile_selects_the_yarn_runner() {
        let configs = detect_package_json(&fixture("yarn_project"));
        assert_eq!(configs[0].program, "yarn");
    }

    #[test]
    fn pnpm_lockfile_selects_the_pnpm_runner() {
        let configs = detect_package_json(&fixture("pnpm_project"));
        assert_eq!(configs[0].program, "pnpm");
    }

    #[test]
    fn makefile_phony_targets_are_detected_and_pattern_rules_are_not() {
        let configs = detect_makefile(&fixture("make_project"));
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"build"));
        assert!(names.contains(&"test"));
        assert!(names.contains(&"clean"));
        assert!(!names.contains(&"%.o"), "{names:?}");
        assert!(!names.contains(&"app.o"), "{names:?}");
        assert!(!names.contains(&"CFLAGS"), "{names:?}");
    }

    fn project_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        dir
    }

    #[test]
    fn cmake_executables_are_detected_and_run_out_of_the_build_dir() {
        let dir = project_with(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\n\
             add_executable(app main.cpp)\n\
             # add_executable(commented out.cpp)\n\
             add_executable(tool tool.cpp)\n",
        )]);
        let configs = detect_cmake(dir.path());
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["app", "tool"]);
        assert_eq!(configs[0].program, "build/app");
        assert_eq!(configs[0].cwd.as_deref(), Some("$PROJECT_DIR"));
    }

    #[test]
    fn a_cmake_target_named_by_a_variable_is_skipped_rather_than_guessed() {
        let dir = project_with(&[("CMakeLists.txt", "add_executable(${PROJECT_NAME} m.cpp)\n")]);
        assert!(detect_cmake(dir.path()).is_empty());
    }

    #[test]
    fn python_entry_points_are_detected_only_beside_a_python_marker() {
        let without_marker = project_with(&[("main.py", "")]);
        assert!(detect_python(without_marker.path()).is_empty());

        let dir = project_with(&[("pyproject.toml", ""), ("main.py", ""), ("app.py", "")]);
        let configs = detect_python(dir.path());
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["main.py", "app.py"]);
        assert_eq!(configs[0].args, vec!["main.py"]);
    }

    #[test]
    fn jvm_projects_get_one_run_task_each_through_the_wrapper() {
        let dir = project_with(&[("build.gradle", ""), ("gradlew", "")]);
        let configs = detect_jvm(dir.path());
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].program, "./gradlew");
        assert_eq!(configs[0].args, vec!["run"]);
    }

    #[test]
    fn a_cmake_target_is_built_before_it_runs_and_a_cargo_bin_is_not() {
        let dir = project_with(&[("CMakeLists.txt", "add_executable(app main.cpp)\n")]);
        let cmake = detect(dir.path());
        assert_eq!(
            cmake[0].before_launch.len(),
            1,
            "running `build/app` has to build it first"
        );

        let cargo = detect(&fixture("cargo_project"));
        assert!(
            cargo[0].before_launch.is_empty(),
            "`cargo run` already compiles; a Build task would compile twice"
        );
    }

    #[test]
    fn a_workspace_with_no_build_files_yields_no_configs_and_no_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect(dir.path()).is_empty());
    }

    #[test]
    fn auto_detected_configs_do_not_overwrite_a_user_edited_config_of_the_same_name() {
        let existing = vec![RunConfig {
            id: "cargo-bin-main".into(),
            name: "main".into(),
            program: "cargo".into(),
            args: vec![
                "run".into(),
                "--bin".into(),
                "main".into(),
                "--release".into(),
            ],
            env: vec![("EDITED".into(), "yes".into())],
            ..RunConfig::default()
        }];
        let detected = vec![make_config(
            ToolchainId::Cargo,
            "main",
            "cargo-bin-main",
            "main",
            "cargo",
            vec!["run".into(), "--bin".into(), "main".into()],
        )];
        let merged = merge_detected(&existing, detected);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].env,
            vec![("EDITED".to_string(), "yes".to_string())]
        );
    }

    #[test]
    fn merge_adds_new_names_and_leaves_others_untouched() {
        let existing = vec![make_config(
            ToolchainId::Cargo,
            "a",
            "a",
            "a",
            "cargo",
            vec![],
        )];
        let detected = vec![make_config(
            ToolchainId::Cargo,
            "b",
            "b",
            "b",
            "cargo",
            vec![],
        )];
        let merged = merge_detected(&existing, detected);
        let names: Vec<&str> = merged.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
