//! The argv a test scope and its options turn into (PHPUnit editor runs
//! plan, E4, ADR-0073): what a `phpunit` run configuration, a caret run and a
//! gutter run all launch.
//!
//! Pure: the caller has already validated the scope's required fields (the
//! types make a missing one unrepresentable) and expanded macros. Paths are
//! project-relative with `/`, because the run's working directory is the
//! project root on every host (local, WSL, container).

use crate::filter::{apply_filter, escape_regex, FilterDialect};

/// The infix PHPUnit and Pest put between a test name and its data set.
const DATA_SET_INFIX: &str = " with data set ";

/// What a run covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestScope {
    Directory {
        path: String,
    },
    /// A file, optionally narrowed by a PCRE `--filter` (a Pest `it()` or
    /// `describe()`, which have no class).
    File {
        path: String,
        filter: Option<String>,
    },
    Class {
        class: String,
        path: Option<String>,
    },
    Method {
        class: String,
        method: String,
        path: Option<String>,
    },
    /// One data set; `data_set` is the text after `with data set `.
    DataSet {
        class: String,
        method: String,
        data_set: String,
        path: Option<String>,
    },
    /// What the configuration file selects, or one of its test suites.
    Config {
        testsuite: Option<String>,
    },
}

impl TestScope {
    /// The scope's name as a settings file spells it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Directory { .. } => "directory",
            Self::File { .. } => "file",
            Self::Class { .. } => "class",
            Self::Method { .. } => "method",
            Self::DataSet { .. } => "data_set",
            Self::Config { .. } => "config",
        }
    }
}

/// The options every scope accepts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeOptions {
    /// An alternative configuration file, project-relative.
    pub configuration: Option<String>,
    pub groups: Vec<String>,
    pub exclude_groups: Vec<String>,
    /// Appended last, already split into words.
    pub runner_options: Vec<String>,
}

/// Why a framework cannot run a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    /// The framework's dialect has no way to select this scope.
    UnsupportedScope(&'static str),
    /// The framework's dialect has no flag for this option.
    UnsupportedOption(&'static str),
    /// Codeception selects a test by file, so the scope needs one.
    NeedsFile(&'static str),
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedScope(scope) => write!(
                f,
                "This test framework cannot run a {scope} scope; run the whole suite instead."
            ),
            Self::UnsupportedOption(option) => {
                write!(f, "This test framework does not support {option}.")
            }
            Self::NeedsFile(scope) => write!(
                f,
                "This test framework selects tests by file, so the {scope} scope needs a test file."
            ),
        }
    }
}

impl std::error::Error for ScopeError {}

/// The PCRE selecting every test of `class`.
fn class_filter(class: &str) -> String {
    format!("^{}::", escape_regex(class))
}

/// The PCRE selecting `method`, with or without data sets.
fn method_filter(class: &str, method: &str) -> String {
    format!(
        "^{}::{}( with data set .+)?$",
        escape_regex(class),
        escape_regex(method)
    )
}

fn data_set_filter(class: &str, method: &str, data_set: &str) -> String {
    format!(
        "^{}::{}{DATA_SET_INFIX}{}$",
        escape_regex(class),
        escape_regex(method),
        escape_regex(data_set)
    )
}

/// Split `Class::method with data set "x"` into its three parts; `None` for
/// anything that is not a data-set row of a method.
pub fn split_data_set_id(id: &str) -> Option<(&str, &str, &str)> {
    let (class, rest) = id.split_once("::")?;
    let (method, data_set) = rest.split_once(DATA_SET_INFIX)?;
    let named = data_set.starts_with('"') || data_set.starts_with('#');
    (!class.is_empty() && !method.is_empty() && named).then_some((class, method, data_set))
}

/// The full argv: `base`, the scope's selection, the options, then the
/// scope's path last (PHPUnit's positional argument).
///
/// PHPUnit and Pest share one dialect here (Pest speaks PHPUnit's flags).
/// Codeception selects by `file` / `file:^method$` and has no flag for the
/// options; every other dialect refuses.
pub fn scope_args(
    base: &[String],
    dialect: FilterDialect,
    filter_flag: Option<&str>,
    filter_template: Option<&str>,
    scope: &TestScope,
    options: &ScopeOptions,
) -> Result<Vec<String>, ScopeError> {
    match dialect {
        FilterDialect::PhpUnitRegex | FilterDialect::PestRegex => Ok(phpunit_args(
            base,
            filter_flag,
            filter_template,
            scope,
            options,
        )),
        FilterDialect::Codeception => codeception_args(base, scope, options),
        _ => Err(ScopeError::UnsupportedScope(scope.name())),
    }
}

fn phpunit_args(
    base: &[String],
    filter_flag: Option<&str>,
    filter_template: Option<&str>,
    scope: &TestScope,
    options: &ScopeOptions,
) -> Vec<String> {
    let (filter, path, testsuite) = match scope {
        TestScope::Directory { path } => (None, Some(path), None),
        TestScope::File { path, filter } => (filter.clone(), Some(path), None),
        TestScope::Class { class, path } => (Some(class_filter(class)), path.as_ref(), None),
        TestScope::Method {
            class,
            method,
            path,
        } => (Some(method_filter(class, method)), path.as_ref(), None),
        TestScope::DataSet {
            class,
            method,
            data_set,
            path,
        } => (
            Some(data_set_filter(class, method, data_set)),
            path.as_ref(),
            None,
        ),
        TestScope::Config { testsuite } => (None, None, testsuite.as_ref()),
    };
    let mut args = match filter {
        Some(pattern) => apply_filter(base, filter_flag, filter_template, &[pattern]),
        None => base.to_vec(),
    };
    if let Some(testsuite) = testsuite {
        args.extend(["--testsuite".to_string(), testsuite.clone()]);
    }
    if let Some(configuration) = &options.configuration {
        args.extend(["--configuration".to_string(), configuration.clone()]);
    }
    if !options.groups.is_empty() {
        args.extend(["--group".to_string(), options.groups.join(",")]);
    }
    if !options.exclude_groups.is_empty() {
        args.extend([
            "--exclude-group".to_string(),
            options.exclude_groups.join(","),
        ]);
    }
    args.extend(options.runner_options.iter().cloned());
    args.extend(path.cloned());
    args
}

fn codeception_args(
    base: &[String],
    scope: &TestScope,
    options: &ScopeOptions,
) -> Result<Vec<String>, ScopeError> {
    if options.configuration.is_some() {
        return Err(ScopeError::UnsupportedOption(
            "an alternative configuration file",
        ));
    }
    if !options.groups.is_empty() || !options.exclude_groups.is_empty() {
        return Err(ScopeError::UnsupportedOption("groups"));
    }
    let target = match scope {
        TestScope::Directory { path } => Some(path.clone()),
        TestScope::File { path, filter: None } => Some(path.clone()),
        TestScope::Config { testsuite: None } => None,
        TestScope::Class {
            path: Some(path), ..
        } => Some(path.clone()),
        TestScope::Method {
            method,
            path: Some(path),
            ..
        } => Some(format!("{path}:^{}$", escape_regex(method))),
        TestScope::Class { path: None, .. } | TestScope::Method { path: None, .. } => {
            return Err(ScopeError::NeedsFile(scope.name()))
        }
        _ => return Err(ScopeError::UnsupportedScope(scope.name())),
    };
    let mut args = base.to_vec();
    args.extend(options.runner_options.iter().cloned());
    args.extend(target);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<String> {
        vec!["--teamcity".to_string()]
    }

    fn phpunit(scope: &TestScope, options: &ScopeOptions) -> Vec<String> {
        scope_args(
            &base(),
            FilterDialect::PhpUnitRegex,
            Some("--filter"),
            None,
            scope,
            options,
        )
        .unwrap()
    }

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn directory_and_file_scopes_pass_their_path_last() {
        let options = ScopeOptions::default();
        assert_eq!(
            phpunit(
                &TestScope::Directory {
                    path: "tests/Unit".into()
                },
                &options
            ),
            words(&["--teamcity", "tests/Unit"])
        );
        assert_eq!(
            phpunit(
                &TestScope::File {
                    path: "tests/ATest.php".into(),
                    filter: None
                },
                &options
            ),
            words(&["--teamcity", "tests/ATest.php"])
        );
    }

    #[test]
    fn a_file_scope_can_carry_a_filter() {
        let scope = TestScope::File {
            path: "tests/Pest.php".into(),
            filter: Some("it greets".into()),
        };
        assert_eq!(
            phpunit(&scope, &ScopeOptions::default()),
            words(&["--teamcity", "--filter", "it greets", "tests/Pest.php"])
        );
    }

    #[test]
    fn a_class_filters_by_the_escaped_qualified_name() {
        let scope = TestScope::Class {
            class: "App\\GreeterTest".into(),
            path: Some("tests/GreeterTest.php".into()),
        };
        assert_eq!(
            phpunit(&scope, &ScopeOptions::default()),
            words(&[
                "--teamcity",
                "--filter",
                "^App\\\\GreeterTest::",
                "tests/GreeterTest.php"
            ])
        );
    }

    #[test]
    fn a_method_matches_its_data_sets_too() {
        let scope = TestScope::Method {
            class: "App\\GreeterTest".into(),
            method: "testGreets".into(),
            path: None,
        };
        assert_eq!(
            phpunit(&scope, &ScopeOptions::default()),
            words(&[
                "--teamcity",
                "--filter",
                "^App\\\\GreeterTest::testGreets( with data set .+)?$"
            ])
        );
    }

    #[test]
    fn a_data_set_is_matched_exactly() {
        let scope = TestScope::DataSet {
            class: "App\\GreeterTest".into(),
            method: "testGreets".into(),
            data_set: "\"a (b)\"".into(),
            path: Some("t.php".into()),
        };
        assert_eq!(
            phpunit(&scope, &ScopeOptions::default()),
            words(&[
                "--teamcity",
                "--filter",
                "^App\\\\GreeterTest::testGreets with data set \"a \\(b\\)\"$",
                "t.php"
            ])
        );
    }

    #[test]
    fn the_config_scope_selects_a_test_suite_or_nothing() {
        let options = ScopeOptions::default();
        assert_eq!(
            phpunit(&TestScope::Config { testsuite: None }, &options),
            base()
        );
        assert_eq!(
            phpunit(
                &TestScope::Config {
                    testsuite: Some("Unit".into())
                },
                &options
            ),
            words(&["--teamcity", "--testsuite", "Unit"])
        );
    }

    #[test]
    fn options_follow_the_selection_and_precede_the_path() {
        let options = ScopeOptions {
            configuration: Some("ci/phpunit.xml".into()),
            groups: vec!["a".into(), "b".into()],
            exclude_groups: vec!["slow".into()],
            runner_options: words(&["--stop-on-failure", "--colors=never"]),
        };
        assert_eq!(
            phpunit(
                &TestScope::Directory {
                    path: "tests".into()
                },
                &options
            ),
            words(&[
                "--teamcity",
                "--configuration",
                "ci/phpunit.xml",
                "--group",
                "a,b",
                "--exclude-group",
                "slow",
                "--stop-on-failure",
                "--colors=never",
                "tests"
            ])
        );
    }

    #[test]
    fn pest_shares_phpunits_dialect() {
        let scope = TestScope::Directory {
            path: "tests".into(),
        };
        let args = scope_args(
            &base(),
            FilterDialect::PestRegex,
            Some("--filter"),
            None,
            &scope,
            &ScopeOptions::default(),
        );
        assert_eq!(args.unwrap(), words(&["--teamcity", "tests"]));
    }

    #[test]
    fn codeception_selects_by_file_and_refuses_what_it_has_no_flag_for() {
        let run = |scope: &TestScope, options: &ScopeOptions| {
            scope_args(
                &words(&["run", "--xml"]),
                FilterDialect::Codeception,
                Some("--filter"),
                None,
                scope,
                options,
            )
        };
        let none = ScopeOptions::default();
        let method = TestScope::Method {
            class: "A".into(),
            method: "testA".into(),
            path: Some("tests/ATest.php".into()),
        };
        assert_eq!(
            run(&method, &none).unwrap(),
            words(&["run", "--xml", "tests/ATest.php:^testA$"])
        );
        assert_eq!(
            run(&TestScope::Config { testsuite: None }, &none).unwrap(),
            words(&["run", "--xml"])
        );
        let class_without_file = TestScope::Class {
            class: "A".into(),
            path: None,
        };
        assert_eq!(
            run(&class_without_file, &none),
            Err(ScopeError::NeedsFile("class"))
        );
        let data_set = TestScope::DataSet {
            class: "A".into(),
            method: "m".into(),
            data_set: "#0".into(),
            path: Some("f".into()),
        };
        assert_eq!(
            run(&data_set, &none),
            Err(ScopeError::UnsupportedScope("data_set"))
        );
        let grouped = ScopeOptions {
            groups: vec!["g".into()],
            ..ScopeOptions::default()
        };
        assert_eq!(
            run(&method, &grouped),
            Err(ScopeError::UnsupportedOption("groups"))
        );
    }

    #[test]
    fn frameworks_without_a_name_filter_refuse_every_scope() {
        let error = scope_args(
            &base(),
            FilterDialect::None,
            Some("--example"),
            None,
            &TestScope::Config { testsuite: None },
            &ScopeOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("config"));
    }

    #[test]
    fn a_data_set_row_id_splits_into_class_method_and_set() {
        assert_eq!(
            split_data_set_id("App\\T::testA with data set \"x y\""),
            Some(("App\\T", "testA", "\"x y\""))
        );
        assert_eq!(
            split_data_set_id("App\\T::testA with data set #1"),
            Some(("App\\T", "testA", "#1"))
        );
        assert_eq!(split_data_set_id("App\\T::testA"), None);
        assert_eq!(split_data_set_id("App\\T::testA with data set x"), None);
    }
}
