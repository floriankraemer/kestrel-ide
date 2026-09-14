//! Running a Maven sync: read the root POM statically to find modules
//! (A5), then for each module run `help:effective-pom` and the pinned
//! verbose `dependency:tree`, parsing both.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use run_core::toolchain::maven_program;

use super::{dep_tree, effective_pom, goals, pom};
use crate::model::{BuildModel, Module, SourceContent, SourceRoot, SourceRootKind, Task, Tool};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// `dependency:tree -Dverbose` pinned to the exact coordinate the plan
/// states: `-Dverbose` was a documented no-op on the 3.0-3.1 releases, so
/// an unpinned invocation could silently stop reporting conflicts.
const DEPENDENCY_PLUGIN_GOAL: &str = "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:tree";

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub offline: bool,
    pub timeout: Option<Duration>,
}

#[derive(Debug)]
pub enum SyncError {
    NotFound,
    TimedOut,
    Io(String),
    BuildFailed {
        stderr: String,
    },
    EffectivePomUnreadable(String),
    EffectivePomInvalid(String),
    /// The root `pom.xml` itself could not be read or parsed — nothing to
    /// sync at all.
    RootPomInvalid(String),
}

impl From<process_exec::Failure> for SyncError {
    fn from(failure: process_exec::Failure) -> Self {
        match failure {
            process_exec::Failure::NotFound => SyncError::NotFound,
            process_exec::Failure::TimedOut => SyncError::TimedOut,
            process_exec::Failure::Io(message) => SyncError::Io(message),
        }
    }
}

pub fn sync(project_root: &Path, opts: &SyncOptions) -> Result<BuildModel, SyncError> {
    let root_pom_text = std::fs::read_to_string(project_root.join("pom.xml"))
        .map_err(|err| SyncError::RootPomInvalid(err.to_string()))?;
    let root_pom =
        pom::parse(&root_pom_text).map_err(|err| SyncError::RootPomInvalid(err.to_string()))?;

    let module_dirs: Vec<PathBuf> = if root_pom.modules.is_empty() {
        vec![project_root.to_path_buf()]
    } else {
        root_pom
            .modules
            .iter()
            .map(|name| project_root.join(name))
            .collect()
    };

    let mut modules = Vec::with_capacity(module_dirs.len());
    for dir in &module_dirs {
        modules.push(sync_module(dir, opts)?);
    }

    let tasks = goals::LIFECYCLE_PHASES
        .iter()
        .map(|phase| Task {
            path: phase.to_string(),
            name: phase.to_string(),
            group: "lifecycle".to_string(),
            description: String::new(),
        })
        .collect();

    Ok(BuildModel {
        tool: Tool::Maven,
        root: project_root.to_path_buf(),
        modules,
        tasks,
        warnings: Vec::new(),
        synced_at: SystemTime::now(),
    })
}

fn sync_module(module_dir: &Path, opts: &SyncOptions) -> Result<Module, SyncError> {
    let program = maven_program(module_dir);
    let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

    let effective = run_effective_pom(&program, module_dir, opts.offline, timeout)?;
    let dep_tree_nodes = run_dependency_tree(&program, module_dir, opts.offline, timeout)?;
    let dependencies = dep_tree::build_children(&dep_tree_nodes);

    let source_roots = [
        effective
            .source_directory
            .clone()
            .map(|p| (p, SourceRootKind::Main, SourceContent::Java)),
        effective
            .test_source_directory
            .clone()
            .map(|p| (p, SourceRootKind::Test, SourceContent::Java)),
    ]
    .into_iter()
    .flatten()
    .map(|(path, kind, content)| SourceRoot {
        path,
        kind,
        content,
    })
    .collect();

    let output_dirs = [effective.output_directory, effective.test_output_directory]
        .into_iter()
        .flatten()
        .collect();

    Ok(Module {
        path: format!("{}:{}", effective.group_id, effective.artifact_id),
        name: effective.artifact_id,
        dir: module_dir.to_path_buf(),
        build_file: module_dir.join("pom.xml"),
        source_roots,
        output_dirs,
        jdk: None,
        dependencies,
        children: Vec::new(),
    })
}

fn run_effective_pom(
    program: &str,
    module_dir: &Path,
    offline: bool,
    timeout: Duration,
) -> Result<effective_pom::EffectivePom, SyncError> {
    let out_path = temp_file(module_dir, "effective-pom", "xml");
    let mut args = vec![
        "-B".to_string(),
        "help:effective-pom".to_string(),
        format!("-Doutput={}", out_path.display()),
    ];
    if offline {
        args.push("-o".to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = process_exec::run(program, &arg_refs, module_dir, None, timeout, &[])?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&out_path);
        return Err(SyncError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let text = std::fs::read_to_string(&out_path)
        .map_err(|err| SyncError::EffectivePomUnreadable(err.to_string()))?;
    let _ = std::fs::remove_file(&out_path);
    effective_pom::parse(&text).map_err(|err| SyncError::EffectivePomInvalid(err.to_string()))
}

fn run_dependency_tree(
    program: &str,
    module_dir: &Path,
    offline: bool,
    timeout: Duration,
) -> Result<Vec<dep_tree::DepTreeNode>, SyncError> {
    let mut args = vec![
        "-B".to_string(),
        DEPENDENCY_PLUGIN_GOAL.to_string(),
        "-Dverbose".to_string(),
    ];
    if offline {
        args.push("-o".to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = process_exec::run(program, &arg_refs, module_dir, None, timeout, &[])?;
    if !output.status.success() {
        return Err(SyncError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(dep_tree::parse(&text))
}

fn temp_file(module_dir: &Path, label: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!(
        "ide-{label}-{}-{unique}.{extension}",
        module_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    std::env::temp_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_root_pom_is_reported_not_panicked() {
        let dir = tempfile::tempdir().unwrap();
        let result = sync(dir.path(), &SyncOptions::default());
        assert!(matches!(result, Err(SyncError::RootPomInvalid(_))));
    }
}
