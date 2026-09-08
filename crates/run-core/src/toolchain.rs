//! The toolchain table (R1-1): which build tool a project uses, and what
//! that tool's run, build and clean invocations look like.
//!
//! This is the single source of truth for "this project is a Cargo / CMake /
//! Gradle project", shared by three consumers: [`crate::detect`], which turns
//! it into run configurations; `build-core`, which turns it into a build
//! invocation; and `dap-core`, which needs the debug adapter a toolchain
//! implies. `docs/architecture/layering.md` forbids a second detection table,
//! so a new consumer extends this module rather than re-deriving the answer.
//!
//! Detection is marker-file presence only — no invoking `cargo`, `mvn` or
//! `cmake`, exactly as [`crate::detect`] promises for target names.

use std::path::Path;

/// A build tool a project can be driven by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolchainId {
    Cargo,
    Cmake,
    Python,
    Maven,
    Gradle,
    Npm,
    Make,
}

/// A tool invocation: the program to run and its arguments, with no console,
/// cwd or environment. [`crate::LaunchSpec`] is what those get attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ToolCommand {
    fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        }
    }
}

impl ToolchainId {
    /// Every toolchain, in the order [`detect_toolchains`] reports them.
    pub const ALL: [ToolchainId; 7] = [
        ToolchainId::Cargo,
        ToolchainId::Cmake,
        ToolchainId::Python,
        ToolchainId::Maven,
        ToolchainId::Gradle,
        ToolchainId::Npm,
        ToolchainId::Make,
    ];

    /// The persisted identifier. Stable: it reaches `.ide/settings.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolchainId::Cargo => "cargo",
            ToolchainId::Cmake => "cmake",
            ToolchainId::Python => "python",
            ToolchainId::Maven => "maven",
            ToolchainId::Gradle => "gradle",
            ToolchainId::Npm => "npm",
            ToolchainId::Make => "make",
        }
    }

    /// The inverse of [`ToolchainId::as_str`]; `None` for an unknown id, so a
    /// settings file written by a newer version degrades to "no toolchain"
    /// rather than failing to load.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == id)
    }

    /// Files whose presence in the project root means this toolchain is in
    /// use. Any one of them is enough.
    pub fn marker_files(self) -> &'static [&'static str] {
        match self {
            ToolchainId::Cargo => &["Cargo.toml"],
            ToolchainId::Cmake => &["CMakeLists.txt"],
            ToolchainId::Python => &["pyproject.toml", "setup.py", "requirements.txt"],
            ToolchainId::Maven => &["pom.xml"],
            ToolchainId::Gradle => &["build.gradle", "build.gradle.kts", "settings.gradle"],
            ToolchainId::Npm => &["package.json"],
            ToolchainId::Make => &["Makefile", "makefile"],
        }
    }

    /// Whether this toolchain's marker sits in `project_root`.
    pub fn is_present(self, project_root: &Path) -> bool {
        self.marker_files()
            .iter()
            .any(|name| project_root.join(name).is_file())
    }

    /// How to build the whole project, or `None` for a toolchain with no
    /// build step of its own (Python) or no fixed one (Make, whose build
    /// target is whatever the Makefile calls it).
    pub fn build_command(self, project_root: &Path) -> Option<ToolCommand> {
        match self {
            ToolchainId::Cargo => Some(ToolCommand::new("cargo", &["build"])),
            ToolchainId::Cmake => Some(ToolCommand::new("cmake", &["--build", "build"])),
            ToolchainId::Maven => Some(ToolCommand::new(maven_program(project_root), &["compile"])),
            ToolchainId::Gradle => Some(ToolCommand::new(gradle_program(project_root), &["build"])),
            ToolchainId::Npm => Some(ToolCommand {
                program: package_manager(project_root).to_string(),
                args: vec!["run".into(), "build".into()],
            }),
            ToolchainId::Python | ToolchainId::Make => None,
        }
    }

    /// How to discard previous build output, so that Rebuild is clean plus
    /// build rather than a flag we would have to invent per tool.
    pub fn clean_command(self, project_root: &Path) -> Option<ToolCommand> {
        match self {
            ToolchainId::Cargo => Some(ToolCommand::new("cargo", &["clean"])),
            ToolchainId::Cmake => Some(ToolCommand::new(
                "cmake",
                &["--build", "build", "--target", "clean"],
            )),
            ToolchainId::Maven => Some(ToolCommand::new(maven_program(project_root), &["clean"])),
            ToolchainId::Gradle => Some(ToolCommand::new(gradle_program(project_root), &["clean"])),
            ToolchainId::Npm | ToolchainId::Python | ToolchainId::Make => None,
        }
    }

    /// Whether this toolchain's *run* command compiles first.
    ///
    /// `cargo run` and `gradle run` do; a CMake project's run configuration
    /// starts an already-built binary and a `mvn exec:java` run does not
    /// recompile changed sources. That distinction is what decides whether a
    /// detected configuration is given a Build before-launch task: adding
    /// one where the tool already builds would compile twice per run (B2-1).
    pub fn builds_before_running(self) -> bool {
        match self {
            ToolchainId::Cargo | ToolchainId::Gradle | ToolchainId::Npm => true,
            ToolchainId::Cmake | ToolchainId::Maven | ToolchainId::Python | ToolchainId::Make => {
                false
            }
        }
    }

    /// The debug adapter this toolchain's programs are debugged with, by
    /// catalog id. `None` means "we do not ship a default for this one" —
    /// the user can still name an adapter explicitly.
    pub fn debug_adapter(self) -> Option<&'static str> {
        match self {
            ToolchainId::Cargo | ToolchainId::Cmake => Some("codelldb"),
            ToolchainId::Python => Some("debugpy"),
            ToolchainId::Maven | ToolchainId::Gradle => Some("java-debug"),
            ToolchainId::Npm | ToolchainId::Make => None,
        }
    }
}

/// The JS package manager to drive, by lockfile presence: `yarn` next to a
/// `yarn.lock`, `pnpm` next to a `pnpm-lock.yaml`, otherwise `npm`.
/// `package.json` names no package manager, so presence is all there is.
pub fn package_manager(project_root: &Path) -> &'static str {
    if project_root.join("yarn.lock").exists() {
        "yarn"
    } else if project_root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else {
        "npm"
    }
}

/// The interpreter a Python run configuration is launched with. Windows
/// ships `python`; everywhere else `python` may be absent or Python 2, so
/// `python3` is the only safe default.
///
/// This is a host question, not a `cfg!(windows)` one (W4-2): a project
/// opened from a WSL UNC path runs inside the distro regardless of which OS
/// this binary itself was compiled for, so it gets the Linux answer even
/// when `cfg!(windows)` is true.
pub fn python_program(project_root: &Path) -> &'static str {
    let is_remote = process_exec::host::ExecHost::for_path(project_root).is_remote();
    if cfg!(windows) && !is_remote {
        "python"
    } else {
        "python3"
    }
}

/// The project's Gradle wrapper when it has one, so the build runs the
/// version the project pins rather than whatever is on `PATH`.
fn gradle_program(project_root: &Path) -> String {
    wrapper_or(project_root, "gradlew", "gradle")
}

/// The project's Maven wrapper when it has one, for the same reason.
fn maven_program(project_root: &Path) -> String {
    wrapper_or(project_root, "mvnw", "mvn")
}

fn wrapper_or(project_root: &Path, wrapper: &str, fallback: &'static str) -> String {
    let unix = project_root.join(wrapper);
    let windows = project_root.join(format!("{wrapper}.bat"));
    if unix.is_file() {
        // Relative, not absolute: it is spawned with the project root as cwd,
        // and `gradlew` alone would need `.` on PATH.
        format!("./{wrapper}")
    } else if windows.is_file() {
        format!("{wrapper}.bat")
    } else {
        fallback.to_string()
    }
}

/// Every toolchain whose marker file sits in `project_root`, in
/// [`ToolchainId::ALL`] order. A polyglot project reports more than one.
pub fn detect_toolchains(project_root: &Path) -> Vec<ToolchainId> {
    ToolchainId::ALL
        .into_iter()
        .filter(|t| t.is_present(project_root))
        .collect()
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

    // W4-2: a WSL project root always gets the Linux answer, regardless of
    // `cfg!(windows)` (which is false in this Linux CI build anyway, so the
    // one bit this test actually exercises across both host platforms is
    // "remote overrides the compiled-for-Windows branch too").
    #[test]
    fn python_program_is_python3_on_a_wsl_root_even_if_this_were_windows() {
        assert_eq!(
            python_program(Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj")),
            "python3"
        );
    }

    #[test]
    fn every_id_round_trips() {
        for toolchain in ToolchainId::ALL {
            assert_eq!(ToolchainId::from_id(toolchain.as_str()), Some(toolchain));
        }
    }

    #[test]
    fn an_unknown_id_is_none_rather_than_a_panic() {
        assert_eq!(ToolchainId::from_id("bazel"), None);
    }

    #[test]
    fn markers_select_the_toolchain() {
        let dir = project_with(&["Cargo.toml"]);
        assert_eq!(detect_toolchains(dir.path()), vec![ToolchainId::Cargo]);
    }

    #[test]
    fn a_polyglot_project_reports_every_toolchain_in_a_stable_order() {
        let dir = project_with(&["Cargo.toml", "package.json", "Makefile"]);
        assert_eq!(
            detect_toolchains(dir.path()),
            vec![ToolchainId::Cargo, ToolchainId::Npm, ToolchainId::Make]
        );
    }

    #[test]
    fn an_empty_directory_has_no_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_toolchains(dir.path()).is_empty());
    }

    #[test]
    fn a_marker_directory_does_not_count_as_a_marker_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Cargo.toml")).unwrap();
        assert!(detect_toolchains(dir.path()).is_empty());
    }

    #[test]
    fn the_gradle_wrapper_is_preferred_over_gradle_on_path() {
        let dir = project_with(&["build.gradle", "gradlew"]);
        let command = ToolchainId::Gradle.build_command(dir.path()).unwrap();
        assert_eq!(command.program, "./gradlew");
    }

    #[test]
    fn without_a_wrapper_the_tool_on_path_is_used() {
        let dir = project_with(&["build.gradle"]);
        let command = ToolchainId::Gradle.build_command(dir.path()).unwrap();
        assert_eq!(command.program, "gradle");
    }

    #[test]
    fn the_maven_wrapper_is_preferred_too() {
        let dir = project_with(&["pom.xml", "mvnw"]);
        assert_eq!(
            ToolchainId::Maven
                .build_command(dir.path())
                .unwrap()
                .program,
            "./mvnw"
        );
    }

    #[test]
    fn the_lockfile_selects_the_package_manager() {
        let dir = project_with(&["package.json"]);
        assert_eq!(package_manager(dir.path()), "npm");
        let dir = project_with(&["package.json", "yarn.lock"]);
        assert_eq!(package_manager(dir.path()), "yarn");
        let dir = project_with(&["package.json", "pnpm-lock.yaml"]);
        assert_eq!(package_manager(dir.path()), "pnpm");
    }

    #[test]
    fn python_and_make_have_no_build_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ToolchainId::Python.build_command(dir.path()).is_none());
        assert!(ToolchainId::Make.build_command(dir.path()).is_none());
    }

    #[test]
    fn rebuild_is_clean_plus_build_where_the_tool_supports_it() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ToolchainId::Cargo.clean_command(dir.path()).unwrap().args,
            vec!["clean"]
        );
        assert!(ToolchainId::Npm.clean_command(dir.path()).is_none());
    }

    #[test]
    fn only_the_toolchains_whose_run_does_not_compile_need_a_build_first() {
        assert!(ToolchainId::Cargo.builds_before_running());
        assert!(ToolchainId::Gradle.builds_before_running());
        assert!(!ToolchainId::Cmake.builds_before_running());
        assert!(!ToolchainId::Maven.builds_before_running());
    }

    #[test]
    fn the_four_planned_toolchains_name_a_debug_adapter() {
        assert_eq!(ToolchainId::Cargo.debug_adapter(), Some("codelldb"));
        assert_eq!(ToolchainId::Cmake.debug_adapter(), Some("codelldb"));
        assert_eq!(ToolchainId::Python.debug_adapter(), Some("debugpy"));
        assert_eq!(ToolchainId::Maven.debug_adapter(), Some("java-debug"));
        assert_eq!(ToolchainId::Gradle.debug_adapter(), Some("java-debug"));
        assert_eq!(ToolchainId::Make.debug_adapter(), None);
    }
}
