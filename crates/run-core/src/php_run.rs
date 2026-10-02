//! PHP run kinds (PHP parity plan, I5): `php-builtin-server` (`php -S`) and
//! `php-console` (PsySH when the project has it, otherwise `php -a`).
//!
//! Both compile to an ordinary process configuration, so they flow through
//! the plain-process path in [`crate::config`] and inherit whatever applies
//! to it (run targets, before-launch tasks, macros). The program is the
//! `[php]` interpreter carried by [`MacroContext::php_interpreter`].

use std::path::Path;

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
pub fn materialize(config: &RunConfig, context: &MacroContext) -> Option<RunConfig> {
    let program = context
        .php_interpreter
        .clone()
        .unwrap_or_else(|| DEFAULT_PHP_PROGRAM.to_string());
    let args = match config.kind.as_deref()? {
        KIND_BUILTIN_SERVER => server_args(&config.php_server.clone().unwrap_or_default()),
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

fn server_args(server: &PhpBuiltinServerRunSetting) -> Vec<String> {
    let host = non_blank(&server.host).unwrap_or(DEFAULT_HOST);
    let port = if server.port == 0 {
        DEFAULT_PORT
    } else {
        server.port
    };
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
        assert!(materialize(&RunConfig::default(), &MacroContext::default()).is_none());
    }
}
