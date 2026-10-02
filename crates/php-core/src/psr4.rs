//! The PSR-4 namespace of a directory (ADR-0072), for the New > PHP Class
//! templates.

use std::path::{Component, Path};

use crate::composer::{ComposerJson, Psr4Root};

/// The root-relative components of `path`, `.` removed. `None` when `path`
/// is not under `base`.
fn relative_components(base: &Path, path: &Path) -> Option<Vec<String>> {
    let rest = path.strip_prefix(base).ok()?;
    Some(components(rest))
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// The namespace for a file created in `dir` of the project at `project`:
/// the PSR-4 root whose directory is the longest prefix of `dir` (across
/// `autoload` and `autoload-dev`, several roots per prefix included), its
/// namespace prefix, then the sub-directories below it. No root matches ->
/// the empty string, which templates answer by omitting the `namespace`
/// line.
pub fn namespace_for(roots: &[Psr4Root], project: &Path, dir: &Path) -> String {
    let Some(target) = relative_components(project, dir) else {
        return String::new();
    };
    let best = roots
        .iter()
        .filter_map(|root| {
            let root_dir = components(Path::new(&root.dir));
            target
                .starts_with(&root_dir)
                .then_some((root_dir.len(), root))
        })
        .max_by_key(|(depth, _)| *depth);
    let Some((depth, root)) = best else {
        return String::new();
    };
    let mut parts: Vec<&str> = root
        .prefix
        .split('\\')
        .filter(|segment| !segment.is_empty())
        .collect();
    parts.extend(target[depth..].iter().map(String::as_str));
    parts.join("\\")
}

/// [`namespace_for`] with the roots read from `<project>/composer.json`; no
/// file, or one that does not parse, means no namespace.
pub fn namespace_for_dir(project: &Path, dir: &Path) -> String {
    match ComposerJson::read(project) {
        Ok(Some(composer)) => namespace_for(&composer.psr4, project, dir),
        _ => String::new(),
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
            root("App\\Legacy\\", "src/old/", false),
            root("Lib\\", "lib/", false),
            root("Lib\\", "vendor-lib/", false),
            root("Tests\\", "tests/", true),
        ]
    }

    fn ns(dir: &str) -> String {
        namespace_for(&roots(), Path::new("/p"), &Path::new("/p").join(dir))
    }

    #[test]
    fn the_root_directory_is_the_prefix_and_subdirectories_extend_it() {
        assert_eq!(ns("src"), "App");
        assert_eq!(ns("src/Http/Controllers"), "App\\Http\\Controllers");
    }

    #[test]
    fn the_longest_matching_root_wins() {
        assert_eq!(ns("src/old/Models"), "App\\Legacy\\Models");
    }

    #[test]
    fn every_root_of_a_prefix_and_autoload_dev_count() {
        assert_eq!(ns("vendor-lib/Util"), "Lib\\Util");
        assert_eq!(ns("lib"), "Lib");
        assert_eq!(ns("tests/Unit"), "Tests\\Unit");
    }

    #[test]
    fn a_directory_outside_every_root_has_no_namespace() {
        assert_eq!(ns("scripts"), "");
        assert_eq!(ns("srcextra"), "");
        assert_eq!(
            namespace_for(&roots(), Path::new("/p"), Path::new("/elsewhere/src")),
            ""
        );
    }

    #[test]
    fn a_root_without_a_trailing_slash_or_a_dot_prefix_still_matches() {
        let roots = [root("App\\", "./src", false)];
        assert_eq!(
            namespace_for(&roots, Path::new("/p"), Path::new("/p/src/X")),
            "App\\X"
        );
    }

    #[test]
    fn the_empty_prefix_maps_a_directory_onto_the_namespace_root() {
        let roots = [root("", "src/", false)];
        assert_eq!(
            namespace_for(&roots, Path::new("/p"), Path::new("/p/src/Foo")),
            "Foo"
        );
        assert_eq!(
            namespace_for(&roots, Path::new("/p"), Path::new("/p/src")),
            ""
        );
    }

    #[test]
    fn composer_json_is_read_from_the_project() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(namespace_for_dir(dir.path(), &dir.path().join("src")), "");
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
        )
        .unwrap();
        assert_eq!(
            namespace_for_dir(dir.path(), &dir.path().join("src/Models")),
            "App\\Models"
        );
    }
}
