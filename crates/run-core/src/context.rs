//! Running from context (R1-4): the configuration a file implies, and the
//! bounded list of temporary configurations those create.
//!
//! IntelliJ's gutter Run creates a configuration on the spot and keeps it
//! until it is either saved or pushed out by newer ones. The same two rules
//! live here: [`config_for_file`] decides what a file would be run as, and
//! [`remember_temporary`] keeps at most [`TEMPORARY_CAP`] of them so a week
//! of gutter clicks does not turn into a hundred entries in
//! `.ide/settings.toml`.
//!
//! Only the toolchains whose file-to-target mapping is unambiguous get an
//! answer. A CMake source file belongs to whichever `add_executable` lists
//! it, and a JVM class needs the module's classpath — both need the build
//! tool's own model, so they return `None` rather than a guess that runs the
//! wrong thing.

use std::path::{Component, Path};

use crate::config::RunConfig;
use crate::toolchain::{self, ToolchainId};

/// How many temporary configurations are kept. IntelliJ's default is five,
/// and the number only has to be small enough that the list stays readable.
pub const TEMPORARY_CAP: usize = 5;

/// The configuration that running `file` from the editor would launch, or
/// `None` when the file's toolchain has no unambiguous target for it.
///
/// The returned configuration is marked [`RunConfig::temporary`]; its `id` is
/// derived from the file so clicking the same gutter icon twice reuses one
/// entry instead of growing the list.
pub fn config_for_file(project_root: &Path, file: &Path) -> Option<RunConfig> {
    let relative = file.strip_prefix(project_root).ok()?;
    cargo_config(project_root, relative).or_else(|| python_config(project_root, file, relative))
}

fn cargo_config(project_root: &Path, relative: &Path) -> Option<RunConfig> {
    if !ToolchainId::Cargo.is_present(project_root) {
        return None;
    }
    if relative.extension().and_then(|e| e.to_str()) != Some("rs") {
        return None;
    }
    let segments: Vec<&str> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let stem = relative.file_stem().and_then(|s| s.to_str())?;

    // `examples/demo.rs` and `examples/demo/main.rs` are both cargo's
    // `--example demo`; `src/bin/tool.rs` and `src/bin/tool/main.rs` are both
    // `--bin tool`; `src/main.rs` is the package's default binary.
    let (target_flag, target) = match segments.as_slice() {
        ["examples", name] if *name == format!("{stem}.rs") => ("--example", stem.to_string()),
        ["examples", name, "main.rs"] => ("--example", (*name).to_string()),
        ["src", "bin", name] if *name == format!("{stem}.rs") => ("--bin", stem.to_string()),
        ["src", "bin", name, "main.rs"] => ("--bin", (*name).to_string()),
        ["src", "main.rs"] => {
            return Some(temporary(
                ToolchainId::Cargo,
                "run",
                "cargo-run",
                "cargo run",
                "cargo",
                vec!["run".into()],
            ))
        }
        _ => return None,
    };

    Some(temporary(
        ToolchainId::Cargo,
        target.clone(),
        format!("cargo-{}-{target}", target_flag.trim_start_matches("--")),
        target.clone(),
        "cargo",
        vec!["run".into(), target_flag.into(), target],
    ))
}

fn python_config(project_root: &Path, file: &Path, relative: &Path) -> Option<RunConfig> {
    if !ToolchainId::Python.is_present(project_root) {
        return None;
    }
    if file.extension().and_then(|e| e.to_str()) != Some("py") {
        return None;
    }
    let shown = relative.display().to_string();
    Some(temporary(
        ToolchainId::Python,
        shown.clone(),
        format!("python-{shown}"),
        shown.clone(),
        toolchain::python_program(project_root),
        vec![shown],
    ))
}

fn temporary(
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
        cwd: Some("$PROJECT_DIR".into()),
        toolchain: Some(toolchain.as_str().to_string()),
        target: Some(target.into()),
        temporary: true,
        ..RunConfig::default()
    }
}

/// Add `config` to `configs`, replacing an entry with the same id, and drop
/// the oldest temporary entries beyond [`TEMPORARY_CAP`].
///
/// Saved configurations are never evicted — only entries still marked
/// [`RunConfig::temporary`] are, which is what makes "save this temporary
/// configuration" mean "stop it being thrown away".
pub fn remember_temporary(configs: &mut Vec<RunConfig>, config: RunConfig) {
    // An entry for this target already exists — detection found it, or the
    // user saved it, or an earlier gutter click created it. Leave it alone:
    // replacing it would silently turn a saved configuration back into an
    // evictable one, and discard whatever the user edited into it.
    if configs.iter().any(|existing| existing.id == config.id) {
        return;
    }
    configs.push(config);

    let mut excess = configs.iter().filter(|c| c.temporary).count();
    configs.retain(|c| {
        if c.temporary && excess > TEMPORARY_CAP {
            excess -= 1;
            false
        } else {
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn cargo_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        dir
    }

    fn config_for(root: &Path, relative: &str) -> Option<RunConfig> {
        config_for_file(root, &root.join(relative))
    }

    #[test]
    fn src_main_runs_the_default_binary() {
        let dir = cargo_project();
        let config = config_for(dir.path(), "src/main.rs").unwrap();
        assert_eq!(config.args, vec!["run"]);
        assert!(config.temporary);
    }

    #[test]
    fn a_file_under_src_bin_runs_that_binary() {
        let dir = cargo_project();
        let config = config_for(dir.path(), "src/bin/tool.rs").unwrap();
        assert_eq!(config.args, vec!["run", "--bin", "tool"]);
        assert_eq!(config.name, "tool");
    }

    #[test]
    fn both_example_layouts_run_the_same_example() {
        let dir = cargo_project();
        let flat = config_for(dir.path(), "examples/demo.rs").unwrap();
        let nested = config_for(dir.path(), "examples/demo/main.rs").unwrap();
        assert_eq!(flat.args, vec!["run", "--example", "demo"]);
        assert_eq!(nested.args, flat.args);
        assert_eq!(nested.id, flat.id, "the same target means the same entry");
    }

    #[test]
    fn an_ordinary_module_has_no_run_target() {
        let dir = cargo_project();
        assert!(config_for(dir.path(), "src/parser.rs").is_none());
    }

    #[test]
    fn a_file_outside_the_project_has_no_run_target() {
        let dir = cargo_project();
        assert!(config_for_file(dir.path(), &PathBuf::from("/elsewhere/main.rs")).is_none());
    }

    #[test]
    fn a_python_file_runs_under_the_interpreter() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        let config = config_for(dir.path(), "scripts/etl.py").unwrap();
        assert_eq!(config.program, toolchain::python_program(dir.path()));
        assert_eq!(config.args, vec!["scripts/etl.py"]);
    }

    #[test]
    fn a_project_with_no_recognised_toolchain_offers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(config_for(dir.path(), "src/main.rs").is_none());
    }

    fn temp(id: &str) -> RunConfig {
        RunConfig {
            id: id.into(),
            name: id.into(),
            temporary: true,
            ..RunConfig::default()
        }
    }

    #[test]
    fn running_the_same_file_twice_keeps_one_entry() {
        let mut configs = Vec::new();
        remember_temporary(&mut configs, temp("a"));
        remember_temporary(&mut configs, temp("a"));
        assert_eq!(configs.len(), 1);
    }

    #[test]
    fn an_existing_saved_entry_for_the_same_target_is_not_downgraded() {
        let saved = RunConfig {
            id: "python-main.py".into(),
            name: "main.py".into(),
            args: vec!["main.py".into(), "--verbose".into()],
            ..RunConfig::default()
        };
        let mut configs = vec![saved];
        remember_temporary(&mut configs, temp("python-main.py"));
        assert_eq!(configs.len(), 1);
        assert!(!configs[0].temporary, "a saved entry stayed saved");
        assert_eq!(configs[0].args, vec!["main.py", "--verbose"]);
    }

    #[test]
    fn the_oldest_temporary_entries_are_evicted_past_the_cap() {
        let mut configs = Vec::new();
        for i in 0..TEMPORARY_CAP + 2 {
            remember_temporary(&mut configs, temp(&format!("t{i}")));
        }
        let ids: Vec<&str> = configs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), TEMPORARY_CAP);
        assert!(!ids.contains(&"t0"), "{ids:?}");
        assert!(ids.contains(&"t6"), "{ids:?}");
    }

    #[test]
    fn saved_configurations_are_never_evicted() {
        let mut configs = vec![RunConfig {
            id: "saved".into(),
            name: "saved".into(),
            ..RunConfig::default()
        }];
        for i in 0..TEMPORARY_CAP + 3 {
            remember_temporary(&mut configs, temp(&format!("t{i}")));
        }
        assert_eq!(configs.len(), TEMPORARY_CAP + 1);
        assert_eq!(configs[0].id, "saved");
    }
}
