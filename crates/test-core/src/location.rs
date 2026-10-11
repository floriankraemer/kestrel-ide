//! Where a test lives in source: the TeamCity `locationHint` attribute.
//!
//! PHPUnit sends `php_qn://<abs file>::\Ns\Class` for a class and
//! `php_qn://<abs file>::\Ns\Class::method` for a method (a data-set row
//! appends ` with data set #0` or `"name"` to the method). Pest sends
//! `pest_qn://<file>::<description>` with a path relative to the run's work
//! dir, and a bare `pest_qn://<file>` for a file suite; its class-based
//! tests send `pest_qn://Greeter (Tests\Greeter)`, which names no file.
//! The hint carries no line, so [`TestLocation::line`] is only set by
//! `file://<path>:<line>` hints; the caller resolves the rest from the
//! source text.

use std::path::{Path, PathBuf};

use process_exec::host::ExecHost;

/// What a hint names inside its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSymbol {
    /// `\Ns\Class`, without the leading backslash.
    Class(String),
    Method {
        class: String,
        method: String,
    },
    /// A Pest test description, verbatim.
    Description(String),
    /// A class-based test run by Pest, `Name (Ns\Class)`: Pest drops the
    /// `Test` suffix, so this is `Ns\Class` for the declared `Ns\ClassTest`.
    PestClass(String),
    /// A method of a [`Self::PestClass`], under Pest's humanized title
    /// (`testXyzAbc` and `test_xyz_abc` both report `Xyz abc`).
    PestMethod {
        class: String,
        title: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestLocation {
    /// The path as the test process printed it: absolute on the run's host
    /// (WSL, container) or relative to the work dir. See [`Self::local_path`].
    /// Empty while [`Self::unresolved_class`] is `Some`.
    pub file: String,
    /// 1-based, when the hint carried one.
    pub line: Option<u32>,
    pub symbol: Option<TestSymbol>,
}

impl TestLocation {
    /// The file on this machine: a relative path joins `work_dir`, an
    /// absolute one maps through `host` (WSL `/home/..` becomes
    /// `\\wsl.localhost\..`, a container mount its bind source).
    pub fn local_path(&self, host: &ExecHost, work_dir: &Path) -> PathBuf {
        let printed = Path::new(&self.file);
        let absolute = self.file.starts_with('/')
            || self.file.starts_with('\\')
            || self.file.as_bytes().get(1) == Some(&b':');
        if absolute {
            host.path_from_tool(&self.file)
        } else {
            work_dir.join(printed)
        }
    }
}

impl TestLocation {
    /// The class a Pest class-based hint names but no file: the caller
    /// resolves it (PSR-4) and calls [`Self::with_file`].
    pub fn unresolved_class(&self) -> Option<&str> {
        if !self.file.is_empty() {
            return None;
        }
        match self.symbol.as_ref()? {
            TestSymbol::PestClass(class) | TestSymbol::PestMethod { class, .. } => Some(class),
            _ => None,
        }
    }

    /// This location with `file` (as [`Self::file`] means it) and, for a
    /// Pest class-based hint, the class Pest abbreviated spelled out.
    pub fn with_file(mut self, file: String, class: &str) -> Self {
        self.file = file;
        self.symbol = self.symbol.map(|symbol| match symbol {
            TestSymbol::PestClass(_) => TestSymbol::PestClass(class.to_string()),
            TestSymbol::PestMethod { title, .. } => TestSymbol::PestMethod {
                class: class.to_string(),
                title,
            },
            other => other,
        });
        self
    }
}

/// `None` for a hint that names no PHP file (other schemes). A Pest
/// class-based `Greeter (Tests\Greeter)` names a class and no file yet.
pub fn parse_location_hint(hint: &str) -> Option<TestLocation> {
    let (scheme, rest) = hint.split_once("://")?;
    if scheme == "pest_qn" {
        if let Some(location) = parse_pest_class_hint(rest) {
            return Some(location);
        }
    }
    let (file, symbol) = match rest.split_once("::") {
        Some((file, symbol)) => (file, Some(symbol)),
        None => (rest, None),
    };
    let (file, line) = match scheme {
        "file" => split_line(strip_drive_slash(file)),
        "php_qn" | "pest_qn" => (file, None),
        _ => return None,
    };
    if !file.to_ascii_lowercase().ends_with(".php") {
        return None;
    }
    Some(TestLocation {
        file: file.to_string(),
        line,
        symbol: symbol.and_then(|symbol| parse_symbol(scheme, symbol)),
    })
}

/// `Greeter (Tests\Greeter)` or `Greeter (Tests\Greeter)::Xyz abc`.
fn parse_pest_class_hint(rest: &str) -> Option<TestLocation> {
    let (head, title) = match rest.split_once("::") {
        Some((head, title)) => (head, Some(title)),
        None => (rest, None),
    };
    let class = head.strip_suffix(')')?.rsplit_once(" (")?.1;
    if class.is_empty() || head.to_ascii_lowercase().ends_with(".php)") {
        return None;
    }
    let class = class.to_string();
    let symbol = match title {
        Some(title) => TestSymbol::PestMethod {
            class,
            title: title.to_string(),
        },
        None => TestSymbol::PestClass(class),
    };
    Some(TestLocation {
        file: String::new(),
        line: None,
        symbol: Some(symbol),
    })
}

/// `file:///C:/x` -> `C:/x`.
fn strip_drive_slash(path: &str) -> &str {
    match path.strip_prefix('/') {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest,
        _ => path,
    }
}

fn split_line(path: &str) -> (&str, Option<u32>) {
    match path.rsplit_once(':') {
        Some((file, line)) if line.parse::<u32>().is_ok() => (file, line.parse().ok()),
        _ => (path, None),
    }
}

/// What the part after the file's `::` names. Pest's is a verbatim
/// description (`\Foo::bar works` is a title, not a method); PHPUnit's starts
/// with the class's leading backslash.
fn parse_symbol(scheme: &str, symbol: &str) -> Option<TestSymbol> {
    if scheme == "pest_qn" {
        return (!symbol.is_empty()).then(|| TestSymbol::Description(symbol.to_string()));
    }
    let php = symbol.strip_prefix('\\')?;
    Some(match php.split_once("::") {
        Some((class, method)) => TestSymbol::Method {
            class: class.to_string(),
            method: method
                .split_once(" with data set")
                .map_or(method, |(name, _)| name)
                .to_string(),
        },
        None => TestSymbol::Class(php.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(hint: &str) -> TestLocation {
        parse_location_hint(hint).unwrap_or_else(|| panic!("no location in {hint}"))
    }

    #[test]
    fn a_phpunit_class_hint_names_file_and_class() {
        let location = parsed(r"php_qn:///proj/tests/GreeterTest.php::\App\Tests\GreeterTest");
        assert_eq!(location.file, "/proj/tests/GreeterTest.php");
        assert_eq!(
            location.symbol,
            Some(TestSymbol::Class(r"App\Tests\GreeterTest".into()))
        );
        assert_eq!(location.line, None);
    }

    #[test]
    fn a_phpunit_method_hint_names_class_and_method() {
        let location =
            parsed(r"php_qn:///proj/tests/GreeterTest.php::\App\Tests\GreeterTest::testGreets");
        assert_eq!(
            location.symbol,
            Some(TestSymbol::Method {
                class: r"App\Tests\GreeterTest".into(),
                method: "testGreets".into()
            })
        );
    }

    #[test]
    fn a_data_set_row_locates_the_method() {
        for suffix in [" with data set #0", r#" with data set "empty name""#] {
            let location = parsed(&format!(r"php_qn:///p/T.php::\T::testAdds{suffix}"));
            assert_eq!(
                location.symbol,
                Some(TestSymbol::Method {
                    class: "T".into(),
                    method: "testAdds".into()
                })
            );
        }
    }

    #[test]
    fn a_windows_path_keeps_its_drive_colon() {
        let location = parsed(r"php_qn://C:\proj\tests\T.php::\T::testA");
        assert_eq!(location.file, r"C:\proj\tests\T.php");
    }

    #[test]
    fn pest_hints_carry_a_relative_file_and_a_description() {
        let location = parsed("pest_qn://tests/Feature/GreeterPestTest.php::it greets by name");
        assert_eq!(location.file, "tests/Feature/GreeterPestTest.php");
        assert_eq!(
            location.symbol,
            Some(TestSymbol::Description("it greets by name".into()))
        );
        assert_eq!(parsed("pest_qn://tests/A.php").symbol, None);
    }

    #[test]
    fn a_pest_class_hint_names_a_class_and_no_file_yet() {
        let class = parsed(r"pest_qn://Greeter (Tests\Greeter)");
        assert_eq!(class.file, "");
        assert_eq!(class.unresolved_class(), Some(r"Tests\Greeter"));
        let method = parsed(r"pest_qn://Class (Tests\Class)::Xyz abc");
        assert_eq!(
            method.symbol,
            Some(TestSymbol::PestMethod {
                class: r"Tests\Class".into(),
                title: "Xyz abc".into()
            })
        );
        let resolved = method.with_file("tests/ClassTest.php".into(), r"Tests\ClassTest");
        assert_eq!(resolved.unresolved_class(), None);
        assert_eq!(
            resolved.symbol,
            Some(TestSymbol::PestMethod {
                class: r"Tests\ClassTest".into(),
                title: "Xyz abc".into()
            })
        );
    }

    #[test]
    fn a_pest_description_that_looks_like_a_method_stays_a_description() {
        let location = parsed(r"pest_qn://tests/A.php::\Foo::bar works");
        assert_eq!(
            location.symbol,
            Some(TestSymbol::Description(r"\Foo::bar works".into()))
        );
    }

    #[test]
    fn a_hint_without_a_php_file_has_no_location() {
        assert_eq!(parse_location_hint("java:test://com.a.ATest/testX"), None);
        assert_eq!(parse_location_hint("nonsense"), None);
    }

    #[test]
    fn a_file_hint_may_carry_a_line() {
        let location = parsed("file:///proj/tests/T.php:42");
        assert_eq!(
            (location.file.as_str(), location.line),
            ("/proj/tests/T.php", Some(42))
        );
        assert_eq!(parsed("file:///C:/proj/T.php:7").file, "C:/proj/T.php");
    }

    #[test]
    fn local_path_joins_relative_files_and_maps_absolute_ones() {
        let local = ExecHost::Local;
        let work = Path::new("/proj");
        assert_eq!(
            parsed("pest_qn://tests/A.php").local_path(&local, work),
            Path::new("/proj/tests/A.php")
        );
        assert_eq!(
            parsed(r"php_qn:///x/T.php::\T").local_path(&local, work),
            Path::new("/x/T.php")
        );
    }
}
