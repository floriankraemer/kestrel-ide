//! Grammars that are not on crates.io, vendored as generated C under
//! `grammars/` and compiled by `build.rs` (ADR-0071).

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_twig() -> *const ();
    fn tree_sitter_blade() -> *const ();
}

/// The Twig grammar (gbprod/tree-sitter-twig).
pub const TWIG: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_twig) };

/// The Blade grammar (EmranMR/tree-sitter-blade). It parses the HTML around
/// the directives itself and exposes PHP as `php_only` nodes.
pub const BLADE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_blade) };

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(language: LanguageFn, text: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language.into())
            .expect("the vendored grammar's ABI is accepted");
        parser.parse(text, None).expect("parses")
    }

    #[test]
    fn the_vendored_twig_grammar_parses_a_template() {
        let tree = parse(TWIG, "<p>{{ name|upper }}</p>{% if ok %}x{% endif %}\n");
        assert!(!tree.root_node().has_error(), "{}", tree.root_node());
    }

    #[test]
    fn the_vendored_blade_grammar_parses_a_template() {
        let tree = parse(BLADE, "@if ($ok)\n<p>{{ $name }}</p>\n@endif\n");
        assert!(!tree.root_node().has_error(), "{}", tree.root_node());
    }
}
