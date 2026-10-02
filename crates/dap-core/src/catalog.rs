//! Which debug adapter to start, and how (D1-4).
//!
//! Shaped like `lsp_core::catalog`: a shipped table of adapters, each with
//! the command that starts it and the install hint to show when it is not
//! there, layered under whatever the project's settings override. The
//! default for a project comes from `run_core::toolchain` — which adapter a
//! toolchain implies is that table's answer (ADR-0039), not a second one.

use std::path::{Path, PathBuf};

use app_config::DebugAdapterSetting;
use run_core::ToolchainId;

/// One adapter: what to run, and what to say when it is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    /// Stable id, as `run_core::ToolchainId::debug_adapter` spells it and as
    /// a settings file refers to it.
    pub id: String,
    pub program: String,
    pub args: Vec<String>,
    /// Shown verbatim when the program cannot be started. An adapter that is
    /// not installed is the single most likely failure of this whole
    /// feature, so the message says what to install rather than reporting
    /// "No such file or directory".
    pub install_hint: String,
}

/// The adapters this IDE ships knowledge of. None of them is bundled: each
/// is a program the user installs, exactly as language servers are.
pub fn shipped() -> Vec<Adapter> {
    vec![
        Adapter {
            id: "codelldb".into(),
            program: "codelldb".into(),
            args: vec!["--port".into(), "0".into()],
            install_hint:
                "Install the CodeLLDB adapter (vadimcn.vscode-lldb) and put `codelldb` on PATH."
                    .into(),
        },
        Adapter {
            id: "debugpy".into(),
            program: "python3".into(),
            args: vec!["-m".into(), "debugpy.adapter".into()],
            install_hint: "Install debugpy: `python3 -m pip install debugpy`.".into(),
        },
        Adapter {
            id: "java-debug".into(),
            program: "java-debug-adapter".into(),
            args: Vec::new(),
            install_hint:
                "Install the Java debug adapter (microsoft/java-debug) and put its launcher on PATH."
                    .into(),
        },
        Adapter {
            id: PHP_DEBUG.into(),
            program: "node".into(),
            args: vec![php_debug_script()],
            install_hint: "Install the PHP Debug extension (xdebug.php-debug) in VS Code or a \
                           compatible editor so its `phpDebug.js` can be found, or point a \
                           `[[debug_adapter]]` with id \"php-debug\" at `node` and your copy."
                .into(),
        },
    ]
}

/// The catalog id of vscode-php-debug (ADR-0069).
pub const PHP_DEBUG: &str = "php-debug";

/// Editor extension folders under the home directory that may hold the
/// `xdebug.php-debug` extension.
const EXTENSION_HOMES: &[&str] = &[
    ".vscode",
    ".vscode-server",
    ".vscode-insiders",
    ".vscode-oss",
    ".cursor",
    ".windsurf",
];

const PHP_DEBUG_PREFIX: &str = "xdebug.php-debug-";

/// `phpDebug.js` of the newest installed `xdebug.php-debug` extension under
/// `home`, if any.
pub fn locate_php_debug(home: &Path) -> Option<PathBuf> {
    EXTENSION_HOMES
        .iter()
        .filter_map(|editor| std::fs::read_dir(home.join(editor).join("extensions")).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = extension_version(name.strip_prefix(PHP_DEBUG_PREFIX)?);
            let script = entry.path().join("out").join("phpDebug.js");
            script.is_file().then_some((version, script))
        })
        .max()
        .map(|(_, script)| script)
}

/// The `sh` script that lists every `phpDebug.js` under the *distro's* home,
/// one per line — [`locate_php_debug`] for a WSL project, whose adapter runs
/// inside the distro and cannot read the IDE host's extensions.
fn wsl_listing_script() -> String {
    let globs: Vec<String> = EXTENSION_HOMES
        .iter()
        .map(|editor| format!("\"$HOME\"/{editor}/extensions/{PHP_DEBUG_PREFIX}*/out/phpDebug.js"))
        .collect();
    format!("ls -1 {} 2>/dev/null", globs.join(" "))
}

/// The newest `phpDebug.js` in [`wsl_listing_script`]'s output.
fn newest_listed(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let extension = Path::new(line).parent()?.parent()?.file_name()?.to_str()?;
            let version = extension_version(extension.strip_prefix(PHP_DEBUG_PREFIX)?);
            Some((version, line.to_string()))
        })
        .max()
        .map(|(_, script)| script)
}

/// [`locate_php_debug`] inside the WSL distro `host` names; `None` on any
/// other host, or when the distro has no copy.
fn locate_php_debug_on(host: &process_exec::host::ExecHost, cwd: &Path) -> Option<String> {
    if !matches!(host, process_exec::host::ExecHost::Wsl(_)) {
        return None;
    }
    let script = wsl_listing_script();
    let out = process_exec::run_on(
        host,
        "sh",
        &["-c", &script],
        cwd,
        None,
        std::time::Duration::from_secs(10),
        &[],
    )
    .ok()?;
    newest_listed(&String::from_utf8_lossy(&out.stdout))
}

/// `1.36.0` out of `1.36.0-linux-x64`, as comparable numbers.
fn extension_version(suffix: &str) -> Vec<u64> {
    suffix
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// The script argument for the shipped `php-debug` row: the located file, or
/// the bare name, which makes `node` fail and the install hint show.
fn php_debug_script() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .and_then(|home| locate_php_debug(Path::new(&home)))
        .map(|script| script.display().to_string())
        .unwrap_or_else(|| "phpDebug.js".to_string())
}

/// [`resolve`] for an adapter that runs on `host` (the project's). The PHP
/// adapter of a WSL project is looked up in the distro's own home and run
/// there with its Linux path, not the IDE host's `$HOME` copy.
///
/// ponytail: one blocking `wsl.exe` call on a WSL project's first PHP debug
/// start; cache per distro if it is ever felt.
pub fn resolve_on(
    id: &str,
    overrides: &[DebugAdapterSetting],
    host: &process_exec::host::ExecHost,
    cwd: &Path,
) -> Option<Adapter> {
    let mut adapter = resolve(id, overrides)?;
    let overridden_args = overrides
        .iter()
        .any(|setting| setting.id == id && setting.args.is_some());
    if id == PHP_DEBUG && !overridden_args && host.runs_remotely() {
        adapter.args =
            vec![locate_php_debug_on(host, cwd).unwrap_or_else(|| "phpDebug.js".to_string())];
    }
    Some(adapter)
}

/// The adapter for `id`, with any project override applied.
///
/// An override may replace the program and arguments of a shipped adapter,
/// or introduce an adapter the shipped table has never heard of — the same
/// two jobs `[[language_server]]` does for LSP.
pub fn resolve(id: &str, overrides: &[DebugAdapterSetting]) -> Option<Adapter> {
    let shipped = shipped().into_iter().find(|adapter| adapter.id == id);
    let overridden = overrides.iter().find(|setting| setting.id == id);

    match (shipped, overridden) {
        (Some(adapter), None) => Some(adapter),
        (Some(adapter), Some(setting)) => Some(Adapter {
            program: setting.command.clone().unwrap_or(adapter.program),
            args: setting.args.clone().unwrap_or(adapter.args),
            ..adapter
        }),
        (None, Some(setting)) => setting.command.clone().map(|program| Adapter {
            id: id.to_string(),
            program,
            args: setting.args.clone().unwrap_or_default(),
            install_hint: String::new(),
        }),
        (None, None) => None,
    }
}

/// The adapter a project's programs are debugged with by default: the one
/// its toolchain implies.
pub fn for_toolchain(toolchain: ToolchainId, overrides: &[DebugAdapterSetting]) -> Option<Adapter> {
    resolve(toolchain.debug_adapter()?, overrides)
}

/// Whether this adapter can redefine a running program's classes — "reload
/// changed classes" (D4-4).
///
/// Keyed by adapter rather than read from `Capabilities`, because the
/// specification has no flag for it: the JVM can do this through JDWP and
/// java-debug exposes it as a custom `redefineClasses` request, while
/// neither codelldb nor debugpy has an equivalent to advertise. This is the
/// one place that asymmetry is written down, so the view can disable an
/// action it must not offer without knowing why.
pub fn supports_class_reload(adapter_id: &str) -> bool {
    adapter_id == "java-debug"
}

/// The request that performs it. Custom, not DAP: see
/// [`supports_class_reload`].
pub const CLASS_RELOAD_REQUEST: &str = "redefineClasses";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_adapter_has_an_install_hint() {
        for adapter in shipped() {
            assert!(
                !adapter.install_hint.is_empty(),
                "{} would report only an OS error",
                adapter.id
            );
            assert!(!adapter.program.is_empty());
        }
    }

    #[test]
    fn each_planned_toolchain_resolves_to_an_adapter() {
        for toolchain in [
            ToolchainId::Cargo,
            ToolchainId::Cmake,
            ToolchainId::Python,
            ToolchainId::Maven,
            ToolchainId::Gradle,
            ToolchainId::Php,
        ] {
            assert!(
                for_toolchain(toolchain, &[]).is_some(),
                "{toolchain:?} has no adapter"
            );
        }
        assert!(for_toolchain(ToolchainId::Make, &[]).is_none());
    }

    #[test]
    fn an_override_replaces_the_command_and_keeps_the_hint() {
        let overrides = vec![DebugAdapterSetting {
            id: "codelldb".into(),
            command: Some("/opt/codelldb".into()),
            args: Some(vec!["--stdio".into()]),
        }];
        let adapter = resolve("codelldb", &overrides).unwrap();
        assert_eq!(adapter.program, "/opt/codelldb");
        assert_eq!(adapter.args, vec!["--stdio"]);
        assert!(!adapter.install_hint.is_empty());
    }

    #[test]
    fn an_override_may_introduce_an_adapter_we_never_shipped() {
        let overrides = vec![DebugAdapterSetting {
            id: "delve".into(),
            command: Some("dlv".into()),
            args: Some(vec!["dap".into()]),
        }];
        let adapter = resolve("delve", &overrides).unwrap();
        assert_eq!(adapter.program, "dlv");
    }

    #[test]
    fn an_override_with_no_command_cannot_conjure_an_adapter() {
        let overrides = vec![DebugAdapterSetting {
            id: "delve".into(),
            command: None,
            args: None,
        }];
        assert!(resolve("delve", &overrides).is_none());
    }

    fn install(home: &Path, editor: &str, folder: &str, with_script: bool) -> PathBuf {
        let out = home
            .join(editor)
            .join("extensions")
            .join(folder)
            .join("out");
        std::fs::create_dir_all(&out).unwrap();
        let script = out.join("phpDebug.js");
        if with_script {
            std::fs::write(&script, "").unwrap();
        }
        script
    }

    #[test]
    fn php_debug_is_found_in_the_newest_extension_of_any_editor() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), ".vscode", "xdebug.php-debug-1.9.0", true);
        let newest = install(
            home.path(),
            ".cursor",
            "xdebug.php-debug-1.36.0-linux-x64",
            true,
        );
        install(
            home.path(),
            ".vscode-server",
            "xdebug.php-debug-1.40.0",
            false,
        );
        install(home.path(), ".vscode", "ms-python.python-2025.1.0", true);
        assert_eq!(locate_php_debug(home.path()), Some(newest));
    }

    #[test]
    fn a_wsl_listing_picks_the_newest_extension_in_the_distro() {
        let listing = "/home/f/.vscode-server/extensions/xdebug.php-debug-1.9.0/out/phpDebug.js\n\
                       /home/f/.cursor/extensions/xdebug.php-debug-1.36.0-linux-x64/out/phpDebug.js\n";
        assert_eq!(
            newest_listed(listing).as_deref(),
            Some("/home/f/.cursor/extensions/xdebug.php-debug-1.36.0-linux-x64/out/phpDebug.js")
        );
        assert_eq!(newest_listed(""), None);
    }

    #[test]
    fn the_wsl_script_searches_the_distros_home_for_every_editor() {
        let script = wsl_listing_script();
        assert!(script
            .contains("\"$HOME\"/.vscode-server/extensions/xdebug.php-debug-*/out/phpDebug.js"));
        assert!(script.contains(".cursor"));
    }

    #[test]
    fn a_local_host_keeps_the_host_home_lookup() {
        let adapter = resolve_on(
            PHP_DEBUG,
            &[],
            &process_exec::host::ExecHost::Local,
            Path::new("/p"),
        )
        .unwrap();
        assert_eq!(adapter, resolve(PHP_DEBUG, &[]).unwrap());
    }

    #[test]
    fn php_debug_is_not_found_in_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(locate_php_debug(home.path()), None);
    }

    #[test]
    fn php_debug_runs_under_node_and_can_be_overridden() {
        let adapter = resolve(PHP_DEBUG, &[]).unwrap();
        assert_eq!(adapter.program, "node");
        assert_eq!(adapter.args.len(), 1);
        let overrides = vec![DebugAdapterSetting {
            id: PHP_DEBUG.into(),
            command: None,
            args: Some(vec!["/opt/phpDebug.js".into()]),
        }];
        assert_eq!(
            resolve(PHP_DEBUG, &overrides).unwrap().args,
            vec!["/opt/phpDebug.js"]
        );
    }

    #[test]
    fn an_unknown_id_resolves_to_nothing() {
        assert!(resolve("nonesuch", &[]).is_none());
    }
}
