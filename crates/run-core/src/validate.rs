//! Whether a run configuration, as drafted in the editor, can be saved: the
//! rule behind the Run Configurations dialog's refusal, kept out of the
//! bridge so each kind's "nothing to check" is a tested decision.

use app_config::ContainerSettings;

use crate::config::RunConfig;
use crate::php_run;

/// What is wrong with `config`, worded to follow its quoted name, or `None`.
///
/// A kind that compiles its own launch has no `program` to check, so the
/// plain-process check must not run for it: the container kinds have their
/// own `container_core::run_config::validate_*` rule, and the PHP and
/// `sql-script` kinds have nothing to refuse (a `sql-script` configuration
/// runs a `.sql` file against a data source and never has a program).
pub fn problem(config: &RunConfig, containers: &ContainerSettings) -> Option<String> {
    match config.kind.as_deref() {
        Some("container-image") => config
            .container_image
            .as_ref()
            .and_then(container_core::run_config::validate_image),
        Some("containerfile") => config
            .containerfile
            .as_ref()
            .and_then(container_core::run_config::validate_containerfile),
        Some("compose") => config
            .compose
            .as_ref()
            .and_then(container_core::run_config::validate_compose),
        Some(php_run::KIND_BUILTIN_SERVER | php_run::KIND_CONSOLE | "sql-script") => None,
        _ => config
            .program
            .trim()
            .is_empty()
            .then(|| "has no program to run".to_string())
            .or_else(|| {
                config.run_on.as_deref().and_then(|run_on| {
                    let id = crate::container_target::target_id(run_on)?;
                    containers
                        .targets
                        .iter()
                        .all(|t| t.id != id)
                        .then(|| format!("runs on a target (\"{id}\") that no longer exists"))
                })
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(kind: Option<&str>, program: &str) -> RunConfig {
        RunConfig {
            id: "c".into(),
            name: "c".into(),
            program: program.into(),
            kind: kind.map(str::to_string),
            ..RunConfig::default()
        }
    }

    #[test]
    fn a_plain_process_needs_a_program() {
        let containers = ContainerSettings::default();
        assert_eq!(
            problem(&config(None, "  "), &containers).as_deref(),
            Some("has no program to run")
        );
        assert_eq!(problem(&config(None, "cargo"), &containers), None);
    }

    #[test]
    fn a_sql_script_configuration_has_no_program_and_is_not_refused_for_it() {
        let containers = ContainerSettings::default();
        assert_eq!(problem(&config(Some("sql-script"), ""), &containers), None);
    }

    #[test]
    fn the_php_kinds_have_no_program_to_check_either() {
        let containers = ContainerSettings::default();
        for kind in [php_run::KIND_BUILTIN_SERVER, php_run::KIND_CONSOLE] {
            assert_eq!(
                problem(&config(Some(kind), ""), &containers),
                None,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_run_target_that_no_longer_exists_is_refused() {
        let mut stale = config(None, "php");
        stale.run_on = Some("container:gone".into());
        assert_eq!(
            problem(&stale, &ContainerSettings::default()).as_deref(),
            Some("runs on a target (\"gone\") that no longer exists")
        );
    }
}
