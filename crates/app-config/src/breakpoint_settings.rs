//! Machine-local breakpoints: `<project_root>/.ide/local/breakpoints.toml`.
//!
//! Under `.ide/local/` rather than beside the run configurations, for the
//! reason [`crate::vcs_local_settings`] gives: a breakpoint is where *this*
//! person is looking right now, not something the project agrees on.
//! Committing everyone's breakpoints would produce a merge conflict per
//! debugging session.
//!
//! The shape is dumb and stringly, like every other table in this crate: it
//! stores what `dap-core` hands it and interprets none of it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{load_toml, project_settings, update_toml, ConfigError};

const LOCAL_DIR: &str = "local";
const BREAKPOINTS_FILE: &str = "breakpoints.toml";
const TEMP_BREAKPOINTS_FILE: &str = "breakpoints.toml.tmp";

/// One persisted line breakpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointSetting {
    /// Absolute path of the file it is in.
    #[serde(default)]
    pub path: String,
    /// 1-based line.
    #[serde(default)]
    pub line: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub condition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hit_condition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log_message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub depends_on: String,
    /// `"all"` or `"thread"`, as `dap_core::SuspendPolicy` spells it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suspend_policy: String,
}

fn default_true() -> bool {
    true
}

/// This machine's breakpoints for one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointSettings {
    #[serde(default, rename = "breakpoint", skip_serializing_if = "Vec::is_empty")]
    pub breakpoints: Vec<BreakpointSetting>,
    /// Adapter-declared exception filter ids the user switched on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exception_filters: Vec<String>,
    /// Mute Breakpoints, which survives a restart exactly as IntelliJ's does.
    #[serde(default, skip_serializing_if = "crate::is_false")]
    pub muted: bool,
}

fn local_dir(project_root: &Path) -> Result<std::path::PathBuf, ConfigError> {
    Ok(project_settings::project_dir(project_root)?.join(LOCAL_DIR))
}

/// Load `<project_root>/.ide/local/breakpoints.toml`. A missing file means
/// no breakpoints, not an error.
pub fn load(project_root: &Path) -> Result<BreakpointSettings, ConfigError> {
    let dir = local_dir(project_root)?;
    load_toml(&dir.join(BREAKPOINTS_FILE))
}

/// Load, edit and save the file, creating `.ide/local` if an edit needs it.
pub fn update(
    project_root: &Path,
    edit: impl FnOnce(&mut BreakpointSettings),
) -> Result<(), ConfigError> {
    let dir = local_dir(project_root)?;
    update_toml(
        &dir.join(BREAKPOINTS_FILE),
        &dir.join(TEMP_BREAKPOINTS_FILE),
        edit,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        project_settings::update(dir.path(), |_| {}).unwrap();
        dir
    }

    #[test]
    fn a_project_with_no_file_has_no_breakpoints() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), BreakpointSettings::default());
    }

    #[test]
    fn breakpoints_round_trip() {
        let root = project();
        update(root.path(), |settings| {
            settings.breakpoints.push(BreakpointSetting {
                path: "/p/src/main.rs".into(),
                line: 12,
                enabled: true,
                condition: "i > 2".into(),
                ..BreakpointSetting::default()
            });
            settings.exception_filters.push("uncaught".into());
            settings.muted = true;
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        assert_eq!(loaded.breakpoints.len(), 1);
        assert_eq!(loaded.breakpoints[0].line, 12);
        assert_eq!(loaded.breakpoints[0].condition, "i > 2");
        assert!(loaded.breakpoints[0].enabled);
        assert_eq!(loaded.exception_filters, ["uncaught"]);
        assert!(loaded.muted);
    }

    #[test]
    fn a_breakpoint_with_no_enabled_key_is_enabled() {
        // Otherwise a hand-written entry would be silently inert, which is
        // the opposite of what writing one down means.
        let root = project();
        let file = root.path().join(".ide/local/breakpoints.toml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            "[[breakpoint]]\npath = \"/p/src/main.rs\"\nline = 3\n",
        )
        .unwrap();
        assert!(load(root.path()).unwrap().breakpoints[0].enabled);
    }

    #[test]
    fn it_lives_under_the_gitignored_local_directory() {
        let root = project();
        update(root.path(), |settings| {
            settings.breakpoints.push(BreakpointSetting {
                path: "/p/a.rs".into(),
                line: 1,
                ..BreakpointSetting::default()
            });
        })
        .unwrap();
        assert!(root.path().join(".ide/local/breakpoints.toml").is_file());
    }
}
