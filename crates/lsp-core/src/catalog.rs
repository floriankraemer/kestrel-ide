//! Catalog of known language servers: the defaults we ship, layered over by
//! whatever the user configured.
//!
//! Same shape as `app_config::keymap::ACTIONS` — a const table of `Copy`
//! structs of `&'static str`, a lookup function, and unit tests as the
//! invariant guard. The `language_id` is the LSP language identifier (the
//! value sent as `textDocument/didOpen`'s `languageId`) and is the key both
//! for the catalog and for user overrides, so it must stay stable.

/// A known language server: which language it serves, what to run, and the
/// human-readable name the settings UI shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerDef {
    /// Stable server id, unique across the table. It is the language id for a
    /// language with one server, so the id a diagnostics source key or a
    /// user entry names does not change for the common case.
    pub id: &'static str,
    /// LSP language id, e.g. `"rust"`. Several rows may share one.
    pub language_id: &'static str,
    /// Display name, e.g. `"rust-analyzer"`.
    pub name: &'static str,
    /// Executable, looked up on `PATH`.
    pub command: &'static str,
    /// Arguments passed on every launch.
    pub args: &'static [&'static str],
    /// Whether this server's `publishDiagnostics` reach the Problems panel.
    pub diagnostics: bool,
    /// The server cannot run on native Windows (it still runs on WSL and in
    /// a container).
    pub posix_only: bool,
}

/// Default language servers, in the order they answer a `First` request. Nothing here is installed
/// by us — a missing executable simply means no server for that language.
pub const SERVERS: &[ServerDef] = &[
    ServerDef {
        id: "rust",
        language_id: "rust",
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "python",
        language_id: "python",
        name: "Pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "go",
        language_id: "go",
        name: "gopls",
        command: "gopls",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "c",
        language_id: "c",
        name: "clangd",
        command: "clangd",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "cpp",
        language_id: "cpp",
        name: "clangd",
        command: "clangd",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "typescript",
        language_id: "typescript",
        name: "TypeScript Language Server",
        command: "typescript-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "javascript",
        language_id: "javascript",
        name: "TypeScript Language Server",
        command: "typescript-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    // `typescriptreact` is a separate LSP language id, but the same server
    // handles it — it keys JSX parsing off the id it is told.
    ServerDef {
        id: "typescriptreact",
        language_id: "typescriptreact",
        name: "TypeScript Language Server",
        command: "typescript-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "json",
        language_id: "json",
        name: "JSON Language Server",
        command: "vscode-json-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "yaml",
        language_id: "yaml",
        name: "YAML Language Server",
        command: "yaml-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "bash",
        language_id: "bash",
        name: "Bash Language Server",
        command: "bash-language-server",
        args: &["start"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "lua",
        language_id: "lua",
        name: "lua-language-server",
        command: "lua-language-server",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "intelephense",
        language_id: "php",
        name: "Intelephense",
        command: "intelephense",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    // Runs beside Intelephense (ADR-0066). Its diagnostics are off by default
    // because Intelephense already reports them; Phpactor is the second
    // opinion for navigation, completion and refactoring.
    ServerDef {
        id: "phpactor",
        language_id: "php",
        name: "Phpactor",
        command: "phpactor",
        args: &["language-server"],
        diagnostics: false,
        posix_only: true,
    },
    ServerDef {
        id: "java",
        language_id: "java",
        name: "Eclipse JDT.LS",
        command: "jdtls",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "kotlin",
        language_id: "kotlin",
        name: "kotlin-language-server",
        command: "kotlin-language-server",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "swift",
        language_id: "swift",
        name: "SourceKit-LSP",
        command: "sourcekit-lsp",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "scala",
        language_id: "scala",
        name: "Metals",
        command: "metals",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "haskell",
        language_id: "haskell",
        name: "Haskell Language Server",
        command: "haskell-language-server-wrapper",
        args: &["--lsp"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "fsharp",
        language_id: "fsharp",
        name: "FsAutoComplete",
        command: "fsautocomplete",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "zig",
        language_id: "zig",
        name: "ZLS",
        command: "zls",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "ruby",
        language_id: "ruby",
        name: "Solargraph",
        command: "solargraph",
        args: &["stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "toml",
        language_id: "toml",
        name: "Taplo",
        command: "taplo",
        args: &["lsp", "stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "sql",
        language_id: "sql",
        name: "sqls",
        command: "sqls",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "html",
        language_id: "html",
        name: "HTML Language Server",
        command: "vscode-html-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "css",
        language_id: "css",
        name: "CSS Language Server",
        command: "vscode-css-language-server",
        args: &["--stdio"],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "xml",
        language_id: "xml",
        name: "Lemminx",
        command: "lemminx",
        args: &[],
        diagnostics: true,
        posix_only: false,
    },
    ServerDef {
        id: "dockerfile",
        language_id: "dockerfile",
        name: "Docker Language Server",
        command: "docker-language-server",
        args: &["start", "--stdio"],
        diagnostics: true,
        posix_only: false,
    },
];

/// The first shipped default for a language id, if we know one.
pub fn default_server(language_id: &str) -> Option<&'static ServerDef> {
    SERVERS.iter().find(|s| s.language_id == language_id)
}

/// Where a resolved server's definition came from — the shipped `SERVERS`
/// table, a plugin's `language-servers` contribution, or a user entry with
/// no catalog or plugin row underneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSource {
    Builtin,
    Plugin { plugin_id: String },
    User,
}

/// A resolved server launch configuration: a catalog default, a plugin
/// contribution, a user entry, or one of those with user fields applied on
/// top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Stable server id; the key the manager runs the server under.
    pub id: String,
    pub language_id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// A user may keep the entry but switch the server off.
    pub enabled: bool,
    /// The `workspace/configuration` section this server pulls its settings
    /// from, if any.
    pub settings_section: Option<String>,
    /// Default settings for `settings_section`, sent to the server as JSON.
    /// `Null` when the server takes no pulled configuration.
    pub settings: serde_json::Value,
    /// Sent as `initialize.initializationOptions`; `Null` sends none.
    pub initialization_options: serde_json::Value,
    /// Whether this server's diagnostics are shown.
    pub diagnostics: bool,
    /// Skipped on native Windows with a local host.
    pub posix_only: bool,
    /// Which host the server's process runs on.
    pub exec: ServerExec,
    /// Answer order among the servers of one language; lower answers first.
    /// `resolve_servers` numbers the resolved list.
    pub priority: usize,
    pub source: ServerSource,
}

/// Where a server's process runs when the project has an interpreter target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServerExec {
    /// The project's own host (`ExecHost::for_path`).
    #[default]
    Host,
    /// The configured interpreter's host, e.g. a container (ADR-0067).
    Interpreter,
}

impl From<&ServerDef> for ServerConfig {
    fn from(def: &ServerDef) -> Self {
        ServerConfig {
            id: def.id.to_string(),
            language_id: def.language_id.to_string(),
            name: def.name.to_string(),
            command: def.command.to_string(),
            args: def.args.iter().map(|a| a.to_string()).collect(),
            enabled: true,
            settings_section: None,
            settings: serde_json::Value::Null,
            initialization_options: serde_json::Value::Null,
            diagnostics: def.diagnostics,
            posix_only: def.posix_only,
            exec: ServerExec::Host,
            priority: 0,
            source: ServerSource::Builtin,
        }
    }
}

/// One language server a plugin offers, translated into the shape
/// `lsp-core` understands.
///
/// Deliberately not `plugin_api::LanguageServerContribution` — `lsp-core`
/// stays free of the plugin stack (`docs/architecture/layering.md`).
/// `ui-shell` and `settings-model`, which already depend on `plugin-host`
/// for other contribution points, map the contribution type into this one
/// at the call site.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginServer {
    /// The plugin that contributed this server, e.g. `"csharp"`.
    pub plugin_id: String,
    /// LSP language id this server serves, e.g. `"csharp"`.
    pub language_id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub settings_section: Option<String>,
    pub settings: serde_json::Value,
}

impl From<&PluginServer> for ServerConfig {
    fn from(plugin: &PluginServer) -> Self {
        ServerConfig {
            id: plugin.language_id.clone(),
            language_id: plugin.language_id.clone(),
            name: plugin.name.clone(),
            command: plugin.command.clone(),
            args: plugin.args.clone(),
            enabled: true,
            settings_section: plugin.settings_section.clone(),
            settings: plugin.settings.clone(),
            initialization_options: serde_json::Value::Null,
            diagnostics: true,
            posix_only: false,
            exec: ServerExec::Host,
            priority: 0,
            source: ServerSource::Plugin {
                plugin_id: plugin.plugin_id.clone(),
            },
        }
    }
}

/// What a user may say about one server. Every field but the language id is
/// optional: overriding only `enabled` must not wipe the shipped command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerOverride {
    /// The server this entry is about. Absent addresses the first server of
    /// `language_id`, which is how an entry written before a language had
    /// several servers keeps working.
    pub id: Option<String>,
    pub language_id: String,
    pub name: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub settings: Option<serde_json::Value>,
    pub initialization_options: Option<serde_json::Value>,
    pub diagnostics: Option<bool>,
    pub exec: Option<ServerExec>,
}

/// Merge plugin contributions and user entries over the shipped catalog,
/// low to high precedence: `SERVERS` -> `plugin_servers` -> `overrides`.
///
/// Servers are keyed by `id`; a language may have several, kept in catalog
/// order. A plugin entry for a language the const catalog already has
/// REPLACES every row of that language — it is a full alternate definition,
/// not a field-by-field patch like a [`ServerOverride`]. A plugin entry for a
/// language with no catalog row is appended. An override names its server by
/// `id`, or by language alone for the first server of that language; one for
/// an unknown server is appended in the order given and must carry a command
/// (without one there is nothing to launch, so it is dropped). Disabled
/// entries are kept so a settings page can show them — callers start only
/// the ones with `enabled`.
pub fn resolve_servers(
    overrides: &[ServerOverride],
    plugin_servers: &[PluginServer],
) -> Vec<ServerConfig> {
    let mut resolved: Vec<ServerConfig> = SERVERS.iter().map(ServerConfig::from).collect();

    for plugin in plugin_servers {
        let cfg = ServerConfig::from(plugin);
        match resolved
            .iter()
            .position(|c| c.language_id == cfg.language_id)
        {
            Some(first) => {
                resolved.retain(|c| c.language_id != cfg.language_id);
                resolved.insert(first.min(resolved.len()), cfg);
            }
            None => resolved.push(cfg),
        }
    }

    for ov in overrides {
        let target = resolved.iter_mut().find(|c| match &ov.id {
            Some(id) => &c.id == id,
            None => c.language_id == ov.language_id,
        });
        match target {
            Some(cfg) => apply(cfg, ov),
            None => {
                let Some(command) = ov.command.clone() else {
                    continue;
                };
                let mut cfg = ServerConfig {
                    id: ov.id.clone().unwrap_or_else(|| ov.language_id.clone()),
                    language_id: ov.language_id.clone(),
                    name: ov.name.clone().unwrap_or_else(|| command.clone()),
                    command,
                    args: Vec::new(),
                    enabled: true,
                    settings_section: None,
                    settings: serde_json::Value::Null,
                    initialization_options: serde_json::Value::Null,
                    diagnostics: true,
                    posix_only: false,
                    exec: ServerExec::Host,
                    priority: 0,
                    source: ServerSource::User,
                };
                apply(&mut cfg, ov);
                resolved.push(cfg);
            }
        }
    }
    for (priority, cfg) in resolved.iter_mut().enumerate() {
        cfg.priority = priority;
    }
    resolved
}

fn apply(cfg: &mut ServerConfig, ov: &ServerOverride) {
    if let Some(name) = &ov.name {
        cfg.name = name.clone();
    }
    if let Some(command) = &ov.command {
        cfg.command = command.clone();
    }
    if let Some(args) = &ov.args {
        cfg.args = args.clone();
    }
    if let Some(enabled) = ov.enabled {
        cfg.enabled = enabled;
    }
    if let Some(settings) = &ov.settings {
        cfg.settings = settings.clone();
    }
    if let Some(options) = &ov.initialization_options {
        cfg.initialization_options = options.clone();
    }
    if let Some(diagnostics) = ov.diagnostics {
        cfg.diagnostics = diagnostics;
    }
    if let Some(exec) = ov.exec {
        cfg.exec = exec;
    }
}

/// Which of the resolved servers, if any, may serve `language_id` first.
///
/// "May" is the rule: a disabled entry stays in `resolve_servers`' output so
/// a settings page can list it, and this is the single place that decides
/// callers must not launch it.
pub fn enabled_server<'a>(
    resolved: &'a [ServerConfig],
    language_id: &'a str,
) -> Option<&'a ServerConfig> {
    enabled_servers(resolved, language_id).next()
}

/// Every enabled server for `language_id`, in answer order.
pub fn enabled_servers<'a>(
    resolved: &'a [ServerConfig],
    language_id: &'a str,
) -> impl Iterator<Item = &'a ServerConfig> {
    resolved
        .iter()
        .filter(move |c| c.language_id == language_id && c.enabled)
}

/// Which of a language's enabled servers to launch, and which to skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub start: Vec<ServerConfig>,
    /// Servers left out, each with the reason to show the user.
    pub skipped: Vec<(ServerConfig, String)>,
}

/// Split `servers` into those to launch and those the platform rules out.
///
/// A `posix_only` server (Phpactor) cannot run on native Windows, so it is
/// skipped when `is_windows` and its process would run there: on a local
/// host, not inside WSL and not on an interpreter host such as a container.
/// `is_windows` is a parameter so the rule is testable on any platform.
pub fn launch_plan<'a>(
    servers: impl IntoIterator<Item = &'a ServerConfig>,
    host: &process_exec::host::ExecHost,
    is_windows: bool,
) -> LaunchPlan {
    let native_windows = is_windows && !host.is_remote();
    let mut plan = LaunchPlan {
        start: Vec::new(),
        skipped: Vec::new(),
    };
    for cfg in servers {
        if cfg.posix_only && native_windows && cfg.exec == ServerExec::Host {
            plan.skipped.push((
                cfg.clone(),
                format!(
                    "{} needs a POSIX system and does not run on native Windows. \
                     Open the project in WSL or run the server in a container.",
                    cfg.name
                ),
            ));
        } else {
            plan.start.push(cfg.clone());
        }
    }
    plan
}

/// Catalog language id -> LSP language id, for the few languages whose
/// protocol identifier is not their grammar id.
///
/// This is all that is left of what used to be a second extension table:
/// *which* language a file is belongs to `syntax-core`'s registry — the one
/// source of truth for file detection, extended with every language tranche
/// — while *what the protocol calls it* is genuinely LSP's business and
/// lives here. See ADR-0018.
const LSP_LANGUAGE_IDS: &[(&str, &str)] = &[
    // `.tsx` is its own grammar in the catalog (`tsx`); LSP names the JSX
    // dialect `typescriptreact`, and servers key JSX parsing off that.
    ("tsx", "typescriptreact"),
];

/// The LSP `languageId` for a catalog language id.
///
/// Identity for all but the handful of protocol divergences above, so an
/// unknown or future catalog id passes through unchanged rather than
/// vanishing — a language the catalog knows is never invisible here.
pub fn lsp_language_id(catalog_id: &str) -> &str {
    LSP_LANGUAGE_IDS
        .iter()
        .find(|(id, _)| *id == catalog_id)
        .map_or(catalog_id, |(_, lsp_id)| *lsp_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn server_ids_are_unique() {
        let mut seen = HashSet::new();
        for def in SERVERS {
            assert!(seen.insert(def.id), "duplicate server id {:?}", def.id);
        }
    }

    #[test]
    fn a_single_server_language_uses_its_language_id_as_server_id() {
        for def in SERVERS {
            let siblings = SERVERS
                .iter()
                .filter(|d| d.language_id == def.language_id)
                .count();
            if siblings == 1 {
                assert_eq!(def.id, def.language_id);
            }
        }
    }

    #[test]
    fn php_runs_intelephense_then_phpactor_with_phpactor_quiet_and_posix_only() {
        let resolved = resolve_servers(&[], &[]);
        let php: Vec<_> = enabled_servers(&resolved, "php").collect();
        assert_eq!(
            php.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["intelephense", "phpactor"]
        );
        assert_eq!(php[1].command, "phpactor");
        assert_eq!(php[1].args, ["language-server"]);
        assert!(php[0].diagnostics && !php[0].posix_only);
        assert!(!php[1].diagnostics && php[1].posix_only);
    }

    #[test]
    fn an_override_with_an_id_hits_that_server_and_one_without_hits_the_first() {
        let resolved = resolve_servers(
            &[
                ServerOverride {
                    id: Some("phpactor".into()),
                    language_id: "php".into(),
                    diagnostics: Some(true),
                    initialization_options: Some(serde_json::json!({"a": 1})),
                    ..Default::default()
                },
                ServerOverride {
                    language_id: "php".into(),
                    enabled: Some(false),
                    ..Default::default()
                },
            ],
            &[],
        );
        let by_id = |id: &str| resolved.iter().find(|c| c.id == id).unwrap();
        assert!(by_id("phpactor").diagnostics && by_id("phpactor").enabled);
        assert_eq!(by_id("phpactor").initialization_options["a"], 1);
        assert!(!by_id("intelephense").enabled);
    }

    #[test]
    fn a_plugin_entry_replaces_all_rows_of_its_language_in_place() {
        let plugin = PluginServer {
            plugin_id: "php-alt".into(),
            language_id: "php".into(),
            name: "Alt".into(),
            command: "alt".into(),
            args: vec![],
            settings_section: None,
            settings: serde_json::Value::Null,
        };
        let resolved = resolve_servers(&[], std::slice::from_ref(&plugin));
        assert_eq!(
            resolved.iter().filter(|c| c.language_id == "php").count(),
            1
        );
        assert_eq!(resolved.iter().filter(|c| c.id == "php").count(), 1);
    }

    #[test]
    fn every_entry_is_launchable_and_named() {
        for def in SERVERS {
            assert!(!def.language_id.is_empty());
            assert!(!def.name.is_empty(), "{} has no name", def.language_id);
            assert!(
                !def.command.is_empty(),
                "{} has no command",
                def.language_id
            );
        }
    }

    #[test]
    fn lookup_finds_defaults_and_misses_unknown() {
        assert_eq!(default_server("rust").unwrap().command, "rust-analyzer");
        assert!(default_server("brainfuck").is_none());
    }

    #[test]
    fn user_override_replaces_only_the_fields_it_names() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "rust".into(),
                command: Some("/opt/ra".into()),
                ..Default::default()
            }],
            &[],
        );
        let rust = resolved.iter().find(|c| c.language_id == "rust").unwrap();
        assert_eq!(rust.command, "/opt/ra");
        assert_eq!(rust.name, "rust-analyzer");
        assert!(rust.enabled);
        assert_eq!(resolved.len(), SERVERS.len());
    }

    #[test]
    fn user_can_disable_a_shipped_server() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "go".into(),
                enabled: Some(false),
                ..Default::default()
            }],
            &[],
        );
        let go = resolved.iter().find(|c| c.language_id == "go").unwrap();
        assert!(!go.enabled);
        assert_eq!(go.command, "gopls");
    }

    #[test]
    fn user_can_add_an_unknown_language() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "nim".into(),
                command: Some("nimlsp".into()),
                args: Some(vec!["--stdio".into()]),
                ..Default::default()
            }],
            &[],
        );
        let nim = resolved.iter().find(|c| c.language_id == "nim").unwrap();
        assert_eq!(nim.command, "nimlsp");
        assert_eq!(nim.args, ["--stdio"]);
        assert_eq!(nim.name, "nimlsp");
    }

    #[test]
    fn an_unknown_language_without_a_command_is_dropped() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "nim".into(),
                enabled: Some(true),
                ..Default::default()
            }],
            &[],
        );
        assert!(resolved.iter().all(|c| c.language_id != "nim"));
    }

    #[test]
    fn a_disabled_server_is_never_offered_for_launch() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "rust".into(),
                enabled: Some(false),
                ..Default::default()
            }],
            &[],
        );
        assert!(enabled_server(&resolved, "rust").is_none());
        assert_eq!(enabled_server(&resolved, "go").unwrap().command, "gopls");
        assert!(enabled_server(&resolved, "brainfuck").is_none());
    }

    #[test]
    fn protocol_ids_diverge_only_where_the_table_says_so() {
        assert_eq!(lsp_language_id("rust"), "rust");
        assert_eq!(lsp_language_id("tsx"), "typescriptreact");
        // A language with no shipped server still resolves to an id, so a
        // user-configured server for it can be found.
        assert_eq!(lsp_language_id("haskell"), "haskell");
    }

    /// The regression guard for issue #20: every language the editor can
    /// detect must be reachable by the server lookup, not just the dozen
    /// that once had rows in a hand-maintained extension table.
    #[test]
    fn every_catalog_language_is_visible_to_the_server_lookup() {
        for def in syntax_core::BUILTIN_LANGUAGES {
            let language_id = lsp_language_id(def.id).to_string();
            assert!(!language_id.is_empty(), "{} has no LSP id", def.id);

            let resolved = resolve_servers(
                &[ServerOverride {
                    language_id: language_id.clone(),
                    command: Some("some-server".into()),
                    ..Default::default()
                }],
                &[],
            );
            let found = enabled_server(&resolved, &language_id)
                .unwrap_or_else(|| panic!("{} resolves to no server", def.id));
            assert_eq!(found.command, "some-server");
        }
    }

    /// The other direction: a shipped server keyed by an id nothing can
    /// ever detect would never start.
    #[test]
    fn every_shipped_server_is_keyed_by_a_reachable_language_id() {
        for def in SERVERS {
            let reachable = syntax_core::BUILTIN_LANGUAGES
                .iter()
                .any(|l| lsp_language_id(l.id) == def.language_id);
            assert!(
                reachable,
                "no catalog language resolves to {:?}, so its server never starts",
                def.language_id
            );
        }
    }

    fn csharp_plugin() -> PluginServer {
        PluginServer {
            plugin_id: "csharp".into(),
            language_id: "csharp".into(),
            name: "csharp-ls".into(),
            command: "csharp-ls".into(),
            args: vec!["--loglevel".into(), "warning".into()],
            settings_section: Some("csharp".into()),
            settings: serde_json::json!({"analyzersEnabled": true}),
        }
    }

    #[test]
    fn a_plugin_entry_beats_the_const_catalog_row_for_the_same_language() {
        // `csharp` has no const-catalog row any more (it now comes from the
        // built-in plugin only), so this also proves a plugin entry is
        // reachable for a language the catalog itself never shipped.
        assert!(default_server("csharp").is_none());

        let resolved = resolve_servers(&[], &[csharp_plugin()]);
        let csharp = resolved.iter().find(|c| c.language_id == "csharp").unwrap();
        assert_eq!(csharp.command, "csharp-ls");
        assert_eq!(csharp.name, "csharp-ls");
        assert_eq!(csharp.settings_section.as_deref(), Some("csharp"));
        assert_eq!(
            csharp.source,
            ServerSource::Plugin {
                plugin_id: "csharp".into()
            }
        );
        assert!(enabled_server(&resolved, "csharp").is_some());
    }

    #[test]
    fn a_plugin_entry_replaces_a_const_row_wholesale_not_field_by_field() {
        let plugin = PluginServer {
            plugin_id: "rust-alt".into(),
            language_id: "rust".into(),
            name: "Alt Rust LS".into(),
            command: "alt-rust-ls".into(),
            args: vec![],
            settings_section: None,
            settings: serde_json::Value::Null,
        };
        let resolved = resolve_servers(&[], std::slice::from_ref(&plugin));
        let rust = resolved.iter().find(|c| c.language_id == "rust").unwrap();
        assert_eq!(rust.command, "alt-rust-ls");
        assert_eq!(rust.name, "Alt Rust LS");
        assert_eq!(
            rust.source,
            ServerSource::Plugin {
                plugin_id: "rust-alt".into()
            }
        );
        // Const row is gone entirely, not merged with the plugin's fields.
        assert_eq!(
            resolved.iter().filter(|c| c.language_id == "rust").count(),
            1
        );
    }

    #[test]
    fn a_user_override_still_beats_a_plugin_entry() {
        let resolved = resolve_servers(
            &[ServerOverride {
                language_id: "csharp".into(),
                command: Some("/opt/csharp-ls".into()),
                ..Default::default()
            }],
            &[csharp_plugin()],
        );
        let csharp = resolved.iter().find(|c| c.language_id == "csharp").unwrap();
        assert_eq!(csharp.command, "/opt/csharp-ls");
        // Fields the override didn't name stay whatever the plugin set.
        assert_eq!(csharp.name, "csharp-ls");
        assert_eq!(csharp.settings_section.as_deref(), Some("csharp"));
        assert_eq!(
            csharp.source,
            ServerSource::Plugin {
                plugin_id: "csharp".into()
            }
        );
    }

    fn php_servers() -> Vec<ServerConfig> {
        enabled_servers(&resolve_servers(&[], &[]), "php")
            .cloned()
            .collect::<Vec<_>>()
    }

    #[test]
    fn on_native_windows_a_posix_only_server_is_skipped_with_a_reason() {
        let servers = php_servers();
        let plan = launch_plan(&servers, &process_exec::host::ExecHost::Local, true);
        assert_eq!(
            plan.start.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["intelephense"]
        );
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].0.id, "phpactor");
        assert!(
            plan.skipped[0].1.contains("Phpactor"),
            "{}",
            plan.skipped[0].1
        );
        assert!(plan.skipped[0].1.contains("WSL"), "{}", plan.skipped[0].1);
    }

    #[test]
    fn everything_starts_off_windows() {
        let servers = php_servers();
        let plan = launch_plan(&servers, &process_exec::host::ExecHost::Local, false);
        assert_eq!(plan.start.len(), 2);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn a_posix_only_server_runs_in_wsl_and_on_an_interpreter_host() {
        let wsl = process_exec::host::ExecHost::Wsl(process_exec::host::WslHost {
            distro: "Ubuntu".into(),
            unc_prefix: "//wsl.localhost/Ubuntu".into(),
        });
        let servers = php_servers();
        assert!(launch_plan(&servers, &wsl, true).skipped.is_empty());

        let in_container: Vec<_> = servers
            .iter()
            .cloned()
            .map(|c| ServerConfig {
                exec: ServerExec::Interpreter,
                ..c
            })
            .collect();
        let local = process_exec::host::ExecHost::Local;
        assert!(launch_plan(&in_container, &local, true).skipped.is_empty());
    }
}
