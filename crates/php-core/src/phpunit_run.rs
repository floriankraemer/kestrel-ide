//! The `phpunit` run configuration (PHPUnit editor runs plan, E4, ADR-0073):
//! validating its table, turning it into a `test-core` scope, and building
//! the temporary configuration an editor run (caret, gutter, data set)
//! creates.
//!
//! Persistence is `app-config`'s [`PhpUnitRunSetting`]; the argv a scope
//! becomes is `test-core`'s; launching is `ui-shell`'s `TestService`.

use app_config::php::{PhpUnitRunSetting, PHPUNIT_KIND};
use app_config::RunConfigSetting;
use test_core::scope::{split_data_set_id, ScopeOptions, TestScope};

use crate::tests::{MarkerScope, TestMarker};

/// Why a `phpunit` table cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The scope needs a field the table leaves blank.
    MissingField {
        scope: &'static str,
        field: &'static str,
    },
    /// A path that leaves the project: paths are passed relative to the
    /// run's working directory, which is inside the project on every host.
    PathOutsideProject(String),
    /// The working directory, project-relative, is not a directory.
    MissingWorkingDirectory(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField { scope, field } => {
                write!(f, "needs a {field} for its {scope} scope")
            }
            Self::PathOutsideProject(path) => {
                write!(f, "names \"{path}\", which is outside the project")
            }
            Self::MissingWorkingDirectory(dir) => write!(
                f,
                "has the working directory \"{dir}\", which does not exist"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// What a validated table asks `TestService` to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub scope: TestScope,
    pub options: ScopeOptions,
    /// The `php` program to use instead of `[php]`'s.
    pub interpreter: Option<String>,
    /// Arguments for the interpreter, before the test runner's program
    /// (`-d memory_limit=-1`), split then expanded like the runner options.
    pub interpreter_options: Vec<String>,
    /// Where the run starts, project-relative; `None` is the project root.
    /// The scope's and options' paths are relative to it.
    pub working_directory: Option<String>,
}

fn non_blank(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn require(value: &str, scope: &'static str, field: &'static str) -> Result<String, ConfigError> {
    non_blank(value).ok_or(ConfigError::MissingField { scope, field })
}

/// A project path: `expand` has run; backslashes become `/`, a path under
/// `root` becomes relative to it, and anything else absolute or climbing out
/// with `..` is outside the project.
fn project_path(path: &str, root: &str, ignore_case: bool) -> Result<String, ConfigError> {
    let path = path.trim().replace('\\', "/");
    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    let relative = if absolute {
        test_core::filter::relative_to_root(&path, root, ignore_case)
    } else {
        Some(path.trim_start_matches("./").to_string())
    };
    relative
        .filter(|relative| !relative.split('/').any(|part| part == ".."))
        // The project root itself: PHPUnit takes `.` for the working directory.
        .map(|relative| match relative.trim_end_matches('/') {
            "" => ".".to_string(),
            trimmed => trimmed.to_string(),
        })
        .ok_or(ConfigError::PathOutsideProject(path))
}

/// The project-relative working directory `dir` (macros expanded) names;
/// `None` for the project root, blank, or a directory outside the project.
pub fn working_directory(dir: &str, root: &str, ignore_case: bool) -> Option<String> {
    non_blank(dir)
        .and_then(|dir| project_path(&dir, root, ignore_case).ok())
        .filter(|dir| dir != ".")
}

/// Which fields of the table a scope reads — what the editor shows. The
/// same spellings [`scope_of`] matches; an unknown scope is the
/// configuration file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScopeFields {
    pub path: bool,
    pub class: bool,
    pub method: bool,
    pub data_set: bool,
    pub filter: bool,
    pub testsuite: bool,
}

pub fn fields_of(scope: &str) -> ScopeFields {
    let none = ScopeFields::default();
    match scope {
        "directory" => ScopeFields { path: true, ..none },
        "file" => ScopeFields {
            path: true,
            filter: true,
            ..none
        },
        "class" => ScopeFields {
            path: true,
            class: true,
            ..none
        },
        "method" => ScopeFields {
            path: true,
            class: true,
            method: true,
            ..none
        },
        "data_set" => ScopeFields {
            path: true,
            class: true,
            method: true,
            data_set: true,
            ..none
        },
        _ => ScopeFields {
            testsuite: true,
            ..none
        },
    }
}

/// The `phpunit` table of a `phpunit` configuration, the default
/// (configuration-file scope) when it has none; `None` for any other kind.
pub fn setting_of(config: &RunConfigSetting) -> Option<PhpUnitRunSetting> {
    (config.kind.as_deref() == Some(PHPUNIT_KIND))
        .then(|| config.phpunit.clone().unwrap_or_default())
}

/// The scope a table names, with only the fields that scope needs checked.
/// An unknown or blank scope is the configuration-file scope.
fn scope_of(
    setting: &PhpUnitRunSetting,
    project: &impl Fn(&str) -> Result<String, ConfigError>,
) -> Result<TestScope, ConfigError> {
    let optional_path = || non_blank(&setting.path).map(|p| project(&p)).transpose();
    Ok(match setting.scope.as_str() {
        "directory" => TestScope::Directory {
            path: project(&require(&setting.path, "directory", "path")?)?,
        },
        "file" => TestScope::File {
            path: project(&require(&setting.path, "file", "path")?)?,
            filter: non_blank(&setting.filter),
        },
        "class" => TestScope::Class {
            class: require(&setting.class, "class", "class")?,
            path: optional_path()?,
        },
        "method" => TestScope::Method {
            class: require(&setting.class, "method", "class")?,
            method: require(&setting.method, "method", "method")?,
            path: optional_path()?,
        },
        "data_set" => TestScope::DataSet {
            class: require(&setting.class, "data_set", "class")?,
            method: require(&setting.method, "data_set", "method")?,
            data_set: require(&setting.data_set, "data_set", "data set")?,
            path: optional_path()?,
        },
        _ => TestScope::Config {
            testsuite: non_blank(&setting.testsuite),
        },
    })
}

/// Validate `setting` and resolve it for a run in `root`. `expand` replaces
/// the macros in a path or an option; `ignore_case` is for Windows roots.
pub fn plan(
    setting: &PhpUnitRunSetting,
    expand: impl Fn(&str) -> String,
    root: &str,
    ignore_case: bool,
) -> Result<Plan, ConfigError> {
    let working_directory = non_blank(&setting.working_directory)
        .map(|dir| project_path(&expand(&dir), root, ignore_case))
        .transpose()?
        .and_then(|dir| working_directory(&dir, root, ignore_case));
    let work_dir = working_directory.clone().unwrap_or_default();
    let project = |path: &str| {
        project_path(&expand(path), root, ignore_case)
            .map(|path| test_core::filter::relative_to_work_dir(&path, &work_dir))
    };
    let scope = scope_of(setting, &project)?;
    let words = |list: &[String]| -> Vec<String> {
        list.iter().filter_map(|word| non_blank(word)).collect()
    };
    // Split before expanding: a macro's value (a Windows project directory
    // with backslashes or spaces) is one literal word.
    let shell_words = |line: &str| -> Vec<String> {
        container_core::run_config::split_shell_words(line)
            .iter()
            .map(|word| expand(word))
            .collect()
    };
    Ok(Plan {
        scope,
        options: ScopeOptions {
            configuration: non_blank(&setting.configuration)
                .map(|file| project(&file))
                .transpose()?,
            groups: words(&setting.groups),
            exclude_groups: words(&setting.exclude_groups),
            runner_options: shell_words(&setting.runner_options),
        },
        interpreter: non_blank(&setting.interpreter),
        interpreter_options: shell_words(&setting.interpreter_options),
        working_directory,
    })
}

/// `plan`'s working directory exists in the project at `root`: a run in a
/// missing one would only say its program was not found.
pub fn check_working_directory(plan: &Plan, root: &std::path::Path) -> Result<(), ConfigError> {
    match &plan.working_directory {
        Some(dir) if !root.join(dir).is_dir() => {
            Err(ConfigError::MissingWorkingDirectory(dir.clone()))
        }
        _ => Ok(()),
    }
}

/// What is wrong with `setting`, worded to follow a quoted configuration
/// name, or `None`: the editor's refusal to save. A path with a macro is
/// only checked for being present (its value is known at run time); any
/// other path must stay inside `root`.
pub fn problem(setting: &PhpUnitRunSetting, root: &str, ignore_case: bool) -> Option<String> {
    let check = |path: &str| {
        if path.contains('$') {
            Ok(path.to_string())
        } else {
            project_path(path, root, ignore_case)
        }
    };
    scope_of(setting, &check)
        .and_then(|_| {
            [&setting.configuration, &setting.working_directory]
                .into_iter()
                .filter_map(|path| non_blank(path))
                .try_for_each(|path| check(&path).map(drop))
        })
        .err()
        .map(|error| error.to_string())
}

fn short_class(class: &str) -> &str {
    class.rsplit('\\').next().unwrap_or(class)
}

fn last_component(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// The name PhpStorm gives a configuration of this scope.
pub fn default_name(setting: &PhpUnitRunSetting) -> String {
    match setting.scope.as_str() {
        // The project root: PhpStorm names the run of everything "All tests".
        "directory" if matches!(setting.path.trim_end_matches('/'), "." | "") => {
            "All tests".to_string()
        }
        "directory" => format!("Tests in '{}'", last_component(&setting.path)),
        "file" => last_component(&setting.path).to_string(),
        "class" => short_class(&setting.class).to_string(),
        "method" => format!("{}::{}", short_class(&setting.class), setting.method),
        "data_set" => format!(
            "{}::{} with data set {}",
            short_class(&setting.class),
            setting.method,
            setting.data_set
        ),
        _ => {
            let suite = non_blank(&setting.testsuite).unwrap_or_else(|| "All tests".to_string());
            let file = non_blank(&setting.configuration).map_or_else(
                || "phpunit.xml".to_string(),
                |c| last_component(&c).to_string(),
            );
            format!("{suite} ({file})")
        }
    }
}

/// The name a temporary configuration should carry now, when it still
/// wears one an earlier version generated for its scope; `None` keeps the
/// name, as for any name the user chose or a saved configuration.
pub fn refreshed_name(config: &RunConfigSetting) -> Option<String> {
    let setting = setting_of(config).filter(|_| config.temporary)?;
    let current = default_name(&setting);
    (config.name != current && earlier_default_names(&setting).contains(&config.name))
        .then_some(current)
}

/// The names [`default_name`] gave a scope before they read as PhpStorm's.
fn earlier_default_names(setting: &PhpUnitRunSetting) -> Vec<String> {
    match setting.scope.as_str() {
        "directory" => vec![format!("tests in '{}'", last_component(&setting.path))],
        _ => Vec::new(),
    }
}

/// A stable id for the scope, so repeated runs of one test reuse one
/// temporary entry.
fn id_of(setting: &PhpUnitRunSetting) -> String {
    let id = format!(
        "phpunit:{}:{}:{}:{}:{}:{}:{}:{}",
        setting.scope,
        setting.path,
        setting.class,
        setting.method,
        setting.data_set,
        setting.filter,
        setting.testsuite,
        setting.configuration
    );
    // Appended only when set, so the ids of earlier configurations hold.
    match setting.working_directory.as_str() {
        "" => id,
        dir => format!("{id}:{dir}"),
    }
}

/// The temporary configuration for `setting`, named `name` (or its default).
pub fn temporary_config(setting: PhpUnitRunSetting, name: Option<String>) -> RunConfigSetting {
    RunConfigSetting {
        id: id_of(&setting),
        name: name.unwrap_or_else(|| default_name(&setting)),
        kind: Some(PHPUNIT_KIND.to_string()),
        temporary: true,
        phpunit: Some(setting),
        ..RunConfigSetting::default()
    }
}

/// The id earlier builds gave the temporary configuration `config`: before a
/// package's editor runs started in the package (#424), they had neither a
/// working directory nor the package's configuration file. `None` for any
/// other configuration.
pub fn earlier_id(config: &RunConfigSetting) -> Option<String> {
    let setting = setting_of(config)
        .filter(|setting| config.temporary && !setting.working_directory.is_empty())?;
    Some(id_of(&PhpUnitRunSetting {
        working_directory: String::new(),
        configuration: String::new(),
        ..setting
    }))
}

/// The stored configuration an editor run of `config` runs: the one stored
/// under its id, which the dialog may have edited, else the temporary an
/// earlier build stored under [`earlier_id`], moved to the package with its
/// edits kept, else `config` itself.
pub fn stored_for(stored: &[RunConfigSetting], config: RunConfigSetting) -> RunConfigSetting {
    let is_phpunit = |stored: &&RunConfigSetting| stored.kind.as_deref() == Some(PHPUNIT_KIND);
    if let Some(same) = stored.iter().filter(is_phpunit).find(|s| s.id == config.id) {
        return same.clone();
    }
    let earlier = earlier_id(&config).and_then(|id| {
        stored
            .iter()
            .filter(is_phpunit)
            .find(|s| s.temporary && s.id == id)
    });
    let Some(earlier) = earlier else {
        return config;
    };
    let package = config.phpunit.clone().unwrap_or_default();
    let setting = PhpUnitRunSetting {
        working_directory: package.working_directory,
        configuration: package.configuration,
        ..earlier.phpunit.clone().unwrap_or_default()
    };
    RunConfigSetting {
        id: config.id,
        phpunit: Some(setting),
        ..earlier.clone()
    }
}

/// Drop the temporary an earlier build stored for `config` under
/// [`earlier_id`], so the list does not show the run twice.
pub fn forget_earlier(stored: &mut Vec<RunConfigSetting>, config: &RunConfigSetting) {
    if let Some(id) = earlier_id(config) {
        stored.retain(|s| !(s.temporary && s.id == id));
    }
}

/// Why the Tests dock cannot rerun a selection as the runs that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerunError {
    /// The tests came from runs in different working directories or with
    /// different options, and one run can only have one of each.
    MixedRuns,
}

impl std::fmt::Display for RerunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MixedRuns => write!(
                f,
                "These tests came from runs with different working directories or options; rerun the tests of one run at a time."
            ),
        }
    }
}

impl std::error::Error for RerunError {}

/// Everything about a run except which tests it selects.
#[derive(Debug, PartialEq, Eq)]
struct RunContext {
    setting: PhpUnitRunSetting,
    env: Vec<(String, String)>,
}

fn context_of(config: Option<&RunConfigSetting>) -> RunContext {
    let setting = config.and_then(setting_of).unwrap_or_default();
    RunContext {
        setting: PhpUnitRunSetting {
            configuration: setting.configuration,
            groups: setting.groups,
            exclude_groups: setting.exclude_groups,
            runner_options: setting.runner_options,
            interpreter: setting.interpreter,
            interpreter_options: setting.interpreter_options,
            working_directory: setting.working_directory,
            ..PhpUnitRunSetting::default()
        },
        env: config.map(|config| config.env.clone()).unwrap_or_default(),
    }
}

/// The configuration a Tests dock rerun (Rerun Test, Run Failed) narrows by
/// its filter, given the configurations of the runs that last reported the
/// tests (`None` for an ad hoc run, Run All): PhpStorm reruns a test with the
/// configuration that produced it. One run: its configuration. Several in
/// one context (working directory, configuration file, groups, options,
/// interpreter, environment): that context over the configuration file's
/// whole scope. `Ok(None)` runs ad hoc.
pub fn rerun_config(
    producers: &[Option<RunConfigSetting>],
) -> Result<Option<RunConfigSetting>, RerunError> {
    let Some((first, rest)) = producers.split_first() else {
        return Ok(None);
    };
    if rest.iter().all(|producer| producer == first) {
        return Ok(first.clone());
    }
    let context = context_of(first.as_ref());
    if rest
        .iter()
        .any(|producer| context_of(producer.as_ref()) != context)
    {
        return Err(RerunError::MixedRuns);
    }
    Ok(producers.iter().flatten().next().map(|config| {
        let setting = PhpUnitRunSetting {
            scope: "config".into(),
            ..context_of(Some(config)).setting
        };
        RunConfigSetting {
            phpunit: Some(setting),
            ..config.clone()
        }
    }))
}

/// The configuration a gutter or caret run of `marker` stands for, with the
/// test file project-relative when it is inside the project.
pub fn marker_config(
    marker: &TestMarker,
    relative_file: Option<&str>,
) -> Result<RunConfigSetting, ConfigError> {
    let path = relative_file.unwrap_or_default().to_string();
    let setting = match (marker.scope, marker.class.is_empty()) {
        (MarkerScope::File, _) | (_, true) => {
            let scope = "file";
            if path.is_empty() {
                return Err(ConfigError::MissingField {
                    scope,
                    field: "path",
                });
            }
            PhpUnitRunSetting {
                scope: scope.into(),
                path,
                filter: marker.filter.clone(),
                ..PhpUnitRunSetting::default()
            }
        }
        (MarkerScope::Group, false) => PhpUnitRunSetting {
            scope: "class".into(),
            path,
            class: marker.class.clone(),
            ..PhpUnitRunSetting::default()
        },
        (MarkerScope::Test, false) => PhpUnitRunSetting {
            scope: "method".into(),
            path,
            class: marker.class.clone(),
            method: marker.method.clone(),
            ..PhpUnitRunSetting::default()
        },
    };
    // A Pest call has no class to name; its description is the useful name.
    let name = (setting.scope == "file" && !marker.name.is_empty()).then(|| marker.name.clone());
    Ok(temporary_config(setting, name))
}

/// The configuration that reruns one data-set row of the Tests dock (`id` is
/// the node id), or `None` when `id` is not a data-set row.
pub fn data_set_config(id: &str, relative_file: Option<&str>) -> Option<RunConfigSetting> {
    let (class, method, data_set) = split_data_set_id(id)?;
    Some(temporary_config(
        PhpUnitRunSetting {
            scope: "data_set".into(),
            path: relative_file.unwrap_or_default().to_string(),
            class: class.into(),
            method: method.into(),
            data_set: data_set.into(),
            ..PhpUnitRunSetting::default()
        },
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::markers;

    fn setting(scope: &str) -> PhpUnitRunSetting {
        PhpUnitRunSetting {
            scope: scope.into(),
            ..PhpUnitRunSetting::default()
        }
    }

    fn plan_of(setting: &PhpUnitRunSetting) -> Result<Plan, ConfigError> {
        plan(
            setting,
            |text| text.replace("$PROJECT_DIR$", "/p"),
            "/p",
            false,
        )
    }

    #[test]
    fn a_configuration_scope_with_a_test_suite_passes_testsuite_and_configuration() {
        let plan = plan_of(&PhpUnitRunSetting {
            testsuite: "core-unit".into(),
            configuration: "phpunit.dist.xml".into(),
            groups: vec!["api".into()],
            ..setting("config")
        })
        .unwrap();
        let args = test_core::scope::scope_args(
            &["--teamcity".to_string()],
            test_core::filter::FilterDialect::PhpUnitRegex,
            Some("--filter"),
            None,
            &plan.scope,
            &plan.options,
        )
        .unwrap();
        let line = args.join(" ");
        assert!(line.contains("--testsuite core-unit"), "{line}");
        assert!(line.contains("--configuration phpunit.dist.xml"), "{line}");
        assert!(line.contains("--group api"), "{line}");
    }

    #[test]
    fn every_scope_names_the_fields_it_needs() {
        let cases = [
            ("directory", "path"),
            ("file", "path"),
            ("class", "class"),
            ("method", "class"),
            ("data_set", "class"),
        ];
        for (scope, field) in cases {
            assert_eq!(
                plan_of(&setting(scope)).unwrap_err(),
                ConfigError::MissingField {
                    scope: match scope {
                        "directory" => "directory",
                        "file" => "file",
                        "class" => "class",
                        "method" => "method",
                        _ => "data_set",
                    },
                    field
                }
            );
        }
        let no_method = PhpUnitRunSetting {
            class: "A".into(),
            ..setting("method")
        };
        assert!(problem(&no_method, "/p", false).unwrap().contains("method"));
        assert!(problem(&setting("data_set"), "/p", false).is_some());
        assert_eq!(problem(&setting("config"), "/p", false), None);
    }

    #[test]
    fn blank_and_unknown_scopes_read_as_the_configuration_file() {
        for scope in ["", "nonsense"] {
            let plan = plan_of(&setting(scope)).unwrap();
            assert_eq!(plan.scope, TestScope::Config { testsuite: None });
        }
    }

    #[test]
    fn paths_expand_macros_and_become_project_relative() {
        let directory = PhpUnitRunSetting {
            path: "$PROJECT_DIR$/tests\\Unit".into(),
            ..setting("directory")
        };
        assert_eq!(
            plan_of(&directory).unwrap().scope,
            TestScope::Directory {
                path: "tests/Unit".into()
            }
        );
        let relative = PhpUnitRunSetting {
            path: "./tests".into(),
            ..setting("directory")
        };
        assert_eq!(
            plan_of(&relative).unwrap().scope,
            TestScope::Directory {
                path: "tests".into()
            }
        );
    }

    #[test]
    fn a_path_outside_the_project_is_refused() {
        for path in ["/elsewhere/tests", "../tests", "a/../../b"] {
            let outside = PhpUnitRunSetting {
                path: path.into(),
                ..setting("file")
            };
            assert!(matches!(
                plan_of(&outside),
                Err(ConfigError::PathOutsideProject(_))
            ));
        }
    }

    #[test]
    fn a_working_directory_makes_paths_relative_to_it() {
        let package = PhpUnitRunSetting {
            path: "$PROJECT_DIR$/packages/mailer/tests".into(),
            configuration: "packages/mailer/phpunit.xml.dist".into(),
            working_directory: "$PROJECT_DIR$/packages/mailer/".into(),
            ..setting("directory")
        };
        let plan = plan_of(&package).unwrap();
        assert_eq!(plan.working_directory.as_deref(), Some("packages/mailer"));
        assert_eq!(
            plan.scope,
            TestScope::Directory {
                path: "tests".into()
            }
        );
        assert_eq!(
            plan.options.configuration.as_deref(),
            Some("phpunit.xml.dist")
        );

        // The project root is no working directory of its own.
        let root = PhpUnitRunSetting {
            working_directory: ".".into(),
            ..setting("config")
        };
        assert_eq!(plan_of(&root).unwrap().working_directory, None);

        let outside = PhpUnitRunSetting {
            working_directory: "../elsewhere".into(),
            ..setting("config")
        };
        assert!(matches!(
            plan_of(&outside),
            Err(ConfigError::PathOutsideProject(_))
        ));
        assert!(problem(&outside, "/p", false).is_some());
        let with_macro = PhpUnitRunSetting {
            working_directory: "$PROJECT_DIR$/packages/a".into(),
            ..setting("config")
        };
        assert_eq!(problem(&with_macro, "/p", false), None);

        assert_eq!(
            working_directory("/p/packages/a/", "/p", false).as_deref(),
            Some("packages/a")
        );
        for none in ["", "/p", ".", "/elsewhere", "../x"] {
            assert_eq!(working_directory(none, "/p", false), None, "{none}");
        }
    }

    #[test]
    fn the_working_directory_keeps_its_own_temporary_entry() {
        let root = temporary_config(setting("config"), None);
        let package = temporary_config(
            PhpUnitRunSetting {
                working_directory: "packages/a".into(),
                ..setting("config")
            },
            None,
        );
        assert_eq!(root.id, "phpunit:config:::::::");
        assert_ne!(root.id, package.id);
    }

    #[test]
    fn options_are_trimmed_and_the_runner_options_shell_split() {
        let setting = PhpUnitRunSetting {
            configuration: "ci/phpunit.xml".into(),
            groups: vec![" a ".into(), "".into()],
            exclude_groups: vec!["slow".into()],
            runner_options: "--stop-on-failure --filter 'a b'".into(),
            interpreter: " php8.3 ".into(),
            testsuite: "Unit".into(),
            ..setting("config")
        };
        let plan = plan_of(&setting).unwrap();
        assert_eq!(
            plan.scope,
            TestScope::Config {
                testsuite: Some("Unit".into())
            }
        );
        assert_eq!(
            plan.options.configuration.as_deref(),
            Some("ci/phpunit.xml")
        );
        assert_eq!(plan.options.groups, ["a"]);
        assert_eq!(plan.options.exclude_groups, ["slow"]);
        assert_eq!(
            plan.options.runner_options,
            ["--stop-on-failure", "--filter", "a b"]
        );
        assert_eq!(plan.interpreter.as_deref(), Some("php8.3"));
    }

    #[test]
    fn names_follow_phpstorm() {
        let name = |s: PhpUnitRunSetting| default_name(&s);
        assert_eq!(
            name(PhpUnitRunSetting {
                path: "tests/Unit".into(),
                ..setting("directory")
            }),
            "Tests in 'Unit'"
        );
        assert_eq!(
            name(PhpUnitRunSetting {
                path: ".".into(),
                ..setting("directory")
            }),
            "All tests"
        );
        assert_eq!(
            name(PhpUnitRunSetting {
                class: "App\\GreeterTest".into(),
                ..setting("class")
            }),
            "GreeterTest"
        );
        assert_eq!(
            name(PhpUnitRunSetting {
                class: "App\\GreeterTest".into(),
                method: "testGreets".into(),
                ..setting("method")
            }),
            "GreeterTest::testGreets"
        );
        assert_eq!(
            name(PhpUnitRunSetting {
                testsuite: "Unit".into(),
                ..setting("config")
            }),
            "Unit (phpunit.xml)"
        );
        assert_eq!(name(setting("config")), "All tests (phpunit.xml)");
    }

    const SOURCE: &str = "<?php\nnamespace App;\nclass GreeterTest extends TestCase\n{\n    public function testGreets() {}\n}\n";

    #[test]
    fn a_phpunit_marker_becomes_a_class_or_method_configuration() {
        let all = markers(SOURCE);
        let class = marker_config(&all[0], Some("tests/GreeterTest.php")).unwrap();
        assert_eq!(class.name, "GreeterTest");
        assert!(class.temporary);
        assert_eq!(class.kind.as_deref(), Some(PHPUNIT_KIND));
        let method = marker_config(&all[1], Some("tests/GreeterTest.php")).unwrap();
        assert_eq!(method.name, "GreeterTest::testGreets");
        let setting = method.phpunit.unwrap();
        assert_eq!(
            (setting.scope.as_str(), setting.class.as_str()),
            ("method", "App\\GreeterTest")
        );
        // The same test reuses one entry.
        let again = marker_config(&all[1], Some("tests/GreeterTest.php")).unwrap();
        assert_eq!(again.id, method.id);
        assert_ne!(again.id, class.id);
    }

    #[test]
    fn a_pest_call_runs_its_file_narrowed_by_the_marker_filter() {
        let text = "<?php\nit('greets', fn () => 1);\n";
        let marker = &markers(text)[0];
        let config = marker_config(marker, Some("tests/Pest.php")).unwrap();
        assert_eq!(config.name, "greets");
        let setting = config.phpunit.unwrap();
        assert_eq!(setting.scope, "file");
        assert_eq!(setting.filter, marker.filter);
        assert!(matches!(
            marker_config(marker, None),
            Err(ConfigError::MissingField { .. })
        ));
    }

    #[test]
    fn the_whole_file_marker_needs_a_project_file() {
        let marker = crate::tests::marker_at(SOURCE, 0).unwrap();
        assert!(marker_config(&marker, None).is_err());
    }

    #[test]
    fn a_data_set_row_becomes_a_data_set_configuration() {
        let config =
            data_set_config("App\\T::testA with data set \"x\"", Some("tests/T.php")).unwrap();
        let setting = config.phpunit.unwrap();
        assert_eq!(setting.scope, "data_set");
        assert_eq!(setting.data_set, "\"x\"");
        assert_eq!(config.name, "T::testA with data set \"x\"");
        assert!(data_set_config("App\\T::testA", None).is_none());
    }

    #[test]
    fn saving_refuses_a_macro_free_path_outside_the_project() {
        for path in ["/elsewhere/tests", "../tests"] {
            let outside = PhpUnitRunSetting {
                path: path.into(),
                ..setting("directory")
            };
            assert!(problem(&outside, "/p", false)
                .unwrap()
                .contains("outside the project"));
        }
        let config_outside = PhpUnitRunSetting {
            configuration: "/etc/phpunit.xml".into(),
            ..setting("config")
        };
        assert!(problem(&config_outside, "/p", false).is_some());
        for path in ["/p/tests", "tests", "$PROJECT_DIR$/../x"] {
            let fine = PhpUnitRunSetting {
                path: path.into(),
                ..setting("directory")
            };
            assert_eq!(problem(&fine, "/p", false), None, "{path}");
        }
    }

    #[test]
    fn the_project_root_is_a_directory_scope_of_dot() {
        for path in ["/p", "/p/", ".", "./", "$PROJECT_DIR$"] {
            let root = PhpUnitRunSetting {
                path: path.into(),
                ..setting("directory")
            };
            assert_eq!(
                plan_of(&root).unwrap().scope,
                TestScope::Directory { path: ".".into() },
                "{path}"
            );
        }
    }

    #[test]
    fn runner_options_are_split_before_macros_expand() {
        let setting = PhpUnitRunSetting {
            runner_options: "--log-junit $PROJECT_DIR$/out.xml --cache-directory=$PROJECT_DIR$"
                .into(),
            ..setting("config")
        };
        let plan = plan(
            &setting,
            |text| text.replace("$PROJECT_DIR$", r"C:\My Proj"),
            r"C:\My Proj",
            true,
        )
        .unwrap();
        assert_eq!(
            plan.options.runner_options,
            [
                "--log-junit",
                r"C:\My Proj/out.xml",
                r"--cache-directory=C:\My Proj"
            ]
        );
    }

    #[test]
    fn interpreter_options_are_split_before_macros_expand() {
        assert!(plan_of(&setting("config"))
            .unwrap()
            .interpreter_options
            .is_empty());
        let setting = PhpUnitRunSetting {
            interpreter_options: "-d memory_limit=-1 -d 'include_path=$PROJECT_DIR$/lib'".into(),
            ..setting("config")
        };
        let plan = plan(
            &setting,
            |text| text.replace("$PROJECT_DIR$", r"C:\My Proj"),
            r"C:\My Proj",
            true,
        )
        .unwrap();
        assert_eq!(
            plan.interpreter_options,
            [
                "-d",
                "memory_limit=-1",
                "-d",
                r"include_path=C:\My Proj/lib"
            ]
        );
    }

    #[test]
    fn scope_fields_match_what_each_scope_reads() {
        assert_eq!(
            fields_of("config"),
            ScopeFields {
                testsuite: true,
                ..ScopeFields::default()
            }
        );
        assert_eq!(fields_of("nonsense"), fields_of("config"));
        let file = fields_of("file");
        assert!(file.path && file.filter && !file.class);
        let data_set = fields_of("data_set");
        assert!(data_set.path && data_set.class && data_set.method && data_set.data_set);
        assert!(!fields_of("method").data_set);
        assert!(fields_of("class").class && !fields_of("class").method);
        assert!(fields_of("directory").path && !fields_of("directory").filter);
    }

    #[test]
    fn a_phpunit_kind_without_a_table_is_the_default_table() {
        let bare = RunConfigSetting {
            kind: Some(PHPUNIT_KIND.into()),
            ..RunConfigSetting::default()
        };
        assert_eq!(setting_of(&bare), Some(PhpUnitRunSetting::default()));
        assert_eq!(setting_of(&RunConfigSetting::default()), None);
    }

    #[test]
    fn an_earlier_generated_name_is_refreshed_and_a_chosen_one_kept() {
        let directory = |name: &str, temporary: bool| RunConfigSetting {
            name: name.into(),
            temporary,
            ..temporary_config(
                PhpUnitRunSetting {
                    path: "tests/Unit".into(),
                    ..setting("directory")
                },
                None,
            )
        };
        assert_eq!(
            refreshed_name(&directory("tests in 'Unit'", true)).as_deref(),
            Some("Tests in 'Unit'")
        );
        assert_eq!(refreshed_name(&directory("Tests in 'Unit'", true)), None);
        assert_eq!(refreshed_name(&directory("My unit tests", true)), None);
        // A saved configuration's name is the user's, whatever it reads.
        assert_eq!(refreshed_name(&directory("tests in 'Unit'", false)), None);
        assert_eq!(refreshed_name(&RunConfigSetting::default()), None);
    }

    #[test]
    fn a_missing_working_directory_is_named() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("packages/a")).unwrap();
        let with_dir = |dir: &str| Plan {
            scope: TestScope::Config { testsuite: None },
            options: ScopeOptions::default(),
            interpreter: None,
            interpreter_options: Vec::new(),
            working_directory: Some(dir.into()),
        };
        assert_eq!(
            check_working_directory(&with_dir("packages/a"), project.path()),
            Ok(())
        );
        let missing = check_working_directory(&with_dir("packages/gone"), project.path());
        assert_eq!(
            missing.unwrap_err().to_string(),
            "has the working directory \"packages/gone\", which does not exist"
        );
        let root = Plan {
            working_directory: None,
            ..with_dir("")
        };
        assert_eq!(check_working_directory(&root, project.path()), Ok(()));
    }

    fn package_directory_run() -> RunConfigSetting {
        temporary_config(
            PhpUnitRunSetting {
                path: "packages/a/tests".into(),
                working_directory: "packages/a".into(),
                configuration: "packages/a/phpunit.xml".into(),
                ..setting("directory")
            },
            None,
        )
    }

    #[test]
    fn a_package_run_takes_over_the_temporary_an_earlier_build_stored_for_it() {
        let config = package_directory_run();
        let mut earlier = temporary_config(
            PhpUnitRunSetting {
                path: "packages/a/tests".into(),
                runner_options: "--stop-on-failure".into(),
                ..setting("directory")
            },
            Some("tests in 'tests'".into()),
        );
        earlier.env = vec![("A".into(), "1".into())];
        assert_eq!(earlier_id(&config).as_deref(), Some(earlier.id.as_str()));

        let mut stored = vec![earlier.clone()];
        let run = stored_for(&stored, config.clone());
        assert_eq!(run.id, config.id);
        assert_eq!(run.env, earlier.env, "the earlier edits are kept");
        let setting = run.phpunit.clone().unwrap();
        assert_eq!(setting.runner_options, "--stop-on-failure");
        assert_eq!(setting.working_directory, "packages/a");
        assert_eq!(setting.configuration, "packages/a/phpunit.xml");

        stored.push(run);
        forget_earlier(&mut stored, &config);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, config.id);
        // Stored under its own id now: that one wins.
        assert_eq!(stored_for(&stored, config.clone()), stored[0]);
    }

    #[test]
    fn only_temporaries_with_a_working_directory_have_an_earlier_id() {
        let root_run = temporary_config(setting("config"), None);
        assert_eq!(earlier_id(&root_run), None);
        let saved = RunConfigSetting {
            temporary: false,
            ..package_directory_run()
        };
        assert_eq!(earlier_id(&saved), None);
        // A saved configuration under the earlier id is the user's: kept.
        let earlier_saved = RunConfigSetting {
            id: earlier_id(&package_directory_run()).unwrap(),
            ..RunConfigSetting::default()
        };
        let mut stored = vec![earlier_saved];
        forget_earlier(&mut stored, &package_directory_run());
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored_for(&stored, package_directory_run()),
            package_directory_run()
        );
    }

    #[test]
    fn a_rerun_uses_the_configuration_of_the_runs_that_reported_the_tests() {
        let package = package_directory_run();
        let method = |class: &str| {
            Some(temporary_config(
                PhpUnitRunSetting {
                    class: class.into(),
                    method: "testX".into(),
                    working_directory: "packages/a".into(),
                    configuration: "packages/a/phpunit.xml".into(),
                    ..setting("method")
                },
                None,
            ))
        };
        assert_eq!(rerun_config(&[]), Ok(None));
        assert_eq!(rerun_config(&[None, None]), Ok(None));
        assert_eq!(
            rerun_config(&[Some(package.clone())]),
            Ok(Some(package.clone()))
        );

        // Two scopes in one package: the package's whole configuration.
        let both = rerun_config(&[Some(package.clone()), method("A")])
            .unwrap()
            .unwrap();
        let setting = both.phpunit.unwrap();
        assert_eq!(setting.scope, "config");
        assert_eq!(setting.path, "");
        assert_eq!(setting.working_directory, "packages/a");
        assert_eq!(setting.configuration, "packages/a/phpunit.xml");

        // A package run and a project run cannot share one process.
        assert_eq!(
            rerun_config(&[Some(package.clone()), None]),
            Err(RerunError::MixedRuns)
        );
        let mut with_env = package.clone();
        with_env.env = vec![("APP_ENV".into(), "test".into())];
        assert_eq!(
            rerun_config(&[Some(package.clone()), Some(with_env)]),
            Err(RerunError::MixedRuns)
        );
        let mut with_interpreter_options = package.clone();
        with_interpreter_options
            .phpunit
            .as_mut()
            .unwrap()
            .interpreter_options = "-d memory_limit=-1".into();
        assert_eq!(
            rerun_config(&[Some(package), Some(with_interpreter_options)]),
            Err(RerunError::MixedRuns)
        );
    }
}
