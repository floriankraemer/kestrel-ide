//! PHP run kinds (PHP parity plan, I5): `php-builtin-server` (`php -S`) and
//! `php-console` (PsySH when the project has it, otherwise `php -a`).
//!
//! Both compile to an ordinary process configuration, so they flow through
//! the plain-process path in [`crate::config`] and inherit whatever applies
//! to it (run targets, before-launch tasks, macros). The program is the
//! `[php]` interpreter carried by [`MacroContext::php_interpreter`].

use std::path::Path;

use app_config::container_run::PortBinding;
use app_config::php::PhpBuiltinServerRunSetting;

use crate::config::RunConfig;
use crate::macros::MacroContext;
use crate::toolchain::{ToolchainId, DEFAULT_PHP_PROGRAM};

pub const KIND_BUILTIN_SERVER: &str = "php-builtin-server";
pub const KIND_CONSOLE: &str = "php-console";

pub const DEFAULT_HOST: &str = "localhost";
pub const DEFAULT_PORT: u16 = 8000;
const DEFAULT_DOCUMENT_ROOT: &str = "$PROJECT_DIR$";
/// Where Composer installs PsySH's launcher, relative to the project root.
const PSYSH: &str = "vendor/bin/psysh";

/// `config` as a plain process configuration when its kind is one of the
/// PHP kinds; `None` for every other kind.
///
/// ponytail: only a project-local `vendor/bin/psysh` is looked for. A
/// globally installed PsySH is not on this path because finding it means
/// searching `PATH` on whichever host runs PHP; add that probe if asked.
pub fn materialize(
    config: &RunConfig,
    context: &MacroContext,
    in_container: bool,
) -> Option<RunConfig> {
    let program = context
        .php_interpreter
        .clone()
        .unwrap_or_else(|| DEFAULT_PHP_PROGRAM.to_string());
    let args = match config.kind.as_deref()? {
        KIND_BUILTIN_SERVER => {
            server_args(&config.php_server.clone().unwrap_or_default(), in_container)
        }
        KIND_CONSOLE => console_args(context.project_root.as_deref()),
        _ => return None,
    };
    Some(RunConfig {
        program,
        args,
        toolchain: Some(ToolchainId::Php.as_str().to_string()),
        ..config.clone()
    })
}

/// Inside a container the server must listen on every interface, or the
/// published port reaches nothing.
const CONTAINER_BIND_HOST: &str = "0.0.0.0";

fn server_port(server: &PhpBuiltinServerRunSetting) -> u16 {
    if server.port == 0 {
        DEFAULT_PORT
    } else {
        server.port
    }
}

/// The port a `php-builtin-server` configuration must publish when it runs
/// in a container; nothing for any other kind.
pub fn published_ports(config: &RunConfig) -> Vec<PortBinding> {
    if config.kind.as_deref() != Some(KIND_BUILTIN_SERVER) {
        return Vec::new();
    }
    let port = server_port(&config.php_server.clone().unwrap_or_default()).to_string();
    vec![PortBinding {
        host_port: port.clone(),
        container_port: port,
        ..PortBinding::default()
    }]
}

/// Whether `config` runs PHP: the PHP toolchain, or one of the PHP kinds.
pub fn is_php_config(config: &RunConfig) -> bool {
    config.toolchain.as_deref() == Some(ToolchainId::Php.as_str())
        || matches!(
            config.kind.as_deref(),
            Some(KIND_BUILTIN_SERVER | KIND_CONSOLE)
        )
}

/// `config` with the interpreter's container target as its `run_on`, when
/// it is a PHP configuration (the PHP toolchain, or one of the PHP kinds)
/// that names no `run_on` of its own — an explicit `local` or another
/// target is the user's choice and stays. `None`/blank `target` changes
/// nothing.
pub fn inherit_container_target(config: &RunConfig, target: Option<&str>) -> RunConfig {
    let mut config = config.clone();
    if let Some(id) = target.map(str::trim).filter(|id| !id.is_empty()) {
        if is_php_config(&config) && config.run_on.is_none() {
            config.run_on = Some(format!("container:{id}"));
        }
    }
    config
}

fn server_args(server: &PhpBuiltinServerRunSetting, in_container: bool) -> Vec<String> {
    let host = if in_container {
        CONTAINER_BIND_HOST
    } else {
        non_blank(&server.host).unwrap_or(DEFAULT_HOST)
    };
    let port = server_port(server);
    let mut args = vec![
        "-S".to_string(),
        format!("{host}:{port}"),
        "-t".to_string(),
        non_blank(&server.document_root)
            .unwrap_or(DEFAULT_DOCUMENT_ROOT)
            .to_string(),
    ];
    args.extend(non_blank(&server.router).map(str::to_string));
    args
}

fn console_args(project_root: Option<&Path>) -> Vec<String> {
    if project_root.is_some_and(|root| root.join(PSYSH).is_file()) {
        vec![PSYSH.to_string()]
    } else {
        vec!["-a".to_string()]
    }
}

fn non_blank(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RunConfigExt;

    fn server(setting: PhpBuiltinServerRunSetting) -> RunConfig {
        RunConfig {
            kind: Some(KIND_BUILTIN_SERVER.into()),
            php_server: Some(setting),
            ..RunConfig::default()
        }
    }

    #[test]
    fn a_server_with_nothing_set_listens_on_localhost_8000_from_the_project_root() {
        let spec = server(Default::default()).to_launch_spec(Path::new("/p"));
        assert_eq!(spec.program, "php");
        assert_eq!(spec.args, ["-S", "localhost:8000", "-t", "/p"]);
    }

    #[test]
    fn a_server_uses_the_interpreter_docroot_and_router() {
        let config = server(PhpBuiltinServerRunSetting {
            host: "0.0.0.0".into(),
            port: 8081,
            document_root: "$PROJECT_DIR$/public".into(),
            router: "router.php".into(),
        });
        let context = MacroContext::for_project("/p").with_php_interpreter("/opt/php/bin/php");
        let spec = config.to_launch_spec_in(&context);
        assert_eq!(spec.program, "/opt/php/bin/php");
        assert_eq!(
            spec.args,
            ["-S", "0.0.0.0:8081", "-t", "/p/public", "router.php"]
        );
    }

    #[test]
    fn the_console_prefers_a_project_psysh() {
        let dir = tempfile::tempdir().unwrap();
        let config = RunConfig {
            kind: Some(KIND_CONSOLE.into()),
            ..RunConfig::default()
        };
        assert_eq!(config.to_launch_spec(dir.path()).args, ["-a"]);
        std::fs::create_dir_all(dir.path().join("vendor/bin")).unwrap();
        std::fs::write(dir.path().join(PSYSH), "").unwrap();
        assert_eq!(config.to_launch_spec(dir.path()).args, [PSYSH]);
    }

    #[test]
    fn other_kinds_are_left_alone() {
        assert!(materialize(&RunConfig::default(), &MacroContext::default(), false).is_none());
    }

    #[test]
    fn in_a_container_the_server_listens_on_every_interface_and_publishes_its_port() {
        let config = server(PhpBuiltinServerRunSetting {
            host: "localhost".into(),
            port: 8081,
            ..Default::default()
        });
        let materialized = materialize(&config, &MacroContext::for_project("/p"), true).unwrap();
        assert_eq!(materialized.args[..2], ["-S", "0.0.0.0:8081"]);
        let ports = published_ports(&config);
        assert_eq!(
            (
                ports[0].host_port.as_str(),
                ports[0].container_port.as_str()
            ),
            ("8081", "8081")
        );
        assert!(published_ports(&RunConfig::default()).is_empty());
    }

    #[test]
    fn php_configurations_are_the_php_toolchain_and_the_php_kinds() {
        let toolchain = |id: &str| RunConfig {
            toolchain: Some(id.into()),
            ..RunConfig::default()
        };
        assert!(is_php_config(&toolchain("php")));
        assert!(is_php_config(&server(Default::default())));
        assert!(!is_php_config(&toolchain("cargo")));
    }

    #[test]
    fn a_php_configuration_inherits_the_interpreters_container_but_keeps_an_explicit_choice() {
        let php = RunConfig {
            toolchain: Some("php".into()),
            ..RunConfig::default()
        };
        assert_eq!(
            inherit_container_target(&php, Some("t1")).run_on.as_deref(),
            Some("container:t1")
        );
        assert_eq!(
            inherit_container_target(&server(Default::default()), Some("t1"))
                .run_on
                .as_deref(),
            Some("container:t1")
        );
        let local = RunConfig {
            run_on: Some("local".into()),
            ..php.clone()
        };
        assert_eq!(
            inherit_container_target(&local, Some("t1"))
                .run_on
                .as_deref(),
            Some("local")
        );
        assert_eq!(inherit_container_target(&php, None).run_on, None);
        assert_eq!(inherit_container_target(&php, Some(" ")).run_on, None);
        // Not PHP: cargo stays on the host.
        let cargo = RunConfig {
            toolchain: Some("cargo".into()),
            ..RunConfig::default()
        };
        assert_eq!(inherit_container_target(&cargo, Some("t1")).run_on, None);
    }

    #[test]
    fn a_server_run_on_a_container_target_publishes_its_port_and_mounts_the_project() {
        let mut config = server(Default::default());
        config.run_on = Some("container:t1".into());
        let containers = app_config::ContainerSettings {
            targets: vec![app_config::ContainerTargetSetting {
                id: "t1".into(),
                source: "image".into(),
                image: Some("php:8.3-cli".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let context = MacroContext::for_project("/p").with_containers(containers);
        let spec = config.to_launch_spec_in(&context);
        assert_eq!(spec.program, "docker");
        let args = spec.args;
        assert!(
            args.windows(2).any(|w| w == ["-p", "8000:8000"]),
            "{args:?}"
        );
        assert_eq!(
            args[args.len() - 5..],
            ["php", "-S", "0.0.0.0:8000", "-t", "/workspace"]
        );
    }
}
