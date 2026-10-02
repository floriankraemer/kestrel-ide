//! What each PHP language server is told about the `[php]` settings: the
//! initialization options and the pulled settings, as JSON.
//!
//! Key names come from the servers' own documentation:
//! - Intelephense: initialization options `licenceKey`, `storagePath`
//!   (<https://github.com/bmewburn/intelephense-docs/blob/master/installation.md>)
//!   and the `intelephense` configuration section's
//!   `environment.phpVersion`, `environment.includePaths` and `stubs`.
//! - Phpactor: <https://phpactor.readthedocs.io/en/master/reference/configuration.html>,
//!   `php.version` and the four `*.enabled` switches of the analyzers the IDE
//!   runs itself.
//!
//! `lsp-core` stays language-agnostic: it receives these as plain JSON in a
//! `ServerConfig`.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::level::LanguageLevel;

/// The `workspace/configuration` section Intelephense pulls.
pub const INTELEPHENSE_SECTION: &str = "intelephense";

/// The `[php]` values the servers care about.
#[derive(Debug, Clone)]
pub struct LspInput<'a> {
    pub language_level: Option<LanguageLevel>,
    /// Project-relative or absolute.
    pub include_paths: &'a [String],
    /// `None` leaves the server's own stub list.
    pub stubs: Option<&'a [String]>,
    pub licence_key: Option<&'a str>,
    /// Where Intelephense keeps its index; `None` is its default.
    pub storage_path: Option<&'a Path>,
    pub project_root: &'a Path,
}

/// One server's JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerJson {
    /// `Null` sends no `initializationOptions`.
    pub initialization_options: Value,
    /// What the server pulls for its settings section; `Null` when it
    /// pulls none.
    pub settings: Value,
}

pub fn intelephense(input: &LspInput) -> ServerJson {
    let mut init = Map::new();
    if let Some(key) = input.licence_key.filter(|k| !k.trim().is_empty()) {
        init.insert("licenceKey".into(), json!(key.trim()));
    }
    if let Some(path) = input.storage_path {
        init.insert("storagePath".into(), json!(path.to_string_lossy()));
    }

    let mut environment = Map::new();
    if let Some(level) = input.language_level {
        // Intelephense wants a full version string.
        environment.insert("phpVersion".into(), json!(format!("{level}.0")));
    }
    if !input.include_paths.is_empty() {
        let paths: Vec<String> = input
            .include_paths
            .iter()
            .map(|p| absolute(input.project_root, p))
            .collect();
        environment.insert("includePaths".into(), json!(paths));
    }
    let mut settings = Map::new();
    if !environment.is_empty() {
        settings.insert("environment".into(), Value::Object(environment));
    }
    if let Some(stubs) = input.stubs {
        settings.insert("stubs".into(), json!(stubs));
    }
    ServerJson {
        initialization_options: object_or_null(init),
        // Always an object, so adding the first setting later is a
        // settings push and not a restart.
        settings: Value::Object(settings),
    }
}

pub fn phpactor(input: &LspInput) -> ServerJson {
    // The IDE runs PHPStan, Psalm, PHPCS and php-cs-fixer itself as
    // analyzers and formatters; Phpactor's own copies would report twice.
    let mut init = Map::new();
    for key in [
        "language_server_phpstan.enabled",
        "language_server_psalm.enabled",
        "php_code_sniffer.enabled",
        "language_server_php_cs_fixer.enabled",
    ] {
        init.insert(key.into(), json!(false));
    }
    if let Some(level) = input.language_level {
        init.insert("php.version".into(), json!(level.to_string()));
    }
    ServerJson {
        initialization_options: Value::Object(init),
        settings: Value::Null,
    }
}

/// `derived` with `user` laid over it, object keys merged recursively and
/// any other value replaced. A hand-written `[[language_server]]` table
/// therefore wins over what the `[php]` page derived.
pub fn merge_user(derived: Value, user: &Value) -> Value {
    match (derived, user) {
        (Value::Object(mut base), Value::Object(over)) => {
            for (key, value) in over {
                let merged = match base.remove(key) {
                    Some(existing) => merge_user(existing, value),
                    None => value.clone(),
                };
                base.insert(key.clone(), merged);
            }
            Value::Object(base)
        }
        (derived, Value::Null) => derived,
        (_, user) => user.clone(),
    }
}

fn object_or_null(map: Map<String, Value>) -> Value {
    if map.is_empty() {
        Value::Null
    } else {
        Value::Object(map)
    }
}

fn absolute(root: &Path, path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        root.join(p).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(root: &'a Path, paths: &'a [String]) -> LspInput<'a> {
        LspInput {
            language_level: Some("8.3".parse().unwrap()),
            include_paths: paths,
            stubs: None,
            licence_key: Some("KEY123"),
            storage_path: None,
            project_root: root,
        }
    }

    #[test]
    fn intelephense_golden() {
        let paths = vec!["vendor/lib".to_string(), "/abs/inc".to_string()];
        let stubs = vec!["redis".to_string()];
        let mut i = input(Path::new("/p"), &paths);
        i.stubs = Some(&stubs);
        i.storage_path = Some(Path::new("/cache/intelephense"));
        let json = intelephense(&i);
        assert_eq!(
            json.initialization_options,
            json!({"licenceKey": "KEY123", "storagePath": "/cache/intelephense"})
        );
        assert_eq!(
            json.settings,
            json!({
                "environment": {
                    "phpVersion": "8.3.0",
                    "includePaths": ["/p/vendor/lib", "/abs/inc"]
                },
                "stubs": ["redis"]
            })
        );
    }

    #[test]
    fn intelephense_with_nothing_set_sends_no_options_and_an_empty_object() {
        let json = intelephense(&LspInput {
            language_level: None,
            include_paths: &[],
            stubs: None,
            licence_key: None,
            storage_path: None,
            project_root: Path::new("/p"),
        });
        assert_eq!(json.initialization_options, Value::Null);
        assert_eq!(json.settings, json!({}));
    }

    #[test]
    fn a_blank_licence_key_is_not_sent() {
        let mut i = input(Path::new("/p"), &[]);
        i.licence_key = Some("  ");
        assert_eq!(intelephense(&i).initialization_options, Value::Null);
    }

    #[test]
    fn phpactor_golden() {
        let json = phpactor(&input(Path::new("/p"), &[]));
        assert_eq!(
            json.initialization_options,
            json!({
                "language_server_phpstan.enabled": false,
                "language_server_psalm.enabled": false,
                "php_code_sniffer.enabled": false,
                "language_server_php_cs_fixer.enabled": false,
                "php.version": "8.3"
            })
        );
        assert_eq!(json.settings, Value::Null);
    }

    #[test]
    fn user_keys_win_and_objects_merge() {
        let derived = json!({"environment": {"phpVersion": "8.3.0", "includePaths": ["/a"]}});
        let user = json!({"environment": {"phpVersion": "7.4.0"}, "extra": 1});
        assert_eq!(
            merge_user(derived.clone(), &user),
            json!({"environment": {"phpVersion": "7.4.0", "includePaths": ["/a"]}, "extra": 1})
        );
        assert_eq!(merge_user(derived.clone(), &Value::Null), derived);
    }
}
