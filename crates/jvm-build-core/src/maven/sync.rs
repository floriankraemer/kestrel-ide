//! Running a Maven sync: read the root POM statically to find modules
//! (A5), then for each module run `help:effective-pom` and the pinned
//! verbose `dependency:tree`, parsing both.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use run_core::toolchain::maven_program;

use super::{dep_tree, effective_pom, goals, pom};
use crate::model::{
    BuildModel, Module, Plugin, SourceContent, SourceRoot, SourceRootKind, Task, Tool,
};

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

    // The wrapper (`mvnw`) lives at the project root, never inside a child
    // module's own directory, and `run_core::toolchain::wrapper_or` returns
    // it as a path relative to wherever the process is spawned (`./mvnw`) —
    // so every module's invocation is spawned with `project_root` itself as
    // the working directory, never `module_dir`, and reaches its own POM
    // through Maven's own `-f`/`--file` flag instead of a `cd`. Resolving
    // the wrapper once here and passing the program string down (rather
    // than re-resolving it per module directory, which would silently see
    // no `mvnw` in a child directory and fall back to a bare `mvn` on
    // `PATH`) is the other half of the same fix.
    let program = maven_program(project_root);

    let mut modules = Vec::with_capacity(module_dirs.len());
    for dir in &module_dirs {
        modules.push(sync_module(&program, project_root, dir, opts)?);
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
        root_name: root_pom.artifact_id,
        modules,
        tasks,
        warnings: Vec::new(),
        profiles: root_pom.profiles,
        synced_at: SystemTime::now(),
    })
}

fn sync_module(
    program: &str,
    project_root: &Path,
    module_dir: &Path,
    opts: &SyncOptions,
) -> Result<Module, SyncError> {
    let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let pom_path = module_dir.join("pom.xml");

    let effective = run_effective_pom(program, project_root, &pom_path, opts.offline, timeout)?;
    let dep_tree_nodes =
        run_dependency_tree(program, project_root, &pom_path, opts.offline, timeout)?;
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

    let plugins = effective
        .plugins
        .into_iter()
        .map(|plugin| Plugin {
            group: plugin.group_id,
            artifact: plugin.artifact_id,
            version: plugin.version,
            goals: plugin.goals,
        })
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
        plugins,
        children: Vec::new(),
    })
}

fn run_effective_pom(
    program: &str,
    project_root: &Path,
    pom_path: &Path,
    offline: bool,
    timeout: Duration,
) -> Result<effective_pom::EffectivePom, SyncError> {
    let out_path = temp_file(pom_path, "effective-pom", "xml");
    let mut args = vec![
        "-B".to_string(),
        "-f".to_string(),
        pom_path.display().to_string(),
        "help:effective-pom".to_string(),
        format!("-Doutput={}", out_path.display()),
    ];
    if offline {
        args.push("-o".to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = process_exec::run(program, &arg_refs, project_root, None, timeout, &[])?;
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
    project_root: &Path,
    pom_path: &Path,
    offline: bool,
    timeout: Duration,
) -> Result<Vec<dep_tree::DepTreeNode>, SyncError> {
    let mut args = vec![
        "-B".to_string(),
        "-f".to_string(),
        pom_path.display().to_string(),
        DEPENDENCY_PLUGIN_GOAL.to_string(),
        "-Dverbose".to_string(),
    ];
    if offline {
        args.push("-o".to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = process_exec::run(program, &arg_refs, project_root, None, timeout, &[])?;
    if !output.status.success() {
        return Err(SyncError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(dep_tree::parse(&text))
}

fn temp_file(pom_path: &Path, label: &str, extension: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let module_dir_name = pom_path
        .parent()
        .and_then(|dir| dir.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = format!("ide-{label}-{module_dir_name}-{unique}.{extension}");
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

    /// A wrapper-only multi-module project: `mvnw` exists only at the
    /// project root, never inside a child module's own directory. Before
    /// this fix, `sync_module` re-resolved `maven_program` against each
    /// module directory, so the wrapper "existing" was only ever true for
    /// whichever module happened to *be* the project root — every other
    /// module silently fell back to a bare `mvn` on `PATH`.
    #[cfg(unix)]
    #[test]
    fn a_child_modules_sync_finds_the_root_only_wrapper_not_a_bare_mvn() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("pom.xml"),
            r#"<project><modelVersion>4.0.0</modelVersion>
                <groupId>g</groupId><artifactId>root</artifactId><version>1</version>
                <packaging>pom</packaging>
                <modules><module>mod-a</module></modules>
            </project>"#,
        )
        .unwrap();
        fs::create_dir(root.join("mod-a")).unwrap();
        fs::write(
            root.join("mod-a/pom.xml"),
            r#"<project><modelVersion>4.0.0</modelVersion>
                <parent><groupId>g</groupId><artifactId>root</artifactId><version>1</version></parent>
                <artifactId>mod-a</artifactId>
            </project>"#,
        )
        .unwrap();

        // A stub "mvnw" that proves it was invoked with the *root* as its
        // working directory (a real wrapper script's own assumption) by
        // succeeding only when it can see this project's own root POM
        // beside it; a wrong cwd (a child module's directory) would not.
        let stub = root.join("mvnw");
        fs::write(
            &stub,
            "#!/bin/sh\ntest -f pom.xml && test \"$1\" = -B || exit 3\nexit 0\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&stub, perms).unwrap();

        let result = sync_module(
            &maven_program(root),
            root,
            &root.join("mod-a"),
            &SyncOptions {
                offline: false,
                timeout: Some(Duration::from_secs(10)),
            },
        );
        // The stub exits 0 without writing an effective-pom output file —
        // this proves the process actually ran (as `./mvnw` found beside
        // `pom.xml` in the root working directory, not as `NotFound`/
        // `BuildFailed`'s exit-3 guard) and stops exactly where a real
        // `mvn`'s own missing-output-file case would.
        assert!(
            matches!(result, Err(SyncError::EffectivePomUnreadable(_))),
            "{result:?}"
        );
    }
}
