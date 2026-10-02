//! Rendering a new-file template (ADR-0072).

use plugin_api::FileTemplateContribution;

/// What `${…}` stands for in a file template body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vars {
    /// The class or file name, without the extension.
    pub name: String,
    /// Empty when the file sits under no namespace root.
    pub namespace: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub year: String,
}

const NAMESPACE_VAR: &str = "${NAMESPACE}";

/// The name as the template wants it: `name_suffix` appended unless the
/// name already ends with it, and a typed-in extension removed.
pub fn entity_name(template: &FileTemplateContribution, typed: &str) -> String {
    let dotted = format!(".{}", template.extension);
    let name = typed.trim();
    let name = name.strip_suffix(&dotted).unwrap_or(name);
    match &template.name_suffix {
        Some(suffix) if !name.ends_with(suffix.as_str()) => format!("{name}{suffix}"),
        _ => name.to_string(),
    }
}

/// The file name for `name` (already through [`entity_name`]).
pub fn file_name(template: &FileTemplateContribution, name: &str) -> String {
    format!("{name}.{}", template.extension)
}

/// The file's content: the body with its variables filled in. With an empty
/// namespace, the lines that mention `${NAMESPACE}` go, and the blank line
/// right after them.
pub fn render(template: &FileTemplateContribution, vars: &Vars) -> String {
    let mut lines: Vec<&str> = template.body.split('\n').collect();
    if vars.namespace.is_empty() {
        let mut kept = Vec::with_capacity(lines.len());
        let mut dropped_previous = false;
        for line in lines {
            if line.contains(NAMESPACE_VAR) {
                dropped_previous = true;
                continue;
            }
            if dropped_previous && line.trim().is_empty() {
                dropped_previous = false;
                continue;
            }
            dropped_previous = false;
            kept.push(line);
        }
        lines = kept;
    }
    lines
        .join("\n")
        .replace("${NAME}", &vars.name)
        .replace(NAMESPACE_VAR, &vars.namespace)
        .replace("${DATE}", &vars.date)
        .replace("${YEAR}", &vars.year)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(body: &str, suffix: Option<&str>) -> FileTemplateContribution {
        FileTemplateContribution {
            id: "t".into(),
            name: "T".into(),
            language: "php".into(),
            extension: "php".into(),
            name_suffix: suffix.map(str::to_string),
            body: body.into(),
        }
    }

    fn vars(namespace: &str) -> Vars {
        Vars {
            name: "Foo".into(),
            namespace: namespace.into(),
            date: "2026-10-02".into(),
            year: "2026".into(),
        }
    }

    const BODY: &str = "<?php\n\nnamespace ${NAMESPACE};\n\nclass ${NAME} {}\n// ${DATE} ${YEAR}\n";

    #[test]
    fn variables_are_filled_in() {
        assert_eq!(
            render(&template(BODY, None), &vars("App\\Models")),
            "<?php\n\nnamespace App\\Models;\n\nclass Foo {}\n// 2026-10-02 2026\n"
        );
    }

    #[test]
    fn an_empty_namespace_drops_its_line_and_the_blank_after_it() {
        assert_eq!(
            render(&template(BODY, None), &vars("")),
            "<?php\n\nclass Foo {}\n// 2026-10-02 2026\n"
        );
    }

    #[test]
    fn the_name_gets_its_suffix_once_and_loses_a_typed_extension() {
        let t = template("", Some("Test"));
        assert_eq!(entity_name(&t, "Foo"), "FooTest");
        assert_eq!(entity_name(&t, " FooTest.php "), "FooTest");
        assert_eq!(file_name(&t, "FooTest"), "FooTest.php");
        assert_eq!(entity_name(&template("", None), "Foo"), "Foo");
    }

    #[test]
    fn the_shipped_php_templates_render_with_and_without_a_namespace() {
        let php_tools: Vec<_> = plugin_host::BUILTIN_PLUGINS
            .iter()
            .copied()
            .filter(|b| b.manifest.contains("id = \"php-tools\""))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let registry = plugin_host::load(dir.path(), &php_tools, &[]);
        assert!(registry.errors().is_empty(), "{:?}", registry.errors());
        let by_id = |id: &str| {
            registry
                .file_templates()
                .map(|(_, t)| t)
                .find(|t| t.id == id)
                .unwrap()
                .clone()
        };

        let test = by_id("php-test");
        let name = entity_name(&test, "Foo");
        let v = Vars {
            name: name.clone(),
            ..vars("Tests\\Unit")
        };
        let out = render(&test, &v);
        assert!(out.contains("namespace Tests\\Unit;"), "{out}");
        assert!(
            out.contains("final class FooTest extends TestCase"),
            "{out}"
        );
        let bare = render(&test, &Vars { name, ..vars("") });
        assert!(!bare.contains("namespace"), "{bare}");
        assert!(
            bare.starts_with("<?php\n\ndeclare(strict_types=1);\n\nuse PHPUnit"),
            "{bare}"
        );

        for (id, keyword) in [
            ("php-class", "class Foo"),
            ("php-interface", "interface Foo"),
            ("php-trait", "trait Foo"),
            ("php-enum", "enum Foo"),
        ] {
            let out = render(&by_id(id), &vars("App"));
            assert!(
                out.contains("namespace App;") && out.contains(keyword),
                "{id}: {out}"
            );
        }
        assert_eq!(
            render(&by_id("php-file"), &vars("App")),
            "<?php\n\ndeclare(strict_types=1);\n"
        );
    }
}
