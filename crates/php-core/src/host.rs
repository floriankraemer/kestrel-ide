//! Where PHP tools run (ADR-0067): the container the `[php]` settings
//! name, or the project's own host.
//!
//! Analyzers, test frameworks and language servers that need the PHP
//! interpreter (`requires-interpreter = "php"`, `exec = "interpreter"`) run
//! on this host; git, cargo and everything else stays on the one
//! `ExecHost::for_path` gives.

use std::path::Path;

use app_config::php::PhpSettings;
use app_config::ContainerSettings;
use container_core::target::{exec_host_for, ExecMode};
use process_exec::host::ExecHost;

/// The host the PHP interpreter runs on.
///
/// A blank `container_target`, or one no `[containers.target]` row has any
/// more, means the project's own host: tools keep working rather than
/// failing until the setting is fixed. `container_mode` is `exec` or `run`;
/// any other word lets the target's source decide.
pub fn interpreter_host(
    php: &PhpSettings,
    containers: &ContainerSettings,
    root: &Path,
) -> ExecHost {
    let mode = match php.container_mode.as_deref().map(str::trim) {
        Some("exec") => Some(ExecMode::Exec),
        Some("run") => Some(ExecMode::Run),
        _ => None,
    };
    php.container_target
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| exec_host_for(containers, id, root, mode))
        .unwrap_or_else(|| ExecHost::for_path(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::ContainerTargetSetting;

    fn containers() -> ContainerSettings {
        ContainerSettings {
            targets: vec![ContainerTargetSetting {
                id: "t1".into(),
                source: "compose-service".into(),
                compose_files: vec!["dc.yml".into()],
                service: Some("php".into()),
                ..ContainerTargetSetting::default()
            }],
            ..ContainerSettings::default()
        }
    }

    fn php(target: Option<&str>, mode: Option<&str>) -> PhpSettings {
        PhpSettings {
            container_target: target.map(String::from),
            container_mode: mode.map(String::from),
            ..PhpSettings::default()
        }
    }

    fn verb(host: &ExecHost) -> String {
        let ExecHost::Container(c) = host else {
            panic!("expected a container host, got {host:?}")
        };
        c.verb_args.join(" ")
    }

    #[test]
    fn no_target_is_the_projects_own_host() {
        let root = Path::new("/p");
        assert_eq!(
            interpreter_host(&php(None, None), &containers(), root),
            ExecHost::Local
        );
        assert_eq!(
            interpreter_host(&php(Some("  "), None), &containers(), root),
            ExecHost::Local
        );
    }

    #[test]
    fn a_target_that_no_longer_exists_falls_back_to_the_project_host() {
        assert_eq!(
            interpreter_host(&php(Some("gone"), None), &containers(), Path::new("/p")),
            ExecHost::Local
        );
    }

    #[test]
    fn a_compose_service_target_execs_unless_the_mode_says_run() {
        let root = Path::new("/p");
        assert_eq!(
            verb(&interpreter_host(
                &php(Some("t1"), None),
                &containers(),
                root
            )),
            "exec -T"
        );
        assert_eq!(
            verb(&interpreter_host(
                &php(Some("t1"), Some("run")),
                &containers(),
                root
            )),
            "run --rm -T"
        );
        // An unknown word lets the source decide.
        assert_eq!(
            verb(&interpreter_host(
                &php(Some("t1"), Some("bogus")),
                &containers(),
                root
            )),
            "exec -T"
        );
    }
}
