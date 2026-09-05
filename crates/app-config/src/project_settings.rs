//! Per-project settings: `<project_root>/.ide/settings.toml`, layered over the
//! global `settings.toml`.
//!
//! This module is **persistence only**, per ADR-0017. It does not know which
//! layer wins, which fields may be overridden, or where an effective value
//! came from — those are rules, and rules live in `settings-model`. All this
//! does is read and write a file.
//!
//! # Why a separate, sparse type
//!
//! [`Settings`](crate::Settings) gives every field `#[serde(default)]`, so a
//! missing key and a key explicitly set to the default are indistinguishable
//! once parsed. That is fine for a single file and fatal for a layered one:
//! "tab width is not set here, ask the global layer" and "tab width is set to
//! 0" have to be different answers. So every field here is an `Option`, and
//! `None` means *absent*, not *default*.
//!
//! # Scope
//!
//! Only settings that describe **the project** belong here — a project may
//! configure the project, not the person reading it. Theme, fonts, keymap and
//! AI providers stay global on purpose.
//!
//! That is the four areas ADR-0022 names: language servers, editing
//! behaviour, run configurations and index excludes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    load_toml, save_toml, update_toml, ConfigError, DebugAdapterSetting, EditingSettings,
    LanguageServerSetting, RunConfigSetting, TerminalSettings,
};

/// Directory holding a project's IDE files, inside the project root.
pub const PROJECT_DIR: &str = ".ide";
const PROJECT_SETTINGS_FILE: &str = "settings.toml";
const TEMP_PROJECT_SETTINGS_FILE: &str = "settings.toml.tmp";
const PROJECT_GITIGNORE: &str = ".gitignore";

/// What `.ide/.gitignore` is seeded with. `settings.toml` is meant to be
/// committed and shared with everyone working on the project; anything
/// machine-local goes in `.ide/local/` and is ignored.
const PROJECT_GITIGNORE_BODY: &str = "\
# Project settings are meant to be committed and shared.
# Machine-local state goes here and is not.
local/
";

/// The schema version this build writes.
///
/// [`Settings`](crate::Settings) has no version and no migration path, which
/// leaves it unable to tell a file from the future apart from a corrupt one.
/// The project layer starts with one while it costs nothing.
pub const CURRENT_VERSION: u32 = 1;

/// Per-project overrides. Every field is `Option`: `None` means the project
/// does not override it, and the global layer's value stands.
/// `[remote_attach]`: where a remote debuggee listens, and how its paths
/// line up with this checkout (D4-2).
///
/// `path_mappings` are `"local=remote"` strings — one line each, the same
/// shape the environment and before-launch fields already use, because
/// TOML tables of two keys read worse than the pair they encode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAttachSetting {
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_mappings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// Schema version of the file on disk. Absent means "written before
    /// versioning", which is treated as version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,

    /// Per-language server overrides, as `[[language_server]]` blocks — the
    /// same shape the global layer uses, so a user can move a block between
    /// the two files unchanged.
    #[serde(
        default,
        rename = "language_server",
        skip_serializing_if = "Option::is_none"
    )]
    pub language_servers: Option<Vec<LanguageServerSetting>>,

    /// The project's `[editing]` overrides — tab width, spaces-vs-tabs and
    /// the save rules, with the same per-language table the global layer
    /// has. One of the four project-scoped areas ADR-0022 names, and the
    /// first of them whose counterpart in [`Settings`](crate::Settings) now
    /// exists: how a project's files are indented is a property of the
    /// project, which is exactly what a shared, committed file should say.
    ///
    /// Sparse like everything else here: an absent section means the project
    /// overrides nothing, not that it overrides everything with defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editing: Option<EditingSettings>,

    /// The project's `[[run_config]]` entries (F4-4, ADR-0022): a run
    /// configuration is the definition of a project, not a preference, so it
    /// lives here rather than in the global `settings.toml`.
    ///
    /// Sparse like the rest of this struct: `None` means the project has no
    /// run configurations file section at all (distinct from `Some(vec![])`,
    /// which is a project that has explicitly cleared every configuration).
    #[serde(
        default,
        rename = "run_config",
        skip_serializing_if = "Option::is_none"
    )]
    pub run_configs: Option<Vec<RunConfigSetting>>,

    /// Debug adapter overrides (D1-4): replace the command of an adapter
    /// the IDE ships knowledge of, or introduce one it has never heard of.
    /// Sparse like the rest — `None` is "this project says nothing about
    /// adapters", which is not the same as explicitly overriding none.
    #[serde(
        default,
        rename = "debug_adapter",
        skip_serializing_if = "Option::is_none"
    )]
    pub debug_adapters: Option<Vec<DebugAdapterSetting>>,

    /// The remote debuggee this project was last attached to (D4-2), so
    /// the next Attach to Remote offers it instead of asking for a host and
    /// port that have not changed since yesterday.
    ///
    /// Written by the IDE, but a normal committed setting rather than a
    /// machine-local one: which debug server a project attaches to — and
    /// especially how a container's paths map onto the checkout — is the
    /// same for everyone working on it, and `path_mappings` is only ever
    /// editable here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_attach: Option<RemoteAttachSetting>,

    /// Gitignore-syntax patterns the project index skips, on top of the
    /// `.gitignore` rules the walker already honours (ADR-0022's fourth
    /// project-scoped area).
    ///
    /// Project-shaped by construction: which of *this* tree's directories
    /// are generated output is a property of the project, and a checked-out
    /// copy should not have to rediscover it.
    ///
    /// Sparse like the rest: `None` is "the project says nothing about
    /// excludes", which is a different answer from `Some(vec![])` — the
    /// project explicitly excluding nothing, and so overriding a global
    /// list.
    #[serde(
        default,
        rename = "index_exclude",
        skip_serializing_if = "Option::is_none"
    )]
    pub index_excludes: Option<Vec<String>>,

    /// The project's `[terminal]` overrides — which shell a terminal opened
    /// in this checkout spawns, where it starts, and what it adds to the
    /// environment.
    ///
    /// Project-shaped for the same reason as `[editing]`: a repository whose
    /// tooling only runs under WSL, or under `bash` on a machine whose owner
    /// uses `fish`, is describing the checkout, not the person.
    ///
    /// Sparse like the rest: `None` is "the project says nothing about the
    /// terminal".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalSettings>,

    /// Keys this build does not understand, kept verbatim so a round trip
    /// through an older binary does not delete what a newer one wrote.
    ///
    /// Without this, opening a project in an older build and changing one
    /// setting would silently drop every key that build had never heard of.
    #[serde(flatten, default, skip_serializing_if = "toml::Table::is_empty")]
    pub unknown: toml::Table,
}

impl ProjectSettings {
    /// True if nothing is overridden — the file, if written, would say nothing.
    pub fn is_empty(&self) -> bool {
        self.language_servers.is_none()
            && self.editing.is_none()
            && self.run_configs.is_none()
            && self.debug_adapters.is_none()
            && self.remote_attach.is_none()
            && self.index_excludes.is_none()
            && self.terminal.is_none()
            && self.unknown.is_empty()
    }
}

/// `<project_root>/.ide`, verified to stay inside the project.
///
/// Both load and save go through here — as does `vcs_local_settings`, which
/// keeps its own file under this same directory and relies on this same
/// symlink check rather than repeating it.
///
/// A `.ide` that resolves outside the project — most obviously a symlink
/// pointing elsewhere — is refused rather than followed: reading it would
/// disclose a file the project has no claim to, and writing it would let a
/// checked-out repository scribble outside its own directory.
pub(crate) fn project_dir(project_root: &Path) -> Result<PathBuf, ConfigError> {
    let dir = project_root.join(PROJECT_DIR);
    // Nothing there yet is fine — it cannot escape anywhere.
    if !dir.exists() {
        return Ok(dir);
    }
    let real_dir = dir.canonicalize()?;
    let real_root = project_root.canonicalize()?;
    if !real_dir.starts_with(&real_root) {
        return Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} resolves to {}, which is outside the project at {}",
                dir.display(),
                real_dir.display(),
                real_root.display()
            ),
        )));
    }
    Ok(real_dir)
}

/// Load `<project_root>/.ide/settings.toml`.
///
/// A missing file (or a project with no `.ide` at all) is not an error — it
/// means the project overrides nothing. A malformed file **is** an error, and
/// is reported rather than silently becoming defaults; the caller's job is to
/// tell the user and carry on with the global layer, not to overwrite what it
/// could not read.
///
/// A file whose `version` is newer than this build understands is also an
/// error, for the same reason: it is likelier to be a file this build would
/// damage than one it should reset.
pub fn load(project_root: &Path) -> Result<ProjectSettings, ConfigError> {
    let dir = project_dir(project_root)?;
    let settings: ProjectSettings = load_toml(&dir.join(PROJECT_SETTINGS_FILE))?;
    if let Some(version) = settings.version {
        if version > CURRENT_VERSION {
            return Err(ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "project settings are version {version}, but this build understands \
                     at most {CURRENT_VERSION}; update the IDE rather than letting it \
                     rewrite the file"
                ),
            )));
        }
    }
    Ok(settings)
}

/// Save to `<project_root>/.ide/settings.toml`, creating `.ide` (and seeding
/// its `.gitignore`) if needed. Atomic, like the global layer.
pub fn save(project_root: &Path, settings: &ProjectSettings) -> Result<(), ConfigError> {
    let dir = project_dir(project_root)?;
    fs::create_dir_all(&dir)?;
    ensure_gitignore(&dir)?;
    let mut to_write = settings.clone();
    to_write.version = Some(CURRENT_VERSION);
    save_toml(
        &dir.join(PROJECT_SETTINGS_FILE),
        &dir.join(TEMP_PROJECT_SETTINGS_FILE),
        &to_write,
    )
}

/// Load, edit, save. Aborts on a load failure rather than writing defaults
/// over a file it could not read.
pub fn update(
    project_root: &Path,
    edit: impl FnOnce(&mut ProjectSettings),
) -> Result<(), ConfigError> {
    let dir = project_dir(project_root)?;
    fs::create_dir_all(&dir)?;
    ensure_gitignore(&dir)?;
    update_toml(
        &dir.join(PROJECT_SETTINGS_FILE),
        &dir.join(TEMP_PROJECT_SETTINGS_FILE),
        |s: &mut ProjectSettings| {
            edit(s);
            s.version = Some(CURRENT_VERSION);
        },
    )
}

/// Seed `.ide/.gitignore` if it is not already there.
///
/// `.ide/settings.toml` is meant to be committed, but it sits next to the
/// index cache at `.ide-index/`, which is not. Without this file the obvious
/// reflex — `echo .ide >> .gitignore` — silently un-commits the settings the
/// project is trying to share. An existing file is never touched: it is the
/// user's.
fn ensure_gitignore(dir: &Path) -> Result<(), ConfigError> {
    let path = dir.join(PROJECT_GITIGNORE);
    if path.exists() {
        return Ok(());
    }
    fs::write(&path, PROJECT_GITIGNORE_BODY)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_settings(root: &Path, body: &str) {
        let dir = root.join(PROJECT_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PROJECT_SETTINGS_FILE), body).unwrap();
    }

    #[test]
    fn absent_file_means_the_project_overrides_nothing() {
        let root = project();
        let settings = load(root.path()).unwrap();
        assert_eq!(settings, ProjectSettings::default());
        assert!(settings.is_empty());
    }

    #[test]
    fn absent_project_dir_is_not_an_error() {
        let root = project();
        assert!(!root.path().join(PROJECT_DIR).exists());
        assert!(load(root.path()).is_ok());
    }

    // The global settings.toml was once wiped to defaults by a load failure
    // being treated as "nothing saved yet". The layered file must not repeat
    // it: a malformed file is reported, and the caller falls back to the
    // global layer rather than overwriting what it could not parse.
    #[test]
    fn malformed_file_is_reported_and_never_becomes_defaults() {
        let root = project();
        write_settings(root.path(), "this is not = = toml");
        let err = load(root.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn malformed_file_is_not_overwritten_by_update() {
        let root = project();
        write_settings(root.path(), "this is not = = toml");
        assert!(update(root.path(), |s| {
            s.language_servers = Some(Vec::new());
        })
        .is_err());
        let on_disk =
            fs::read_to_string(root.path().join(PROJECT_DIR).join(PROJECT_SETTINGS_FILE)).unwrap();
        assert_eq!(
            on_disk, "this is not = = toml",
            "the bad file was rewritten"
        );
    }

    #[test]
    fn a_version_from_the_future_is_refused() {
        let root = project();
        write_settings(root.path(), &format!("version = {}\n", CURRENT_VERSION + 1));
        assert!(load(root.path()).is_err());
    }

    #[test]
    fn unknown_keys_survive_a_round_trip() {
        let root = project();
        write_settings(
            root.path(),
            "version = 1\nsomething_new = \"from a later build\"\n",
        );
        let loaded = load(root.path()).unwrap();
        assert!(loaded.unknown.contains_key("something_new"));

        save(root.path(), &loaded).unwrap();
        let body =
            fs::read_to_string(root.path().join(PROJECT_DIR).join(PROJECT_SETTINGS_FILE)).unwrap();
        assert!(
            body.contains("from a later build"),
            "an older build dropped a newer build's key: {body}"
        );
    }

    #[test]
    fn language_servers_round_trip() {
        let root = project();
        update(root.path(), |s| {
            s.language_servers = Some(vec![LanguageServerSetting {
                language_id: "rust".into(),
                command: Some("/opt/rust-analyzer".into()),
                ..LanguageServerSetting::default()
            }]);
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        let servers = loaded.language_servers.expect("language servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].language_id, "rust");
        assert_eq!(servers[0].command.as_deref(), Some("/opt/rust-analyzer"));
    }

    #[test]
    fn run_configs_round_trip() {
        let root = project();
        update(root.path(), |s| {
            s.run_configs = Some(vec![RunConfigSetting {
                id: "run-1".into(),
                name: "cargo run".into(),
                program: "cargo".into(),
                args: vec!["run".into()],
                cwd: Some("$PROJECT_DIR".into()),
                env: vec![("RUST_LOG".into(), "debug".into())],
                ..Default::default()
            }]);
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        let configs = loaded.run_configs.expect("run configs");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].id, "run-1");
        assert_eq!(configs[0].name, "cargo run");
        assert_eq!(configs[0].program, "cargo");
        assert_eq!(configs[0].args, vec!["run".to_string()]);
        assert_eq!(configs[0].cwd.as_deref(), Some("$PROJECT_DIR"));
        assert_eq!(
            configs[0].env,
            vec![("RUST_LOG".to_string(), "debug".to_string())]
        );
    }

    #[test]
    fn terminal_settings_round_trip() {
        let root = project();
        update(root.path(), |s| {
            let mut terminal = TerminalSettings {
                shell_id: "wsl:Ubuntu".into(),
                start_directory: "sub/crate".into(),
                ..TerminalSettings::default()
            };
            terminal.env.insert("RUST_LOG".into(), "debug".into());
            s.terminal = Some(terminal);
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        let terminal = loaded.terminal.expect("terminal section");
        assert_eq!(terminal.shell_id, "wsl:Ubuntu");
        assert_eq!(terminal.start_directory, "sub/crate");
        assert_eq!(
            terminal.env.get("RUST_LOG").map(String::as_str),
            Some("debug")
        );
    }

    /// A project file written before this section existed still loads, and
    /// says nothing about the terminal rather than defaulting it.
    #[test]
    fn absent_terminal_key_falls_back_to_none() {
        let root = project();
        update(root.path(), |s| {
            s.index_excludes = Some(vec!["target/".into()]);
        })
        .unwrap();

        assert!(load(root.path()).unwrap().terminal.is_none());
    }

    #[test]
    fn absent_run_configs_key_falls_back_to_none() {
        let root = project();
        write_settings(root.path(), "version = 1\n");
        let loaded = load(root.path()).unwrap();
        assert_eq!(loaded.run_configs, None);
    }

    #[test]
    fn editing_overrides_round_trip_and_stay_sparse() {
        let root = project();
        update(root.path(), |s| {
            s.editing = Some(EditingSettings {
                tab_width: 2,
                ..EditingSettings::default()
            });
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        let editing = loaded.editing.expect("editing section");
        assert_eq!(editing.tab_width, 2);
        // Everything the project did not say stays absent, so the global
        // layer still shows through.
        assert_eq!(editing.use_spaces, None);
        let body =
            fs::read_to_string(root.path().join(PROJECT_DIR).join(PROJECT_SETTINGS_FILE)).unwrap();
        assert!(!body.contains("use_spaces"), "{body}");
    }

    #[test]
    fn save_stamps_the_current_version() {
        let root = project();
        save(root.path(), &ProjectSettings::default()).unwrap();
        assert_eq!(load(root.path()).unwrap().version, Some(CURRENT_VERSION));
    }

    #[test]
    fn saving_seeds_a_gitignore_but_never_replaces_one() {
        let root = project();
        save(root.path(), &ProjectSettings::default()).unwrap();
        let gitignore = root.path().join(PROJECT_DIR).join(PROJECT_GITIGNORE);
        assert!(gitignore.exists());
        assert!(fs::read_to_string(&gitignore).unwrap().contains("local/"));

        fs::write(&gitignore, "# mine\n").unwrap();
        save(root.path(), &ProjectSettings::default()).unwrap();
        assert_eq!(fs::read_to_string(&gitignore).unwrap(), "# mine\n");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let root = project();
        save(root.path(), &ProjectSettings::default()).unwrap();
        assert!(!root
            .path()
            .join(PROJECT_DIR)
            .join(TEMP_PROJECT_SETTINGS_FILE)
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_dot_ide_escaping_the_project_is_refused_on_load_and_save() {
        let root = project();
        let outside = project();
        fs::create_dir_all(outside.path().join("escaped")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("escaped"),
            root.path().join(PROJECT_DIR),
        )
        .unwrap();

        assert!(load(root.path()).is_err(), "load followed the symlink out");
        assert!(
            save(root.path(), &ProjectSettings::default()).is_err(),
            "save followed the symlink out"
        );
    }
}
