//! Code generation for a PHP class (ADR-0072): constructor, getters and
//! setters from its properties, worked out over the tree-sitter parse so it
//! needs no language server (Phpactor does not run on Windows).
//!
//! Everything here is a pure function of the text: [`class_at`] reads the
//! class around an offset, [`Class::members`] lists what a kind of
//! generation could take, and [`Class::generate`] produces the insertion.

use std::collections::HashSet;

use tree_sitter::Node;

/// What to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Constructor,
    Getters,
    Setters,
    GettersAndSetters,
}

/// One property the user may pick: `name` is what [`Class::generate`] takes
/// back, `label` what the picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub label: String,
}

/// Text to insert at `offset`; nothing is replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    pub offset: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Property {
    /// Without the `$`.
    name: String,
    type_text: Option<String>,
    is_static: bool,
    readonly: bool,
    has_default: bool,
    promoted: bool,
}

impl Property {
    fn is_bool(&self) -> bool {
        self.type_text
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("bool"))
    }

    fn getter_name(&self) -> String {
        let camel = camel_case(&self.name);
        if self.is_bool() && !has_is_prefix(&camel) {
            format!("is{}", upper_first(&camel))
        } else if self.is_bool() {
            camel
        } else {
            format!("get{}", upper_first(&camel))
        }
    }

    fn setter_name(&self) -> String {
        let camel = camel_case(&self.name);
        let base = if self.is_bool() && has_is_prefix(&camel) {
            &camel[2..]
        } else {
            &camel
        };
        format!("set{}", upper_first(base))
    }

    fn label(&self) -> String {
        match &self.type_text {
            Some(t) => format!("${}: {t}", self.name),
            None => format!("${}", self.name),
        }
    }

    /// `?string $name` or `$name`.
    fn parameter(&self) -> String {
        let var = camel_case(&self.name);
        match &self.type_text {
            Some(t) => format!("{t} ${var}"),
            None => format!("${var}"),
        }
    }
}

/// A class or trait, reduced to what generation needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub name: String,
    properties: Vec<Property>,
    /// Lower-cased.
    methods: HashSet<String>,
    has_constructor: bool,
    /// Indentation of the class declaration; members sit one unit deeper.
    base_indent: String,
    /// Where the closing brace's line starts, or the brace itself when
    /// something precedes it on its line.
    close: Close,
    /// The body has a member before the closing brace.
    has_members: bool,
    /// End of the last property declaration, and whether a blank line must
    /// follow the constructor placed there.
    after_properties: Option<(usize, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Close {
    LineStart(usize),
    /// `class A { }`: no line of its own.
    Inline(usize),
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn camel_case(name: &str) -> String {
    let mut parts = name.split('_').filter(|p| !p.is_empty());
    let first = parts.next().unwrap_or("").to_string();
    parts.fold(first, |acc, part| acc + &upper_first(part))
}

fn has_is_prefix(camel: &str) -> bool {
    camel
        .strip_prefix("is")
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_uppercase)
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    let language = syntax_core::language_by_id("php")?;
    let compiled = syntax_core::registry().compiled(language)?.ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&compiled.grammar).ok()?;
    parser.parse(text, None)
}

/// The class or trait that contains `offset`, innermost first.
pub fn class_at(text: &str, offset: usize) -> Option<Class> {
    let tree = parse(text)?;
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(offset.min(text.len()), offset.min(text.len()));
    while let Some(current) = node {
        if matches!(current.kind(), "class_declaration" | "trait_declaration") {
            return read_class(text, current);
        }
        node = current.parent();
    }
    None
}

fn read_class(text: &str, node: Node<'_>) -> Option<Class> {
    let name = text[node.child_by_field_name("name")?.byte_range()].to_string();
    let body = node.child_by_field_name("body")?;
    let class_readonly = children(node).any(|c| c.kind() == "readonly_modifier");
    let mut properties = Vec::new();
    let mut methods = HashSet::new();
    let mut has_constructor = false;
    let mut has_members = false;
    let mut after_properties = None;

    for member in children(body) {
        match member.kind() {
            "{" | "}" | "comment" => continue,
            _ => has_members = true,
        }
        match member.kind() {
            "property_declaration" => {
                read_property_declaration(text, member, class_readonly, &mut properties);
                after_properties = Some((member.end_byte(), blank_needed_after(text, member)));
            }
            "method_declaration" => {
                let method = member
                    .child_by_field_name("name")
                    .map(|n| text[n.byte_range()].to_lowercase())
                    .unwrap_or_default();
                if method == "__construct" {
                    has_constructor = true;
                    read_promoted(text, member, class_readonly, &mut properties);
                }
                methods.insert(method);
            }
            _ => {}
        }
    }

    let close_byte = body.end_byte().saturating_sub(1);
    let line_start = text[..close_byte].rfind('\n').map_or(0, |i| i + 1);
    let close = if text[line_start..close_byte].trim().is_empty() {
        Close::LineStart(line_start)
    } else {
        Close::Inline(close_byte)
    };
    let decl_start = node.start_byte();
    let decl_line = text[..decl_start].rfind('\n').map_or(0, |i| i + 1);
    let base_indent: String = text[decl_line..decl_start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    Some(Class {
        name,
        properties,
        methods,
        has_constructor,
        base_indent,
        close,
        has_members,
        after_properties,
    })
}

fn children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (0..node.child_count()).filter_map(move |i| node.child(i as u32))
}

fn blank_needed_after(text: &str, member: Node<'_>) -> bool {
    let rest = &text[member.end_byte()..];
    let next_line = rest.split_once('\n').map_or("", |(_, after)| after);
    let next = next_line.lines().next().unwrap_or("").trim();
    !next.is_empty() && !next.starts_with('}')
}

fn read_property_declaration(
    text: &str,
    node: Node<'_>,
    class_readonly: bool,
    out: &mut Vec<Property>,
) {
    let type_text = node
        .child_by_field_name("type")
        .map(|t| text[t.byte_range()].to_string());
    let is_static = children(node).any(|c| c.kind() == "static_modifier");
    let readonly = class_readonly || children(node).any(|c| c.kind() == "readonly_modifier");
    for element in children(node).filter(|c| c.kind() == "property_element") {
        let Some(variable) = element.child_by_field_name("name") else {
            continue;
        };
        out.push(Property {
            name: text[variable.byte_range()]
                .trim_start_matches('$')
                .to_string(),
            type_text: type_text.clone(),
            is_static,
            readonly,
            has_default: element.child_by_field_name("default_value").is_some(),
            promoted: false,
        });
    }
}

fn read_promoted(text: &str, method: Node<'_>, class_readonly: bool, out: &mut Vec<Property>) {
    let Some(parameters) = method.child_by_field_name("parameters") else {
        return;
    };
    for parameter in children(parameters).filter(|c| c.kind() == "property_promotion_parameter") {
        let Some(variable) = parameter.child_by_field_name("name") else {
            continue;
        };
        out.push(Property {
            name: text[variable.byte_range()]
                .trim_start_matches('$')
                .to_string(),
            type_text: parameter
                .child_by_field_name("type")
                .map(|t| text[t.byte_range()].to_string()),
            is_static: false,
            readonly: class_readonly || parameter.child_by_field_name("readonly").is_some(),
            has_default: true,
            promoted: true,
        });
    }
}

impl Class {
    fn has_method(&self, name: &str) -> bool {
        self.methods.contains(&name.to_lowercase())
    }

    fn needs_getter(&self, p: &Property) -> bool {
        !p.is_static && !self.has_method(&p.getter_name())
    }

    fn needs_setter(&self, p: &Property) -> bool {
        !p.is_static && !p.readonly && !self.has_method(&p.setter_name())
    }

    fn wanted(&self, kind: Kind, p: &Property) -> bool {
        match kind {
            Kind::Constructor => !p.is_static && !p.promoted && !p.has_default,
            Kind::Getters => self.needs_getter(p),
            Kind::Setters => self.needs_setter(p),
            Kind::GettersAndSetters => self.needs_getter(p) || self.needs_setter(p),
        }
    }

    /// The properties `kind` can be generated for, or why it cannot be.
    pub fn members(&self, kind: Kind) -> Result<Vec<Member>, String> {
        if kind == Kind::Constructor && self.has_constructor {
            return Err("The class already has a constructor.".to_string());
        }
        let members: Vec<Member> = self
            .properties
            .iter()
            .filter(|p| self.wanted(kind, p))
            .map(|p| Member {
                name: p.name.clone(),
                label: p.label(),
            })
            .collect();
        if members.is_empty() {
            return Err(match kind {
                Kind::Constructor => "Every property is already initialized.",
                Kind::Getters => "Every property already has a getter.",
                Kind::Setters => "Every property already has a setter, or is readonly.",
                Kind::GettersAndSetters => "Every property already has its accessors.",
            }
            .to_string());
        }
        Ok(members)
    }

    /// The code for the properties named in `selected`, in declaration
    /// order, indented with `unit`.
    pub fn generate(
        &self,
        kind: Kind,
        selected: &[String],
        unit: &str,
    ) -> Result<Insertion, String> {
        self.members(kind)?;
        let chosen: Vec<&Property> = self
            .properties
            .iter()
            .filter(|p| self.wanted(kind, p) && selected.contains(&p.name))
            .collect();
        if chosen.is_empty() {
            return Err("No property was selected.".to_string());
        }
        let indent = format!("{}{unit}", self.base_indent);
        let method = |signature: String, body: String| {
            format!("{indent}public function {signature}\n{indent}{{\n{indent}{unit}{body}\n{indent}}}\n")
        };
        if kind == Kind::Constructor {
            let parameters: Vec<String> = chosen.iter().map(|p| p.parameter()).collect();
            let body: Vec<String> = chosen
                .iter()
                .map(|p| format!("$this->{} = ${};", p.name, camel_case(&p.name)))
                .collect();
            let separator = format!("\n{indent}{unit}");
            let ctor = method(
                format!("__construct({})", parameters.join(", ")),
                body.join(&separator),
            );
            return Ok(self.place_constructor(ctor));
        }
        let mut methods = Vec::new();
        for p in chosen {
            let with_getter =
                matches!(kind, Kind::Getters | Kind::GettersAndSetters) && self.needs_getter(p);
            let with_setter =
                matches!(kind, Kind::Setters | Kind::GettersAndSetters) && self.needs_setter(p);
            if with_getter {
                let return_type = p
                    .type_text
                    .as_ref()
                    .map_or(String::new(), |t| format!(": {t}"));
                methods.push(method(
                    format!("{}(){return_type}", p.getter_name()),
                    format!("return $this->{};", p.name),
                ));
            }
            if with_setter {
                methods.push(method(
                    format!("{}({}): void", p.setter_name(), p.parameter()),
                    format!("$this->{} = ${};", p.name, camel_case(&p.name)),
                ));
            }
        }
        Ok(self.place_at_end(&methods.join("\n")))
    }

    fn place_at_end(&self, methods: &str) -> Insertion {
        match self.close {
            Close::LineStart(offset) => Insertion {
                offset,
                text: if self.has_members {
                    format!("\n{methods}")
                } else {
                    methods.to_string()
                },
            },
            Close::Inline(offset) => Insertion {
                offset,
                text: format!("\n{methods}{}", self.base_indent),
            },
        }
    }

    fn place_constructor(&self, ctor: String) -> Insertion {
        match self.after_properties {
            Some((offset, blank_after)) => Insertion {
                offset,
                text: format!(
                    "\n\n{}{}",
                    ctor.trim_end_matches('\n'),
                    if blank_after { "\n" } else { "" }
                ),
            },
            None => self.place_at_end(&ctor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = "    ";

    const SAMPLE: &str = "<?php\nclass User\n{\n    private ?string $name = null;\n    protected static int $count;\n    public readonly array $items;\n    private bool $active;\n    private int $user_id;\n\n    public function getName(): ?string\n    {\n        return $this->name;\n    }\n}\n";

    fn class(text: &str) -> Class {
        class_at(text, text.find("class").unwrap()).expect("a class")
    }

    fn apply(text: &str, i: &Insertion) -> String {
        let mut out = text.to_string();
        out.insert_str(i.offset, &i.text);
        out
    }

    fn names(members: Vec<Member>) -> Vec<String> {
        members.into_iter().map(|m| m.name).collect()
    }

    #[test]
    fn the_class_around_the_caret_is_found_and_read() {
        let c = class(SAMPLE);
        assert_eq!(c.name, "User");
        let labels: Vec<_> = c.properties.iter().map(Property::label).collect();
        assert_eq!(
            labels,
            [
                "$name: ?string",
                "$count: int",
                "$items: array",
                "$active: bool",
                "$user_id: int"
            ]
        );
        assert!(class_at(SAMPLE, 0).is_none());
        assert!(class_at("<?php\nfunction f() {}\n", 12).is_none());
    }

    #[test]
    fn a_constructor_takes_the_uninitialized_instance_properties() {
        let c = class(SAMPLE);
        assert_eq!(
            names(c.members(Kind::Constructor).unwrap()),
            ["items", "active", "user_id"]
        );
        let all: Vec<String> = ["items", "active", "user_id"].map(String::from).to_vec();
        let out = apply(SAMPLE, &c.generate(Kind::Constructor, &all, UNIT).unwrap());
        assert!(
            out.contains(
                "    private int $user_id;\n\n    public function __construct(array $items, bool $active, int $userId)\n    {\n        $this->items = $items;\n        $this->active = $active;\n        $this->user_id = $userId;\n    }\n\n    public function getName()"
            ),
            "{out}"
        );
    }

    #[test]
    fn only_the_selected_properties_are_used() {
        let c = class(SAMPLE);
        let out = apply(
            SAMPLE,
            &c.generate(Kind::Constructor, &["active".to_string()], UNIT)
                .unwrap(),
        );
        assert!(out.contains("__construct(bool $active)"), "{out}");
        assert!(!out.contains("$items = "), "{out}");
        assert!(c.generate(Kind::Constructor, &[], UNIT).is_err());
    }

    #[test]
    fn a_class_with_a_constructor_cannot_get_a_second_one() {
        let text = "<?php\nclass A\n{\n    private int $x;\n\n    public function __construct(private string $s)\n    {\n    }\n}\n";
        let c = class(text);
        assert!(c
            .members(Kind::Constructor)
            .unwrap_err()
            .contains("constructor"));
        // The promoted property is a property all the same.
        assert_eq!(names(c.members(Kind::Getters).unwrap()), ["x", "s"]);
    }

    #[test]
    fn getters_skip_existing_methods_statics_and_respect_types() {
        let c = class(SAMPLE);
        assert_eq!(
            names(c.members(Kind::Getters).unwrap()),
            ["items", "active", "user_id"]
        );
        let all: Vec<String> = ["items", "active", "user_id"].map(String::from).to_vec();
        let out = apply(SAMPLE, &c.generate(Kind::Getters, &all, UNIT).unwrap());
        assert!(out.contains("    public function getItems(): array\n    {\n        return $this->items;\n    }\n"), "{out}");
        assert!(out.contains("public function isActive(): bool"), "{out}");
        assert!(out.contains("public function getUserId(): int"), "{out}");
        assert_eq!(out.matches("function getName").count(), 1);
        assert!(out.ends_with("    }\n}\n"), "{out}");
    }

    #[test]
    fn setters_skip_readonly_and_keep_nullable_types() {
        let c = class(SAMPLE);
        assert_eq!(
            names(c.members(Kind::Setters).unwrap()),
            ["name", "active", "user_id"]
        );
        let out = apply(
            SAMPLE,
            &c.generate(
                Kind::Setters,
                &["name".to_string(), "active".to_string()],
                UNIT,
            )
            .unwrap(),
        );
        assert!(out.contains("public function setName(?string $name): void\n    {\n        $this->name = $name;\n    }"), "{out}");
        // `isActive`-style bool properties lose the `is` in their setter.
        assert!(
            out.contains("public function setActive(bool $active): void"),
            "{out}"
        );
    }

    #[test]
    fn getters_and_setters_come_in_pairs_without_duplicating_what_exists() {
        let c = class(SAMPLE);
        let out = apply(
            SAMPLE,
            &c.generate(Kind::GettersAndSetters, &["name".to_string()], UNIT)
                .unwrap(),
        );
        // `getName` exists, so only the setter is added.
        assert_eq!(out.matches("function getName").count(), 1);
        assert!(
            out.contains("function setName(?string $name): void"),
            "{out}"
        );
        let both = apply(
            SAMPLE,
            &c.generate(Kind::GettersAndSetters, &["user_id".to_string()], UNIT)
                .unwrap(),
        );
        let get = both.find("function getUserId").unwrap();
        let set = both.find("function setUserId").unwrap();
        assert!(get < set);
    }

    #[test]
    fn an_is_prefixed_bool_keeps_its_name_as_the_getter() {
        let text = "<?php\nclass A\n{\n    private bool $isActive;\n}\n";
        let out = apply(
            text,
            &class(text)
                .generate(Kind::GettersAndSetters, &["isActive".to_string()], UNIT)
                .unwrap(),
        );
        assert!(out.contains("function isActive(): bool"), "{out}");
        assert!(
            out.contains("function setActive(bool $isActive): void"),
            "{out}"
        );
    }

    #[test]
    fn untyped_properties_get_untyped_accessors() {
        let text = "<?php\nclass A\n{\n    private $v;\n}\n";
        let out = apply(
            text,
            &class(text)
                .generate(Kind::GettersAndSetters, &["v".to_string()], UNIT)
                .unwrap(),
        );
        assert!(out.contains("public function getV()\n"), "{out}");
        assert!(out.contains("public function setV($v): void"), "{out}");
    }

    #[test]
    fn nested_and_inline_classes_indent_and_close_correctly() {
        let text = "<?php\nnamespace N {\n    class A { private int $x; }\n}\n";
        let c = class(text);
        let out = apply(
            text,
            &c.generate(Kind::Getters, &["x".to_string()], "  ").unwrap(),
        );
        assert!(out.contains("function getX(): int"), "{out}");
        assert!(out.contains("    class A { private int $x; \n"), "{out}");
        let tree_ok = class_at(&out, out.find("class").unwrap()).is_some();
        assert!(tree_ok);
    }

    #[test]
    fn an_empty_class_body_gets_no_leading_blank_line() {
        let text = "<?php\nclass A\n{\n    private int $x;\n}\n";
        let out = apply(
            text,
            &class(text)
                .generate(Kind::Getters, &["x".to_string()], UNIT)
                .unwrap(),
        );
        assert_eq!(
            out,
            "<?php\nclass A\n{\n    private int $x;\n\n    public function getX(): int\n    {\n        return $this->x;\n    }\n}\n"
        );
    }

    #[test]
    fn a_class_without_candidates_says_why() {
        let text = "<?php\nclass A\n{\n}\n";
        let c = class(text);
        for kind in [
            Kind::Constructor,
            Kind::Getters,
            Kind::Setters,
            Kind::GettersAndSetters,
        ] {
            assert!(c.members(kind).is_err());
        }
    }

    #[test]
    fn traits_are_classes_too() {
        let text = "<?php\ntrait T\n{\n    private int $x;\n}\n";
        assert_eq!(class_at(text, 8).unwrap().name, "T");
    }
}
