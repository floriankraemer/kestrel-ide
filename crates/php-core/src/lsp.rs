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

use lsp_core::ServerConfig;
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

/// The server ids [`apply`] configures.
pub const INTELEPHENSE_ID: &str = "intelephense";
pub const PHPACTOR_ID: &str = "phpactor";

/// A server's `[php.servers.<id>]` toggles; `None` keeps what the resolved
/// configuration already says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    pub enabled: Option<bool>,
    pub diagnostics: Option<bool>,
}

/// Lay the `[php]` settings over the resolved server configs of the two PHP
/// servers: their toggles, initialization options and (Intelephense) pulled
/// settings. Anything the user wrote into a `[[language_server]]` table
/// stays on top of what is derived, and a `[[language_server]]` that
/// disables a server is not undone by `[php]`.
pub fn apply(configs: &mut [ServerConfig], input: &LspInput, toggles: impl Fn(&str) -> Toggles) {
    for cfg in configs.iter_mut() {
        let json = match cfg.id.as_str() {
            INTELEPHENSE_ID => intelephense(input),
            PHPACTOR_ID => phpactor(input),
            _ => continue,
        };
        let toggle = toggles(&cfg.id);
        cfg.enabled &= toggle.enabled.unwrap_or(true);
        cfg.diagnostics = toggle.diagnostics.unwrap_or(cfg.diagnostics);
        cfg.initialization_options = merge_user(
            json.initialization_options,
            &std::mem::take(&mut cfg.initialization_options),
        );
        if cfg.id == INTELEPHENSE_ID {
            cfg.settings_section
                .get_or_insert_with(|| INTELEPHENSE_SECTION.to_string());
            cfg.settings = merge_user(json.settings, &std::mem::take(&mut cfg.settings));
        }
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

    fn php_configs() -> Vec<ServerConfig> {
        lsp_core::resolve_servers(&[], &[])
            .into_iter()
            .filter(|c| c.language_id == "php")
            .collect()
    }

    #[test]
    fn apply_configures_both_servers_and_their_toggles() {
        let mut configs = php_configs();
        let paths = vec!["lib".to_string()];
        apply(&mut configs, &input(Path::new("/p"), &paths), |id| {
            Toggles {
                enabled: (id == PHPACTOR_ID).then_some(false),
                diagnostics: (id == PHPACTOR_ID).then_some(true),
            }
        });
        let by_id = |id: &str| configs.iter().find(|c| c.id == id).unwrap();
        let intelephense = by_id(INTELEPHENSE_ID);
        assert!(intelephense.enabled && intelephense.diagnostics);
        assert_eq!(
            intelephense.settings_section.as_deref(),
            Some("intelephense")
        );
        assert_eq!(intelephense.initialization_options["licenceKey"], "KEY123");
        assert_eq!(
            intelephense.settings["environment"]["includePaths"],
            json!(["/p/lib"])
        );
        let phpactor = by_id(PHPACTOR_ID);
        assert!(!phpactor.enabled);
        assert!(phpactor.diagnostics);
        assert_eq!(phpactor.initialization_options["php.version"], "8.3");
    }

    #[test]
    fn a_user_disable_and_user_keys_survive() {
        let mut configs = php_configs();
        configs[0].enabled = false;
        configs[0].settings = json!({"environment": {"phpVersion": "7.4.0"}});
        apply(&mut configs, &input(Path::new("/p"), &[]), |_| Toggles {
            enabled: Some(true),
            diagnostics: None,
        });
        assert!(!configs[0].enabled);
        assert_eq!(configs[0].settings["environment"]["phpVersion"], "7.4.0");
    }

    #[test]
    fn a_changed_level_is_a_settings_push_and_a_changed_licence_a_restart() {
        use lsp_core::{reload_kind, ReloadKind};
        let build = |level: &str, key: &str| {
            let mut c = php_configs();
            let mut i = input(Path::new("/p"), &[]);
            i.language_level = Some(level.parse().unwrap());
            i.licence_key = Some(key);
            apply(&mut c, &i, |_| Toggles::default());
            c.remove(0)
        };
        let base = build("8.2", "A");
        assert_eq!(
            reload_kind(&base, &build("8.3", "A")),
            ReloadKind::PushSettings
        );
        assert_eq!(reload_kind(&base, &build("8.2", "B")), ReloadKind::Restart);
        assert_eq!(
            reload_kind(&base, &build("8.2", "A")),
            ReloadKind::Unchanged
        );
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
