//! The `composer.json` / `composer.lock` model: only what the IDE reads
//! (PSR-4 roots, scripts, the `require.php` constraint, package lists).
//!
//! Parsing is deliberately lenient about shape (a script may be a string or
//! a list, a PSR-4 root a string or a list) and strict about syntax: a file
//! that is not JSON is an error, a file that lacks a key just has none.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde_json::Value;

/// Why a Composer file could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum ComposerError {
    /// The file could not be read from disk.
    Io(String),
    /// The text is not a JSON object.
    Malformed(String),
}

impl fmt::Display for ComposerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(m) => write!(f, "cannot read the Composer file: {m}"),
            Self::Malformed(m) => write!(f, "the Composer file is not valid JSON: {m}"),
        }
    }
}

impl std::error::Error for ComposerError {}

/// One `autoload`/`autoload-dev` PSR-4 mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Psr4Root {
    /// The namespace prefix, e.g. `App\`.
    pub prefix: String,
    /// Directory relative to the project root, e.g. `src/`.
    pub dir: String,
    /// From `autoload-dev`.
    pub dev: bool,
}

/// A `scripts` entry; a script that is a list runs its commands in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerScript {
    pub name: String,
    pub commands: Vec<String>,
}

/// A `require`/`require-dev` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub constraint: String,
    pub dev: bool,
}

/// The parts of `composer.json` the IDE uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerJson {
    pub name: Option<String>,
    /// The `require.php` constraint, verbatim (`^8.1`, `>=8.0 <8.4`).
    pub require_php: Option<String>,
    pub psr4: Vec<Psr4Root>,
    /// Every directory or file `autoload` and `autoload-dev` load code from
    /// (PSR-4, PSR-0 and classmap), project-relative, in file order, without
    /// duplicates. Never the project root itself and never under `vendor/`.
    pub autoload_paths: Vec<String>,
    pub scripts: Vec<ComposerScript>,
    /// Packages only: `php` and `ext-*` platform entries are excluded.
    pub require: Vec<Requirement>,
}

impl ComposerJson {
    pub fn parse(text: &str) -> Result<Self, ComposerError> {
        let root = object(text)?;
        let mut psr4 = Vec::new();
        for (key, dev) in [("autoload", false), ("autoload-dev", true)] {
            let Some(map) = root
                .get(key)
                .and_then(|a| a.get("psr-4"))
                .and_then(Value::as_object)
            else {
                continue;
            };
            for (prefix, dirs) in map {
                for dir in strings(dirs) {
                    psr4.push(Psr4Root {
                        prefix: prefix.clone(),
                        dir,
                        dev,
                    });
                }
            }
        }
        let mut autoload_paths: Vec<String> = Vec::new();
        for section in ["autoload", "autoload-dev"].map(|key| root.get(key)) {
            let Some(section) = section else { continue };
            let mapped = ["psr-4", "psr-0"]
                .iter()
                .filter_map(|kind| section.get(kind).and_then(Value::as_object))
                .flat_map(|map| map.values().flat_map(strings));
            let classmap = section.get("classmap").map(strings).unwrap_or_default();
            for path in mapped.chain(classmap) {
                let path = path
                    .trim_start_matches("./")
                    .trim_end_matches('/')
                    .to_string();
                let in_vendor = path == "vendor" || path.starts_with("vendor/");
                if !path.is_empty() && path != "." && !in_vendor && !autoload_paths.contains(&path)
                {
                    autoload_paths.push(path);
                }
            }
        }
        let scripts = root
            .get("scripts")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(name, v)| ComposerScript {
                        name: name.clone(),
                        commands: strings(v),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut require = Vec::new();
        for (key, dev) in [("require", false), ("require-dev", true)] {
            if let Some(map) = root.get(key).and_then(Value::as_object) {
                for (name, constraint) in map {
                    if is_platform(name) {
                        continue;
                    }
                    require.push(Requirement {
                        name: name.clone(),
                        constraint: constraint.as_str().unwrap_or_default().to_string(),
                        dev,
                    });
                }
            }
        }
        Ok(Self {
            name: root.get("name").and_then(Value::as_str).map(str::to_string),
            require_php: root
                .get("require")
                .and_then(|r| r.get("php"))
                .and_then(Value::as_str)
                .map(str::to_string),
            psr4,
            autoload_paths,
            scripts,
            require,
        })
    }

    /// The paths a project-wide analyzer run covers when the tool's own
    /// config names none: the autoload paths that exist under `project`, as
    /// absolute paths. Empty when there is no readable composer.json or it
    /// names none, which leaves the run to its previous default.
    pub fn analysis_paths(project: &Path) -> Vec<std::path::PathBuf> {
        let Ok(Some(composer)) = Self::read(project) else {
            return Vec::new();
        };
        composer
            .autoload_paths
            .iter()
            .map(|path| project.join(path))
            .filter(|path| path.exists())
            .collect()
    }

    /// Read `<project>/composer.json`; `Ok(None)` when there is none.
    pub fn read(project: &Path) -> Result<Option<Self>, ComposerError> {
        read_optional(&project.join("composer.json"))?
            .map(|text| Self::parse(&text))
            .transpose()
    }
}

/// One installed package from `composer.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub dev: bool,
}

/// The parts of `composer.lock` the IDE uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerLock {
    pub packages: Vec<LockedPackage>,
}

impl ComposerLock {
    pub fn parse(text: &str) -> Result<Self, ComposerError> {
        let root = object(text)?;
        let mut packages = Vec::new();
        for (key, dev) in [("packages", false), ("packages-dev", true)] {
            for p in root
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(name) = p.get("name").and_then(Value::as_str) else {
                    continue;
                };
                packages.push(LockedPackage {
                    name: name.to_string(),
                    version: p
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    dev,
                });
            }
        }
        Ok(Self { packages })
    }

    /// Read `<project>/composer.lock`; `Ok(None)` when there is none.
    pub fn read(project: &Path) -> Result<Option<Self>, ComposerError> {
        read_optional(&project.join("composer.lock"))?
            .map(|text| Self::parse(&text))
            .transpose()
    }

    /// Locked version of `package`, if installed.
    pub fn version_of(&self, package: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|p| p.name == package)
            .map(|p| p.version.as_str())
    }
}

/// `php`, `ext-*`, `lib-*`, `composer-*-api` are platform requirements.
fn is_platform(name: &str) -> bool {
    name == "php"
        || name == "php-64bit"
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        || name.starts_with("composer-")
}

fn object(text: &str) -> Result<BTreeMap<String, Value>, ComposerError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map.into_iter().collect()),
        Ok(_) => Err(ComposerError::Malformed(
            "top level is not an object".into(),
        )),
        Err(e) => Err(ComposerError::Malformed(e.to_string())),
    }
}

/// A string, or a list of strings, as a list.
fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, ComposerError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ComposerError::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
        "name": "acme/app",
        "require": {"php": "^8.1", "ext-json": "*", "monolog/monolog": "^3.0"},
        "require-dev": {"phpunit/phpunit": "^10"},
        "autoload": {"psr-4": {"App\\": "src/", "Lib\\": ["lib/", "vendor-lib/"]}},
        "autoload-dev": {"psr-4": {"Tests\\": "tests/"}},
        "scripts": {"test": "phpunit", "ci": ["@test", "phpstan analyse"]}
    }"#;

    #[test]
    fn reads_php_constraint_and_skips_platform_requirements() {
        let c = ComposerJson::parse(JSON).unwrap();
        assert_eq!(c.require_php.as_deref(), Some("^8.1"));
        let names: Vec<_> = c.require.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["monolog/monolog", "phpunit/phpunit"]);
        assert!(c.require[1].dev);
    }

    #[test]
    fn psr4_roots_accept_strings_and_lists() {
        let c = ComposerJson::parse(JSON).unwrap();
        let dirs: Vec<_> = c.psr4.iter().map(|r| (r.dir.as_str(), r.dev)).collect();
        assert_eq!(
            dirs,
            [
                ("src/", false),
                ("lib/", false),
                ("vendor-lib/", false),
                ("tests/", true)
            ]
        );
    }

    #[test]
    fn autoload_paths_cover_psr4_psr0_and_classmap_but_never_vendor_or_the_root() {
        let c = ComposerJson::parse(
            r#"{
            "autoload": {
                "psr-4": {"App\\": "app/", "Root\\": "", "Lib\\": ["lib/", "./app/"]},
                "psr-0": {"Legacy_": "legacy"},
                "classmap": ["database/seeds", "vendor/acme/x", "vendor"]
            },
            "autoload-dev": {"psr-4": {"Tests\\": "tests/"}, "classmap": ["."]}
        }"#,
        )
        .unwrap();
        assert_eq!(
            c.autoload_paths,
            ["app", "lib", "legacy", "database/seeds", "tests"]
        );
    }

    #[test]
    fn analysis_paths_are_the_existing_autoload_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            ComposerJson::analysis_paths(dir.path()).is_empty(),
            "no composer.json"
        );
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/", "Gone\\": "gone/"}}}"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        assert_eq!(
            ComposerJson::analysis_paths(dir.path()),
            [dir.path().join("src")]
        );
    }

    #[test]
    fn scripts_accept_strings_and_lists() {
        let c = ComposerJson::parse(JSON).unwrap();
        let script = |n: &str| {
            c.scripts
                .iter()
                .find(|s| s.name == n)
                .unwrap()
                .commands
                .clone()
        };
        assert_eq!(script("test"), ["phpunit"]);
        assert_eq!(script("ci"), ["@test", "phpstan analyse"]);
    }

    #[test]
    fn missing_keys_are_empty_and_bad_json_is_an_error() {
        assert_eq!(ComposerJson::parse("{}").unwrap(), ComposerJson::default());
        assert!(matches!(
            ComposerJson::parse("[1]"),
            Err(ComposerError::Malformed(_))
        ));
        assert!(ComposerJson::parse("{").is_err());
    }

    #[test]
    fn lock_lists_packages_with_dev_flag() {
        let lock = ComposerLock::parse(
            r#"{"packages":[{"name":"a/b","version":"1.2.3"}],"packages-dev":[{"name":"c/d","version":"v2"},{"version":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(lock.packages.len(), 2);
        assert_eq!(lock.version_of("c/d"), Some("v2"));
        assert!(lock.packages[1].dev);
    }

    #[test]
    fn read_returns_none_without_a_file() {
        let dir = std::env::temp_dir().join("php-core-no-composer");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(ComposerJson::read(&dir).unwrap(), None);
        assert_eq!(ComposerLock::read(&dir).unwrap(), None);
    }
}
