//! What a build runs (B1-1): a request turned into the steps that satisfy
//! it, each one a `run_core::LaunchSpec`.
//!
//! Build is *delegated*, never modelled (ADR-0040): the argv comes from
//! `run_core::toolchain`, this module only decides which of them run and in
//! what order. Nothing here knows what a compiler is.

use std::path::{Path, PathBuf};

use run_core::toolchain::{ToolCommand, ToolchainId};
use run_core::{ConsoleKind, LaunchSpec};

use crate::error::BuildError;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildKind {
    /// Build the project as it stands.
    Build,
    /// Discard previous output first — the tool's own clean, then its
    /// build, rather than a flag we would have to invent per tool.
    Rebuild,
    /// Build one named target, for the toolchains that address targets
    /// (`cargo build -p`, `cmake --build build --target`, `gradle :module:build`).
    Target(String),
}

/// One build request against one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    pub toolchain: ToolchainId,
    pub kind: BuildKind,
    pub project_root: PathBuf,
}

impl BuildSpec {
    pub fn new(toolchain: ToolchainId, kind: BuildKind, project_root: impl Into<PathBuf>) -> Self {
        Self {
            toolchain,
            kind,
            project_root: project_root.into(),
        }
    }

    /// The steps this request runs, in order. More than one only for
    /// [`BuildKind::Rebuild`], which is clean followed by build.
    ///
    /// Every step is `ConsoleKind::Pty`, which is not what a build wants
    /// and is what we have: `pty-core` is the only process transport in the
    /// repo, and it is the one that can kill a build's whole process tree
    /// (`cargo` spawns `rustc`, `gradle` spawns a daemon) rather than only
    /// its direct child. The cost of a tty is coloured diagnostics, and the
    /// adapter already strips ANSI out of run-console output before caching
    /// it — the parsers here are fed that same stripped text.
    /// `ConsoleKind::Pipes` therefore stays reserved for `dap-core`, whose
    /// adapter really does speak a protocol over stdio.
    pub fn steps(&self) -> Result<Vec<LaunchSpec>, BuildError> {
        let name = self.toolchain.as_str().to_string();
        let build = self
            .toolchain
            .build_command(&self.project_root)
            .ok_or_else(|| BuildError::UnsupportedToolchain(name.clone()))?;

        let mut steps = Vec::new();
        if self.kind == BuildKind::Rebuild {
            let clean = self
                .toolchain
                .clean_command(&self.project_root)
                .ok_or(BuildError::NoCleanStep(name))?;
            steps.push(self.to_launch_spec(clean));
        }

        let mut build = match &self.kind {
            BuildKind::Target(target) => with_target(self.toolchain, build, target),
            _ => build,
        };
        if self.toolchain == ToolchainId::Cargo {
            // Only the build step, and only for Cargo: this is what makes
            // the diagnostics exact rather than recovered from prose
            // (`crate::cargo_json`). `cargo clean` has nothing to say.
            build
                .args
                .push(crate::cargo_json::MESSAGE_FORMAT_ARG.to_string());
        }
        steps.push(self.to_launch_spec(build));
        Ok(steps)
    }

    fn to_launch_spec(&self, command: ToolCommand) -> LaunchSpec {
        LaunchSpec {
            program: command.program,
            args: command.args,
            cwd: Some(self.project_root.clone()),
            env: Vec::new(),
            console: ConsoleKind::Pty,
            path_map: None,
        }
    }
}

/// How each toolchain names one target on its build command line. A
/// toolchain with no such spelling builds everything rather than being
/// handed a flag it would reject.
fn with_target(toolchain: ToolchainId, mut command: ToolCommand, target: &str) -> ToolCommand {
    match toolchain {
        ToolchainId::Cargo => {
            command.args.push("-p".into());
            command.args.push(target.to_string());
        }
        ToolchainId::Cmake => {
            command.args.push("--target".into());
            command.args.push(target.to_string());
        }
        ToolchainId::Gradle => {
            // `gradle :module:build` replaces the plain `build` task rather
            // than adding to it.
            command.args = vec![format!(":{target}:build")];
        }
        ToolchainId::Maven | ToolchainId::Npm | ToolchainId::Python | ToolchainId::Make => {}
    }
    command
}

/// The toolchain to build a project with: the first one present that has a
/// build command at all.
///
/// Order is [`ToolchainId::ALL`]'s, so a Rust project that also carries a
/// `package.json` builds with Cargo — the same order detection already
/// reports targets in, rather than a second notion of "the main one".
pub fn buildable_toolchain(project_root: &Path) -> Result<ToolchainId, BuildError> {
    run_core::detect_toolchains(project_root)
        .into_iter()
        .find(|t| t.build_command(project_root).is_some())
        .ok_or(BuildError::NoBuildableToolchain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project_with(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in files {
            fs::write(dir.path().join(name), "").unwrap();
        }
        dir
    }

    #[test]
    fn a_build_is_one_step_launched_over_pipes_in_the_project_root() {
        let dir = project_with(&["Cargo.toml"]);
        let spec = BuildSpec::new(ToolchainId::Cargo, BuildKind::Build, dir.path());
        let steps = spec.steps().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].program, "cargo");
        assert_eq!(steps[0].args, vec!["build", "--message-format=json"]);
        assert_eq!(steps[0].cwd.as_deref(), Some(dir.path()));
        assert_eq!(steps[0].console, ConsoleKind::Pty);
    }

    #[test]
    fn a_rebuild_is_clean_then_build_in_that_order() {
        let dir = project_with(&["Cargo.toml"]);
        let steps = BuildSpec::new(ToolchainId::Cargo, BuildKind::Rebuild, dir.path())
            .steps()
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].args, vec!["clean"], "clean is not asked for JSON");
        assert_eq!(steps[1].args, vec!["build", "--message-format=json"]);
    }

    #[test]
    fn a_rebuild_on_a_toolchain_with_no_clean_step_refuses_rather_than_half_building() {
        let dir = project_with(&["package.json"]);
        let err = BuildSpec::new(ToolchainId::Npm, BuildKind::Rebuild, dir.path())
            .steps()
            .unwrap_err();
        assert_eq!(err, BuildError::NoCleanStep("npm".into()));
    }

    #[test]
    fn a_toolchain_with_no_build_command_refuses() {
        let dir = project_with(&["pyproject.toml"]);
        let err = BuildSpec::new(ToolchainId::Python, BuildKind::Build, dir.path())
            .steps()
            .unwrap_err();
        assert_eq!(err, BuildError::UnsupportedToolchain("python".into()));
    }

    #[test]
    fn each_toolchain_spells_a_target_its_own_way() {
        let dir = project_with(&["Cargo.toml", "CMakeLists.txt", "build.gradle"]);
        let args = |toolchain| {
            BuildSpec::new(toolchain, BuildKind::Target("app".into()), dir.path())
                .steps()
                .unwrap()
                .pop()
                .unwrap()
                .args
        };
        assert_eq!(
            args(ToolchainId::Cargo),
            vec!["build", "-p", "app", "--message-format=json"]
        );
        assert_eq!(
            args(ToolchainId::Cmake),
            vec!["--build", "build", "--target", "app"]
        );
        assert_eq!(args(ToolchainId::Gradle), vec![":app:build"]);
    }

    #[test]
    fn a_toolchain_with_no_target_spelling_builds_everything() {
        let dir = project_with(&["pom.xml"]);
        let steps = BuildSpec::new(
            ToolchainId::Maven,
            BuildKind::Target("app".into()),
            dir.path(),
        )
        .steps()
        .unwrap();
        assert_eq!(steps[0].args, vec!["compile"]);
    }

    #[test]
    fn the_first_present_toolchain_that_can_build_is_chosen() {
        let dir = project_with(&["pyproject.toml", "package.json"]);
        // Python is earlier in ALL, but has no build command at all.
        assert_eq!(buildable_toolchain(dir.path()).unwrap(), ToolchainId::Npm);
    }

    #[test]
    fn a_project_with_nothing_to_build_says_so() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            buildable_toolchain(dir.path()).unwrap_err(),
            BuildError::NoBuildableToolchain
        );
    }
}
