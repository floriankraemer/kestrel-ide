//! PHP 8.1+ constructs the project index needs as symbols: enums, their
//! cases, and class constants, plus the `Foo::` sites that refer to them.
//!
//! An integration test because `lib.rs` is at its size baseline.

use std::path::Path;

use syntax_core::{identifier_occurrences, language_for_path, outline, Language, SymbolKind};

const SNIPPET: &str = "<?php\nenum Suit: string {\n    case Hearts = 'H';\n    const Wild = self::Hearts;\n}\n$s = Suit::Hearts;\nSuit::from('H');\n";

fn php() -> Language {
    language_for_path(Path::new("a.php"))
}

#[test]
fn an_enum_is_an_outline_root_holding_its_cases_and_constants() {
    let roots = outline(php(), SNIPPET);
    let suit = roots
        .iter()
        .find(|s| s.kind == SymbolKind::Enum && s.name == "Suit")
        .expect("expected a Suit enum root");
    let children: Vec<(&str, SymbolKind)> = suit
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.kind))
        .collect();
    assert_eq!(
        children,
        vec![
            ("Hearts", SymbolKind::EnumMember),
            ("Wild", SymbolKind::Constant)
        ]
    );
}

#[test]
fn enum_case_and_constant_names_are_definitions_and_scope_accesses_references() {
    let occurrences = identifier_occurrences(php(), SNIPPET);
    let sites = |name: &str| -> Vec<bool> {
        occurrences
            .iter()
            .filter(|o| o.name == name)
            .map(|o| o.is_definition)
            .collect()
    };
    assert_eq!(
        sites("Suit"),
        vec![true, false, false],
        "declaration, `Suit::Hearts`, `Suit::from`"
    );
    assert_eq!(
        sites("Hearts"),
        vec![true, false, false],
        "case, `self::Hearts`, `Suit::Hearts`"
    );
    assert_eq!(sites("Wild"), vec![true]);
}
