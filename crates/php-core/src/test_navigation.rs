//! Go to Test / Go to Test Subject (PHPUnit editor runs plan, E7): from a
//! class to the tests that exercise it, and from a test back to its subject.
//!
//! The mapping is PhpStorm's: PSR-4 `autoload` roots hold subjects and
//! `autoload-dev` roots hold tests; a test is named `<Subject>Test`, in the
//! subject's namespace mirrored under a test root (optionally below `Unit\`,
//! `Integration\`, `Feature\` or `Functional\`) or in the subject's own
//! namespace; `#[CoversClass]`, `#[CoversMethod]`, `@covers` and Pest's
//! `covers()` name a test's subjects explicitly. Pest files, which declare
//! no class, are found by file name under the test roots.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::composer::{ComposerJson, Psr4Root};
use crate::phpunit_xml::{self, Budget};
use crate::psr4;

/// The suffix a test class or file carries.
const TEST_SUFFIX: &str = "Test";

/// Namespace segments a test root commonly inserts before the mirrored
/// subject namespace: `Tests\Unit\Foo\BarTest` tests `App\Foo\Bar`.
const TEST_KINDS: &[&str] = &["Unit", "Integration", "Feature", "Functional"];

/// Where a jump from one PHP file leads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestNavigation {
    /// From a test to its subjects (`true`), or from a class to its tests.
    pub from_test: bool,
    /// The class (or, for a Pest file, the file name) the jump starts from,
    /// for the chooser's title and the "nothing found" message.
    pub name: String,
    /// Project-relative, `/`-separated files, most likely first.
    pub files: Vec<String>,
}

/// What a PHP file declares, as far as the mapping needs it.
#[derive(Debug, Default)]
struct Declarations {
    namespace: String,
    /// `use` imports: alias -> fully qualified name.
    imports: Vec<(String, String)>,
    /// The first class, interface, trait or enum.
    class: Option<String>,
}

impl Declarations {
    fn read(text: &str) -> Self {
        let mut declarations = Self::default();
        if let Some(tree) = crate::tests::parse(text) {
            declarations.visit(text, tree.root_node());
        }
        declarations
    }

    fn visit(&mut self, text: &str, node: Node<'_>) {
        let text_of = |node: Node<'_>| text[node.byte_range()].to_string();
        match node.kind() {
            "namespace_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.namespace = text_of(name);
                }
            }
            "namespace_use_declaration" => self.imports.extend(imports_of(&text_of(node))),
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration" => {
                if self.class.is_none() {
                    self.class = node.child_by_field_name("name").map(text_of);
                }
                return;
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(text, child);
        }
    }

    fn qualify(&self, name: &str) -> String {
        join(&self.namespace, name)
    }

    /// `name` as written in this file, fully qualified: a leading `\` is
    /// already absolute, an imported first segment expands to its import,
    /// anything else is relative to the file's namespace.
    fn resolve(&self, name: &str) -> String {
        if let Some(absolute) = name.strip_prefix('\\') {
            return absolute.to_string();
        }
        let (first, rest) = name.split_once('\\').unwrap_or((name, ""));
        match self.imports.iter().find(|(alias, _)| alias == first) {
            Some((_, imported)) => join(imported, rest),
            None => self.qualify(name),
        }
    }
}

fn join(namespace: &str, name: &str) -> String {
    match (namespace.is_empty(), name.is_empty()) {
        (true, _) => name.to_string(),
        (_, true) => namespace.to_string(),
        _ => format!("{namespace}\\{name}"),
    }
}

/// The class imports of one `use` declaration (function and constant
/// imports are skipped), group imports (`use App\{A, B as C};`) included.
fn imports_of(declaration: &str) -> Vec<(String, String)> {
    let body = declaration
        .trim()
        .trim_start_matches("use")
        .trim()
        .trim_end_matches(';')
        .trim();
    if body.starts_with("function ") || body.starts_with("const ") {
        return Vec::new();
    }
    let (prefix, clauses) = match body.split_once('{') {
        Some((prefix, group)) => (
            prefix.trim().trim_end_matches('\\'),
            group.trim_end_matches('}'),
        ),
        None => ("", body),
    };
    clauses
        .split(',')
        .filter_map(|clause| {
            let clause = clause.trim();
            let (name, alias) = match clause.split_once(" as ") {
                Some((name, alias)) => (name.trim(), alias.trim()),
                None => (clause, clause.rsplit('\\').next().unwrap_or(clause)),
            };
            let name = join(prefix, name.trim_start_matches('\\'));
            (!name.is_empty() && !alias.is_empty()).then(|| (alias.to_string(), name))
        })
        .collect()
}

/// A class name a test says it covers, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoveredName {
    name: String,
    /// From a `@covers` annotation, whose names are fully qualified with or
    /// without the leading `\`; code (`::class`) resolves as PHP does.
    annotation: bool,
}

/// The classes a test file declaring `declarations` says it covers, fully
/// qualified. An annotation's name is tried as written first, then as code
/// would resolve it, since both spellings occur.
fn covered_classes(text: &str, declarations: &Declarations) -> Vec<String> {
    let mut classes = Vec::new();
    for covered in covered_names(text) {
        if covered.annotation {
            classes.push(covered.name.trim_start_matches('\\').to_string());
        }
        let resolved = declarations.resolve(&covered.name);
        if !classes.contains(&resolved) {
            classes.push(resolved);
        }
    }
    classes
}

/// The class names a test's coverage attributes, `@covers` annotations and
/// Pest `covers()` calls name, as written.
fn covered_names(text: &str) -> Vec<CoveredName> {
    let mut names = Vec::new();
    let class_constant = |argument: &str| {
        argument
            .trim()
            .strip_suffix("::class")
            .map(|name| CoveredName {
                name: name.trim().to_string(),
                annotation: false,
            })
    };
    for attribute in ["CoversClass(", "CoversMethod("] {
        for (at, _) in text.match_indices(attribute) {
            let arguments = &text[at + attribute.len()..];
            let first = arguments.split([',', ')']).next().unwrap_or_default();
            names.extend(class_constant(first));
        }
    }
    for (at, _) in text.match_indices("covers(") {
        let starts_word = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '\\'));
        if !starts_word {
            continue;
        }
        let arguments = text[at + "covers(".len()..]
            .split(')')
            .next()
            .unwrap_or_default();
        names.extend(arguments.split(',').filter_map(class_constant));
    }
    for (at, _) in text.match_indices("@covers") {
        let rest = &text[at + "@covers".len()..];
        // `@coversDefaultClass` and `@coversNothing` are other tags.
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let target = rest.split_whitespace().next().unwrap_or_default();
        let class = target.split("::").next().unwrap_or_default();
        if !class.is_empty() && !class.starts_with('*') {
            names.push(CoveredName {
                name: class.to_string(),
                annotation: true,
            });
        }
    }
    names
}

/// Whether `text` is a test file: one with PHPUnit or Pest tests, or named
/// `*Test.php`. Decides which way Go to Test jumps and how the menu names it.
pub fn is_test_file(path: &Path, text: &str) -> bool {
    is_named_test(path) || !crate::tests::markers(text).is_empty()
}

/// Whether `path` is named `*Test.php`: [`is_test_file`] without parsing,
/// for a caller that holds the file's markers already.
pub fn is_named_test(path: &Path) -> bool {
    path.file_stem()
        .is_some_and(|stem| stem.to_string_lossy().ends_with(TEST_SUFFIX))
}

/// Go to Test / Test Subject for `file` (absolute, holding `text`) in the
/// project at `project`, with the PSR-4 roots read from its `composer.json`.
/// Blocking filesystem work: call it off the GUI thread.
pub fn navigate(project: &Path, file: &Path, text: &str) -> TestNavigation {
    let roots = match ComposerJson::read(project) {
        Ok(Some(composer)) => composer.psr4,
        _ => Vec::new(),
    };
    navigate_with(&roots, project, file, text)
}

/// [`navigate`] over explicit roots.
pub fn navigate_with(
    roots: &[Psr4Root],
    project: &Path,
    file: &Path,
    text: &str,
) -> TestNavigation {
    let declarations = Declarations::read(text);
    let current = relative(project, file);
    let mut navigation = if is_test_file(file, text) {
        subjects_of(roots, project, file, text, &declarations)
    } else {
        tests_of(roots, project, file, &declarations)
    };
    let mut seen = std::collections::HashSet::new();
    navigation
        .files
        .retain(|candidate| Some(candidate) != current.as_ref() && seen.insert(candidate.clone()));
    navigation
}

fn relative(project: &Path, file: &Path) -> Option<String> {
    let rest = file.strip_prefix(project).ok()?;
    Some(rest.to_string_lossy().replace('\\', "/"))
}

fn file_stem(file: &Path) -> String {
    file.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A test's subjects: what it says it covers, then the `<Name>Test`
/// convention mapped back from the test root to every source root.
fn subjects_of(
    roots: &[Psr4Root],
    project: &Path,
    file: &Path,
    text: &str,
    declarations: &Declarations,
) -> TestNavigation {
    let name = declarations
        .class
        .clone()
        .unwrap_or_else(|| file_stem(file));
    // A Pest file declares no namespace; its directory's PSR-4 one stands in.
    let namespace = if declarations.class.is_some() {
        declarations.namespace.clone()
    } else {
        let dir = file.parent().unwrap_or(project);
        psr4::namespace_for(roots, project, dir)
    };
    let mut classes = covered_classes(text, declarations);
    let subject = name.strip_suffix(TEST_SUFFIX).filter(|s| !s.is_empty());
    if let Some(subject) = subject {
        classes.push(join(&namespace, subject));
        let test_roots = roots.iter().filter(|root| root.dev);
        if let Some(rest) = below_longest_prefix(test_roots, &namespace) {
            let mut mirrored = vec![rest.clone()];
            let (kind, below) = rest.split_once('\\').unwrap_or((&rest, ""));
            if TEST_KINDS.contains(&kind) {
                mirrored.push(below.to_string());
            }
            for source_root in roots.iter().filter(|root| !root.dev) {
                for namespace in &mirrored {
                    let class = join(
                        &join(source_root.prefix.trim_end_matches('\\'), namespace),
                        subject,
                    );
                    classes.push(class);
                }
            }
        }
    }
    let mut files: Vec<String> = classes
        .iter()
        .filter_map(|class| psr4::class_file(roots, project, class))
        .collect();
    // A layout the namespaces do not mirror (Laravel's flat `tests/Feature`
    // for `app/Http/Controllers`): the subject's file by name.
    if let (true, Some(subject)) = (files.is_empty(), subject) {
        files = scan_sources(roots, project, subject);
    }
    TestNavigation {
        from_test: true,
        name,
        files,
    }
}

/// `namespace` below the longest of `roots`' prefixes that holds it, so
/// `App\Legacy\Foo` maps through `App\Legacy\` and not also through `App\`.
fn below_longest_prefix<'a>(
    roots: impl Iterator<Item = &'a Psr4Root>,
    namespace: &str,
) -> Option<String> {
    roots
        .filter_map(|root| {
            let rest = strip_namespace(namespace, &root.prefix)?;
            Some((root.prefix.trim_end_matches('\\').len(), rest))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, rest)| rest)
}

/// `namespace` below `prefix` (`App\` or `App`), `None` when it is not.
fn strip_namespace(namespace: &str, prefix: &str) -> Option<String> {
    let prefix = prefix.trim_end_matches('\\');
    if prefix.is_empty() {
        return Some(namespace.to_string());
    }
    if namespace == prefix {
        return Some(String::new());
    }
    namespace
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('\\'))
        .map(str::to_string)
}

/// A class's tests: the `<Name>Test` convention under every test root and in
/// the class's own namespace; only when that finds none, a bounded scan of
/// the test directories for `<Name>Test.php` files (Pest, or a layout PSR-4
/// does not mirror) and for tests that say they cover the class.
fn tests_of(
    roots: &[Psr4Root],
    project: &Path,
    file: &Path,
    declarations: &Declarations,
) -> TestNavigation {
    let name = declarations
        .class
        .clone()
        .unwrap_or_else(|| file_stem(file));
    let subject = declarations.qualify(&name);
    let test_name = format!("{name}{TEST_SUFFIX}");
    let mut classes = vec![declarations.qualify(&test_name)];
    let source_roots = roots.iter().filter(|root| !root.dev);
    if let Some(rest) = below_longest_prefix(source_roots, &declarations.namespace) {
        for test_root in roots.iter().filter(|root| root.dev) {
            let test_prefix = test_root.prefix.trim_end_matches('\\');
            for kind in std::iter::once("").chain(TEST_KINDS.iter().copied()) {
                let namespace = join(&join(test_prefix, kind), &rest);
                classes.push(join(&namespace, &test_name));
            }
        }
    }
    let mut files: Vec<String> = classes
        .iter()
        .filter_map(|class| psr4::class_file(roots, project, class))
        .collect();
    if files.is_empty() {
        files = scan_tests(roots, project, &name, &subject);
    }
    TestNavigation {
        from_test: false,
        name,
        files,
    }
}

/// The test directories: every `autoload-dev` root, else `tests/`.
fn test_dirs(roots: &[Psr4Root], project: &Path) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = roots
        .iter()
        .filter(|root| root.dev)
        .map(|root| project.join(&root.dir))
        .collect();
    if dirs.is_empty() {
        vec![project.join("tests")]
    } else {
        dirs
    }
}

/// The `*Test.php` files under the test directories that are named after
/// `name` or cover `subject`.
fn scan_tests(roots: &[Psr4Root], project: &Path, name: &str, subject: &str) -> Vec<String> {
    let wanted = format!("{name}{TEST_SUFFIX}.php");
    scan(
        project,
        test_dirs(roots, project),
        |path, file_name, budget| {
            file_name.ends_with("Test.php")
                && (file_name == wanted || covers(path, name, subject, budget))
        },
    )
}

/// The `<subject>.php` files under the source (`autoload`) directories.
fn scan_sources(roots: &[Psr4Root], project: &Path, subject: &str) -> Vec<String> {
    let wanted = format!("{subject}.php");
    let dirs = roots
        .iter()
        .filter(|root| !root.dev)
        .map(|root| project.join(&root.dir))
        .collect();
    scan(project, dirs, |_, file_name, _| file_name == wanted)
}

/// The project-relative files under `dirs` that `wanted` accepts (given the
/// path, the file name and the budget left), sorted; within the budget the
/// PHPUnit model's scan uses, so a huge tree cannot stall the worker.
fn scan(
    project: &Path,
    mut dirs: Vec<PathBuf>,
    mut wanted: impl FnMut(&Path, &str, &mut Budget) -> bool,
) -> Vec<String> {
    let mut budget = Budget {
        files: phpunit_xml::MAX_SCANNED_FILES,
        entries: phpunit_xml::MAX_VISITED_ENTRIES,
    };
    let mut found = Vec::new();
    dirs.sort();
    dirs.dedup();
    while let Some(dir) = dirs.pop() {
        if budget.spent() {
            break;
        }
        for (path, is_dir, is_file) in phpunit_xml::entries_of(&dir, &mut budget) {
            if is_dir && !phpunit_xml::is_skipped(&path) {
                dirs.push(path);
                continue;
            }
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if file_name.is_some_and(|name| is_file && wanted(&path, &name, &mut budget)) {
                found.extend(relative(project, &path));
            }
        }
    }
    found.sort();
    found
}

/// Whether the test file at `path` says it covers `subject`. Only files that
/// mention the class's short `name` at all are parsed.
fn covers(path: &Path, name: &str, subject: &str, budget: &mut Budget) -> bool {
    if budget.files == 0 {
        return false;
    }
    budget.files -= 1;
    let small = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= phpunit_xml::MAX_FILE_BYTES);
    let Some(text) = small.then(|| std::fs::read_to_string(path).ok()).flatten() else {
        return false;
    };
    if !text.contains(name) {
        return false;
    }
    covered_classes(&text, &Declarations::read(&text))
        .iter()
        .any(|covered| covered == subject)
}

/// Go to Test's requests: one search runs at a time and only the latest
/// request's answer is used. A request made while a search runs waits for
/// it, replacing any request already waiting, so repeated presses never
/// pile up scans and a late answer never jumps after a newer request.
#[derive(Debug)]
pub struct NavigationRequests<R> {
    tracker: lsp_core::RequestTracker,
    running: bool,
    waiting: Option<(u64, R)>,
}

impl<R> Default for NavigationRequests<R> {
    fn default() -> Self {
        Self {
            tracker: lsp_core::RequestTracker::default(),
            running: false,
            waiting: None,
        }
    }
}

impl<R> NavigationRequests<R> {
    /// A new request, with the token its answer carries: returned to start
    /// now, or `None` while another search runs (it then starts from
    /// [`Self::finish`]).
    pub fn request(&mut self, request: R) -> Option<(u64, R)> {
        let token = self.tracker.begin();
        if self.running {
            self.waiting = Some((token, request));
            return None;
        }
        self.running = true;
        Some((token, request))
    }

    /// The search for `token` finished: whether its answer is still wanted,
    /// and the waiting request to start next.
    pub fn finish(&mut self, token: u64) -> (bool, Option<(u64, R)>) {
        let wanted = self.tracker.accept(token);
        let next = self.waiting.take();
        self.running = next.is_some();
        (wanted, next)
    }

    /// No answer in flight is wanted any more: the user moved on.
    pub fn cancel(&mut self) {
        self.tracker.cancel();
        self.waiting = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(prefix: &str, dir: &str, dev: bool) -> Psr4Root {
        Psr4Root {
            prefix: prefix.into(),
            dir: dir.into(),
            dev,
        }
    }

    fn roots() -> Vec<Psr4Root> {
        vec![
            root("App\\", "src/", false),
            root("Tests\\", "tests/", true),
        ]
    }

    fn write(project: &Path, file: &str, text: &str) {
        let path = project.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const SUBJECT: &str = "<?php\nnamespace App\\Foo;\n\nfinal class Bar {}\n";

    fn test_class(namespace: &str, class: &str, extra: &str) -> String {
        format!(
            "<?php\nnamespace {namespace};\n\nuse PHPUnit\\Framework\\TestCase;\n{extra}\n\
             final class {class} extends TestCase\n{{\n    public function testIt(): void {{}}\n}}\n"
        )
    }

    fn navigate_from(project: &Path, file: &str) -> TestNavigation {
        let text = std::fs::read_to_string(project.join(file)).unwrap();
        navigate_with(&roots(), project, &project.join(file), &text)
    }

    #[test]
    fn a_class_leads_to_its_mirrored_tests_under_every_test_kind() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Foo/BarTest.php",
            &test_class("Tests\\Foo", "BarTest", ""),
        );
        write(
            project,
            "tests/Unit/Foo/BarTest.php",
            &test_class("Tests\\Unit\\Foo", "BarTest", ""),
        );
        write(
            project,
            "tests/Unit/Foo/OtherTest.php",
            &test_class("Tests\\Unit\\Foo", "OtherTest", ""),
        );

        let navigation = navigate_from(project, "src/Foo/Bar.php");

        assert!(!navigation.from_test);
        assert_eq!(navigation.name, "Bar");
        assert_eq!(
            navigation.files,
            ["tests/Foo/BarTest.php", "tests/Unit/Foo/BarTest.php"]
        );
    }

    #[test]
    fn a_test_leads_back_to_its_subject_across_the_kind_segment() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Integration/Foo/BarTest.php",
            &test_class("Tests\\Integration\\Foo", "BarTest", ""),
        );

        let navigation = navigate_from(project, "tests/Integration/Foo/BarTest.php");

        assert!(navigation.from_test);
        assert_eq!(navigation.name, "BarTest");
        assert_eq!(navigation.files, ["src/Foo/Bar.php"]);
    }

    #[test]
    fn a_test_in_the_subjects_own_namespace_is_found_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "src/Foo/BarTest.php",
            &test_class("App\\Foo", "BarTest", ""),
        );

        assert_eq!(
            navigate_from(project, "src/Foo/Bar.php").files,
            ["src/Foo/BarTest.php"]
        );
        assert_eq!(
            navigate_from(project, "src/Foo/BarTest.php").files,
            ["src/Foo/Bar.php"]
        );
    }

    #[test]
    fn covers_attributes_and_annotations_name_subjects_through_imports() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "src/Baz.php",
            "<?php\nnamespace App;\nclass Baz {}\n",
        );
        write(
            project,
            "src/Qux.php",
            "<?php\nnamespace App;\nclass Qux {}\n",
        );
        write(
            project,
            "tests/Acceptance/CheckoutTest.php",
            &test_class(
                "Tests\\Acceptance",
                "CheckoutTest",
                "use App\\Foo\\Bar;\nuse App\\{Baz as B};\n\
                 #[\\PHPUnit\\Framework\\Attributes\\CoversClass(Bar::class)]\n\
                 #[CoversMethod(B::class, 'run')]\n/** @covers \\App\\Qux::go */",
            ),
        );

        let navigation = navigate_from(project, "tests/Acceptance/CheckoutTest.php");

        assert_eq!(
            navigation.files,
            ["src/Foo/Bar.php", "src/Baz.php", "src/Qux.php"]
        );
    }

    #[test]
    fn a_class_finds_tests_that_cover_it_and_pest_files_named_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Acceptance/CheckoutTest.php",
            &test_class(
                "Tests\\Acceptance",
                "CheckoutTest",
                "use App\\Foo\\Bar;\n#[CoversClass(Bar::class)]",
            ),
        );
        write(
            project,
            "tests/Acceptance/UnrelatedTest.php",
            &test_class(
                "Tests\\Acceptance",
                "UnrelatedTest",
                "#[CoversClass(Bar::class)]",
            ),
        );
        write(
            project,
            "tests/Feature/Shop/BarTest.php",
            "<?php\nit('works', fn () => 1);\n",
        );
        write(project, "vendor/x/tests/BarTest.php", "<?php\n");

        let navigation = navigate_from(project, "src/Foo/Bar.php");

        // `UnrelatedTest` covers `Tests\Acceptance\Bar`, not `App\Foo\Bar`.
        assert_eq!(
            navigation.files,
            [
                "tests/Acceptance/CheckoutTest.php",
                "tests/Feature/Shop/BarTest.php"
            ]
        );
    }

    #[test]
    fn a_pest_file_maps_by_file_name_and_its_covers_call() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "src/Baz.php",
            "<?php\nnamespace App;\nclass Baz {}\n",
        );
        write(
            project,
            "tests/Unit/Foo/BarTest.php",
            "<?php\nuse App\\Baz;\n\ncovers(Baz::class);\n\nit('works', fn () => 1);\n",
        );

        let navigation = navigate_from(project, "tests/Unit/Foo/BarTest.php");

        assert!(navigation.from_test);
        assert_eq!(navigation.name, "BarTest");
        assert_eq!(navigation.files, ["src/Baz.php", "src/Foo/Bar.php"]);
    }

    #[test]
    fn without_test_roots_the_tests_directory_is_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/BarTest.php",
            "<?php\ntest('x', fn () => 1);\n",
        );
        let text = std::fs::read_to_string(project.join("src/Foo/Bar.php")).unwrap();

        let roots = [root("App\\", "src/", false)];
        let navigation = navigate_with(&roots, project, &project.join("src/Foo/Bar.php"), &text);

        assert_eq!(navigation.files, ["tests/BarTest.php"]);
    }

    #[test]
    fn nothing_found_is_an_empty_list_and_never_the_file_itself() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        let navigation = navigate_from(project, "src/Foo/Bar.php");
        assert!(navigation.files.is_empty());
        assert_eq!(navigation.name, "Bar");
    }

    #[test]
    fn navigate_reads_the_roots_from_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(
            project,
            "composer.json",
            r#"{"autoload":{"psr-4":{"App\\":"src/"}},"autoload-dev":{"psr-4":{"Tests\\":"tests/"}}}"#,
        );
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Foo/BarTest.php",
            &test_class("Tests\\Foo", "BarTest", ""),
        );

        let navigation = navigate(project, &project.join("src/Foo/Bar.php"), SUBJECT);

        assert_eq!(navigation.files, ["tests/Foo/BarTest.php"]);
    }

    #[test]
    fn test_files_are_recognised_by_name_or_by_tests() {
        assert!(is_test_file(Path::new("tests/EmptyTest.php"), "<?php\n"));
        assert!(is_test_file(
            Path::new("tests/checkout.php"),
            "<?php\nit('works', fn () => 1);\n"
        ));
        assert!(!is_test_file(Path::new("src/Bar.php"), SUBJECT));
    }

    #[test]
    fn imports_cover_aliases_groups_and_skip_functions() {
        assert_eq!(
            imports_of("use App\\Foo\\Bar;"),
            [("Bar".to_string(), "App\\Foo\\Bar".to_string())]
        );
        assert_eq!(
            imports_of("use \\App\\Foo\\Bar as Baz;"),
            [("Baz".to_string(), "App\\Foo\\Bar".to_string())]
        );
        assert_eq!(
            imports_of("use App\\{A, B\\C as D};"),
            [
                ("A".to_string(), "App\\A".to_string()),
                ("D".to_string(), "App\\B\\C".to_string())
            ]
        );
        assert!(imports_of("use function App\\helper;").is_empty());
    }

    #[test]
    fn covered_names_skip_other_covers_tags_and_unrelated_calls() {
        let text = "/** @coversNothing\n * @coversDefaultClass X\n * @covers ::m\n */\n\
                    $x->recovers(Foo::class);\ncovers(Bar::class, 'Baz');";
        let names: Vec<String> = covered_names(text).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Bar"]);
    }

    #[test]
    fn a_covers_annotation_names_a_fully_qualified_class_without_its_backslash() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Acceptance/CheckoutTest.php",
            &test_class(
                "Tests\\Acceptance",
                "CheckoutTest",
                "/** @covers App\\Foo\\Bar */",
            ),
        );

        assert_eq!(
            navigate_from(project, "tests/Acceptance/CheckoutTest.php").files,
            ["src/Foo/Bar.php"]
        );
        assert_eq!(
            navigate_from(project, "src/Foo/Bar.php").files,
            ["tests/Acceptance/CheckoutTest.php"]
        );
    }

    #[test]
    fn a_laravel_feature_test_finds_its_controller_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let roots = [
            root("App\\", "app/", false),
            root("Tests\\", "tests/", true),
        ];
        write(
            project,
            "app/Http/Controllers/UserController.php",
            "<?php\nnamespace App\\Http\\Controllers;\nclass UserController {}\n",
        );
        let test = test_class("Tests\\Feature", "UserControllerTest", "");
        write(project, "tests/Feature/UserControllerTest.php", &test);

        let file = project.join("tests/Feature/UserControllerTest.php");
        let navigation = navigate_with(&roots, project, &file, &test);

        assert_eq!(
            navigation.files,
            ["app/Http/Controllers/UserController.php"]
        );
    }

    #[test]
    fn a_symfony_layout_maps_app_tests_onto_the_tests_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let roots = [
            root("App\\", "src/", false),
            root("App\\Tests\\", "tests/", true),
        ];
        write(project, "src/Foo/Bar.php", SUBJECT);
        let test = test_class("App\\Tests\\Foo", "BarTest", "");
        write(project, "tests/Foo/BarTest.php", &test);

        let subject = project.join("src/Foo/Bar.php");
        let to_test = navigate_with(&roots, project, &subject, SUBJECT);
        assert_eq!(to_test.files, ["tests/Foo/BarTest.php"]);
        let back = navigate_with(
            &roots,
            project,
            &project.join("tests/Foo/BarTest.php"),
            &test,
        );
        assert_eq!(back.files, ["src/Foo/Bar.php"]);
    }

    #[test]
    fn a_prefix_with_several_directories_maps_from_each() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let roots = [
            root("App\\", "src/", false),
            root("App\\", "lib/", false),
            root("Tests\\", "tests/", true),
        ];
        write(project, "lib/Foo/Bar.php", SUBJECT);
        let test = test_class("Tests\\Foo", "BarTest", "");
        write(project, "tests/Foo/BarTest.php", &test);

        let subject = project.join("lib/Foo/Bar.php");
        assert_eq!(
            navigate_with(&roots, project, &subject, SUBJECT).files,
            ["tests/Foo/BarTest.php"]
        );
        let back = navigate_with(
            &roots,
            project,
            &project.join("tests/Foo/BarTest.php"),
            &test,
        );
        assert_eq!(back.files, ["lib/Foo/Bar.php"]);
    }

    #[test]
    fn the_longest_source_prefix_alone_is_mirrored() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let roots = [
            root("App\\", "src/", false),
            root("App\\Legacy\\", "src/old/", false),
            root("Tests\\", "tests/", true),
        ];
        let legacy = "<?php\nnamespace App\\Legacy;\nclass Foo {}\n";
        write(project, "src/old/Foo.php", legacy);
        write(
            project,
            "tests/FooTest.php",
            &test_class("Tests", "FooTest", ""),
        );
        // What `App\` alone would mirror `App\Legacy\Foo` to.
        write(
            project,
            "tests/Legacy/FooTest.php",
            &test_class("Tests\\Legacy", "FooTest", ""),
        );

        let navigation = navigate_with(&roots, project, &project.join("src/old/Foo.php"), legacy);

        assert_eq!(navigation.files, ["tests/FooTest.php"]);
    }

    #[test]
    fn a_convention_match_skips_the_scan_of_the_test_directories() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "src/Foo/Bar.php", SUBJECT);
        write(
            project,
            "tests/Foo/BarTest.php",
            &test_class("Tests\\Foo", "BarTest", ""),
        );
        write(
            project,
            "tests/Acceptance/CheckoutTest.php",
            &test_class(
                "Tests\\Acceptance",
                "CheckoutTest",
                "/** @covers \\App\\Foo\\Bar */",
            ),
        );

        assert_eq!(
            navigate_from(project, "src/Foo/Bar.php").files,
            ["tests/Foo/BarTest.php"]
        );
    }

    #[test]
    fn requests_run_one_at_a_time_and_only_the_latest_answer_counts() {
        let mut requests = NavigationRequests::default();
        let (first, _) = requests.request("a").unwrap();
        // A second and third press while the first search runs: only the
        // last waits.
        assert!(requests.request("b").is_none());
        assert!(requests.request("c").is_none());
        let (wanted, next) = requests.finish(first);
        assert!(!wanted, "a newer request supersedes the first answer");
        let (third, request) = next.unwrap();
        assert_eq!(request, "c");
        assert_eq!(requests.finish(third), (true, None));

        // Nothing running: a request starts at once.
        let (token, _) = requests.request("d").unwrap();
        requests.cancel();
        assert_eq!(requests.finish(token), (false, None));
        assert!(requests.request("e").is_some());
    }
}
