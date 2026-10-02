# 0071. Twig and Blade grammars, and editing that follows injected languages

## Status

Accepted.
Delivered by phase Y of the [PHP parity plan](../php-parity-plan.md).
Amends the language list of [ADR-0018](0018-single-source-language-detection.md): the registry gains three rows and one lookup rule, and stays the only place that maps a file to a language.

## Context

PHP projects are mostly templates.
Twig (Symfony) and Blade (Laravel) files had no grammar, `.phtml` and `.inc` were plain text, and Ctrl+/ in the HTML of a `.php` file inserted `//` because comment syntax came from the file's one language.
Neither tree-sitter-twig nor tree-sitter-blade is published on crates.io, so they cannot be added to `syntax-core/Cargo.toml` like the other grammars.
The Blade grammar parses the surrounding HTML itself and exposes PHP as `php_only` nodes; the Twig grammar is flat, so `{% if %}` and `{% endif %}` are sibling nodes.

## Decision

- **Vendor the generated C sources** under `crates/syntax-core/grammars/{twig,blade}/` with their MIT licences, and compile them in `syntax-core/build.rs` with the `cc` crate.
  `vendored.rs` wraps each `tree_sitter_<name>` symbol in a `tree_sitter_language::LanguageFn`.
  Pinned upstream commits:
  - Twig: `gbprod/tree-sitter-twig` at `2208d2a3c3ee7ef378e97df2e51c18feb7ee9dfc`.
  - Blade: `EmranMR/tree-sitter-blade` at `b5291d1ba207a8ebb8383b2ecb8a8a6535210a50`.
  Both were generated for parser ABI 15, which the workspace's tree-sitter 0.26 accepts, so nothing was regenerated.
  Upgrading means replacing the files and re-running the catalog tests.
- **Portable C only.**
  `build.rs` passes `-std=c11` through `cc` and no other flag, so the MinGW (MXE) cross build uses the same files and its own `CC`.
- **Catalog rows:** `twig` (`twig`), `blade` (`blade.php`), and `php_only`, a row with no extension that exists to be injected by Blade (`tree_sitter_php::LANGUAGE_PHP_ONLY`).
  `.phtml` and `.inc` join the `php` row.
- **Compound extensions.**
  `language_for_path` tries the file name's tail against extensions that contain a dot (`blade.php`) before the plain extension, so `home.blade.php` is Blade and `blade.php` alone stays PHP.
- **Injections.**
  Blade injects `php_only` into directives, echoes and `@php` blocks, and JavaScript into Livewire and Alpine attributes; its own HTML needs no injection.
  Twig injects HTML into its `content` nodes as one document (`injection.combined`, now supported by `syntax-core`).
  The upstream `@envoy` Bash pattern is dropped because its `#has-ancestor?` predicate cannot be evaluated.
- **Folding.**
  Blade folds its nested directive, element and script nodes by query.
  Twig's `{% … %}` pairs are folded by `syntax_core::folds`, which matches an opener to its `end…` partner by tag name on a stack, because no query can capture the block between flat siblings.
- **Injection-aware editing.**
  `syntax_core::language_at(host, text, offset)` and `LanguageMap` (one parse, many offsets) name the innermost injected language under an offset.
  `edit_ops::comment::toggle_line` comments each line in the language it is written in: `<!-- -->` for the markup of a `.php` file, `//` for its PHP, JavaScript's `//` in a `<script>`, `{# #}` on Twig tags.
  A language that comments exactly like the file itself counts as the file's own, so a Markdown `<!-- -->` that the grammar re-parses as injected HTML still toggles back.
- **Richer PHP folds:** enum, `switch` and `match` bodies, arrays, `use A\{B, C}` groups, heredocs, attribute lists, and multi-line parameter and argument lists; doc comments fold through the generic comment rule.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Ship Twig and Blade as runtime grammars (`syntax_core::runtime`) | They would need a compiled library per platform delivered outside the binary, and a first-run download for a built-in language. |
| Wait for a crates.io release | There is none, and the issue is ongoing for every Symfony and Laravel user. |
| Depend on the upstream repositories as git dependencies | Builds would need the network, and a force-pushed branch would change the grammar under us. |
| Regenerate the parsers from `grammar.js` in `build.rs` | It needs Node and `tree-sitter-cli` at build time, in the Linux and MXE images alike. |
| A Twig-specific fold query | Not expressible: the tree has no node for the block. |
| Comment syntax per file, with a separate "template language" setting | The language under the caret is already known from the injection queries, and a setting would be wrong in every mixed file. |

## Consequences

- `syntax-core` gains a build script, a `cc` build-dependency (already in `Cargo.lock` through tree-sitter) and about 24 MB of generated C source, almost all of it Blade's parse tables.
  It compiles in a few seconds and adds a few MB to the binary.
- A C compiler is already required to build tree-sitter and the Qt side, so no new tool is needed, including in the MXE image.
- The Windows cross build has not been run for this change; the build uses plain C11 and the standard `cc` environment variables.
- Twig has no query-level block structure, so selection expansion and structure-aware features see a flat file.
  Only folding pairs the directives.
- A Blade or Twig file larger than the highlight ceiling is not parsed, so it gets neither colour nor injection-aware comments.
- Commenting a `<?php` or `?>` line with `//` still produces text the PHP grammar reads differently; the toggle does not special-case tag lines.

## Related

- [ADR-0018](0018-single-source-language-detection.md) — one source of language detection.
- [ADR-0066](0066-several-language-servers-per-language.md) — the PHP servers that serve these files.
- [PHP parity plan](../php-parity-plan.md), phase Y.
