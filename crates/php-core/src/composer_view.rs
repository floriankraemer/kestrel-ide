//! The Composer tool window's model: what the window lists (scripts,
//! packages with their installed versions) and the argv of every action
//! it offers. The view paints rows and hands an action's wire name and
//! argument to the bridge; nothing else about Composer is decided there.

use std::path::Path;

use crate::composer::{ComposerError, ComposerJson, ComposerLock};

/// The program every action runs.
pub const COMPOSER_PROGRAM: &str = "composer";

/// A script row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRow {
    pub name: String,
    /// What it runs, for the row's detail text.
    pub commands: Vec<String>,
}

/// A package row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRow {
    pub name: String,
    pub constraint: String,
    /// The version `composer.lock` pins; `None` when not installed.
    pub installed: Option<String>,
    pub dev: bool,
}

/// What the window lists for a project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerView {
    pub scripts: Vec<ScriptRow>,
    pub packages: Vec<PackageRow>,
}

/// Whether a change to `path` can change what [`view`] lists: the manifest
/// and the lock file, wherever the project root is.
pub fn is_manifest_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "composer.json" || name == "composer.lock")
}

/// `Ok(None)` when the project has no `composer.json`.
pub fn view(project: &Path) -> Result<Option<ComposerView>, ComposerError> {
    let Some(json) = ComposerJson::read(project)? else {
        return Ok(None);
    };
    // A missing or unreadable lock only means "nothing installed yet".
    let lock = ComposerLock::read(project)
        .ok()
        .flatten()
        .unwrap_or_default();
    Ok(Some(ComposerView {
        scripts: json
            .scripts
            .into_iter()
            .map(|s| ScriptRow {
                name: s.name,
                commands: s.commands,
            })
            .collect(),
        packages: json
            .require
            .into_iter()
            .map(|r| PackageRow {
                installed: lock.version_of(&r.name).map(str::to_string),
                name: r.name,
                constraint: r.constraint,
                dev: r.dev,
            })
            .collect(),
    }))
}

/// Something the window can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    Install,
    /// All packages, or one.
    Update(Option<String>),
    Require {
        package: String,
        dev: bool,
    },
    Remove(String),
    DumpAutoload,
    Outdated,
    RunScript(String),
}

/// Why an action's argument was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    UnknownAction(String),
    /// The action needs a package or script name and got none.
    MissingArgument,
    /// An argument that could not be a package or script name; refused so a
    /// typed value can never be read by Composer as an option.
    InvalidArgument(String),
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAction(a) => write!(f, "unknown Composer action \"{a}\""),
            Self::MissingArgument => write!(f, "this action needs a package or script name"),
            Self::InvalidArgument(a) => write!(
                f,
                "\"{a}\" is not a valid package name; use vendor/name, optionally with a version constraint"
            ),
        }
    }
}

impl std::error::Error for ActionError {}

impl ComposerAction {
    /// Build an action from its wire name (`install`, `update`, `require`,
    /// `require-dev`, `remove`, `dump-autoload`, `outdated`, `run-script`)
    /// and its argument (blank when it takes none).
    pub fn parse(action: &str, argument: &str) -> Result<Self, ActionError> {
        let argument = argument.trim();
        let package = || -> Result<String, ActionError> {
            if argument.is_empty() {
                return Err(ActionError::MissingArgument);
            }
            if is_package_spec(argument) {
                Ok(argument.to_string())
            } else {
                Err(ActionError::InvalidArgument(argument.to_string()))
            }
        };
        Ok(match action {
            "install" => Self::Install,
            "update" => Self::Update((!argument.is_empty()).then(package).transpose()?),
            "require" => Self::Require {
                package: package()?,
                dev: false,
            },
            "require-dev" => Self::Require {
                package: package()?,
                dev: true,
            },
            "remove" => Self::Remove(package()?),
            "dump-autoload" => Self::DumpAutoload,
            "outdated" => Self::Outdated,
            "run-script" => {
                if argument.is_empty() {
                    return Err(ActionError::MissingArgument);
                }
                if argument.starts_with('-') || argument.contains(char::is_whitespace) {
                    return Err(ActionError::InvalidArgument(argument.to_string()));
                }
                Self::RunScript(argument.to_string())
            }
            other => return Err(ActionError::UnknownAction(other.to_string())),
        })
    }

    /// The arguments after `composer`.
    pub fn args(&self) -> Vec<String> {
        let owned = |parts: &[&str]| parts.iter().map(|p| (*p).to_string()).collect::<Vec<_>>();
        match self {
            Self::Install => owned(&["install"]),
            Self::Update(None) => owned(&["update"]),
            Self::Update(Some(p)) => vec!["update".into(), p.clone()],
            Self::Require {
                package,
                dev: false,
            } => vec!["require".into(), package.clone()],
            Self::Require { package, dev: true } => {
                vec!["require".into(), "--dev".into(), package.clone()]
            }
            Self::Remove(p) => vec!["remove".into(), p.clone()],
            Self::DumpAutoload => owned(&["dump-autoload"]),
            Self::Outdated => owned(&["outdated", "--direct"]),
            Self::RunScript(name) => vec!["run-script".into(), name.clone()],
        }
    }

    /// The command as typed, for a run configuration's name.
    pub fn title(&self) -> String {
        format!("{COMPOSER_PROGRAM} {}", self.args().join(" "))
    }
}

/// `vendor/name`, optionally `:constraint` or `=constraint`; never an
/// option, never containing whitespace.
fn is_package_spec(spec: &str) -> bool {
    if spec.starts_with('-') || spec.contains(char::is_whitespace) {
        return false;
    }
    let name = spec.split([':', '=']).next().unwrap_or(spec);
    match name.split_once('/') {
        Some((vendor, package)) => {
            !vendor.is_empty() && !package.is_empty() && !package.contains('/')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_manifest_and_the_lock_file_change_the_view() {
        assert!(is_manifest_path(Path::new("/p/composer.json")));
        assert!(is_manifest_path(Path::new("/p/composer.lock")));
        assert!(!is_manifest_path(Path::new("/p/src/composer.php")));
        assert!(!is_manifest_path(Path::new(
            "/p/vendor/composer/installed.json"
        )));
    }

    use super::*;

    fn args(action: &str, arg: &str) -> Vec<String> {
        ComposerAction::parse(action, arg).unwrap().args()
    }

    #[test]
    fn actions_become_composer_arguments() {
        assert_eq!(args("install", ""), ["install"]);
        assert_eq!(args("update", ""), ["update"]);
        assert_eq!(
            args("update", "monolog/monolog"),
            ["update", "monolog/monolog"]
        );
        assert_eq!(args("require", "a/b:^1.2"), ["require", "a/b:^1.2"]);
        assert_eq!(args("require-dev", "a/b"), ["require", "--dev", "a/b"]);
        assert_eq!(args("remove", "a/b"), ["remove", "a/b"]);
        assert_eq!(args("dump-autoload", ""), ["dump-autoload"]);
        assert_eq!(args("outdated", ""), ["outdated", "--direct"]);
        assert_eq!(args("run-script", "test"), ["run-script", "test"]);
    }

    #[test]
    fn an_argument_that_could_be_an_option_is_refused() {
        for bad in ["--no-plugins", "-vvv", "a/b --dev", "justaname", "/b", "a/"] {
            assert!(
                matches!(
                    ComposerAction::parse("require", bad),
                    Err(ActionError::InvalidArgument(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            ComposerAction::parse("run-script", "--list"),
            Err(ActionError::InvalidArgument(_))
        ));
    }

    #[test]
    fn missing_and_unknown_are_distinct_errors() {
        assert_eq!(
            ComposerAction::parse("remove", " "),
            Err(ActionError::MissingArgument)
        );
        assert_eq!(
            ComposerAction::parse("run-script", ""),
            Err(ActionError::MissingArgument)
        );
        assert!(matches!(
            ComposerAction::parse("selfupdate", ""),
            Err(ActionError::UnknownAction(_))
        ));
    }

    #[test]
    fn the_title_is_the_command_line() {
        let a = ComposerAction::parse("require-dev", "phpunit/phpunit").unwrap();
        assert_eq!(a.title(), "composer require --dev phpunit/phpunit");
    }

    #[test]
    fn the_view_joins_requirements_with_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"require":{"php":"^8.1","a/b":"^1.0","c/d":"*"},"require-dev":{"e/f":"^2"},"scripts":{"test":"phpunit"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("composer.lock"),
            r#"{"packages":[{"name":"a/b","version":"1.4.0"}],"packages-dev":[]}"#,
        )
        .unwrap();
        let v = view(dir.path()).unwrap().unwrap();
        assert_eq!(v.scripts[0].name, "test");
        let installed: Vec<_> = v
            .packages
            .iter()
            .map(|p| (p.name.as_str(), p.installed.as_deref(), p.dev))
            .collect();
        assert_eq!(
            installed,
            [
                ("a/b", Some("1.4.0"), false),
                ("c/d", None, false),
                ("e/f", None, true)
            ]
        );
    }

    #[test]
    fn a_project_without_composer_json_has_no_view() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(view(dir.path()).unwrap(), None);
    }
}
