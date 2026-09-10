//! The table of languages compiled into the binary, and the shared
//! fragments its rows are built from.
//!
//! A sibling module of `registry.rs` rather than a child of it, since
//! `mod` inside a non-`mod.rs` file would look for `src/registry/`.
//!
//! Split out of `registry.rs` rather than living beside the machinery that
//! reads it: this is data — one row per language, ~40 of them — and the
//! file it came from had grown past the size gate's ceiling for reasons
//! that had nothing to do with the machinery. Every type a row is made of
//! (`LanguageDef`, `QuerySet`) still lives in `registry.rs`; only the
//! table and the constants it shares moved.
//!
//! Order is load-bearing — see [`BUILTIN_LANGUAGES`].

use crate::registry::{LanguageDef, QuerySet};

macro_rules! queries {
    ($dir:literal) => {
        QuerySet {
            highlights: Some(include_str!(concat!(
                "../queries/",
                $dir,
                "/highlights.scm"
            ))),
            locals: Some(include_str!(concat!("../queries/", $dir, "/locals.scm"))),
            folds: Some(include_str!(concat!("../queries/", $dir, "/folds.scm"))),
            tags: Some(include_str!(concat!("../queries/", $dir, "/tags.scm"))),
            inherits: Some(include_str!(concat!("../queries/", $dir, "/inherits.scm"))),
            injections: None,
        }
    };
    // Opt-in arm: most languages have no injected regions, and an absent
    // `injections.scm` must stay absent rather than become an empty file
    // per language just to satisfy `include_str!`.
    ($dir:literal, injections) => {
        QuerySet {
            injections: Some(include_str!(concat!(
                "../queries/",
                $dir,
                "/injections.scm"
            ))),
            ..queries!($dir)
        }
    };
}

/// The bracket pairs almost every language shares.
pub(crate) const BRACKETS: &[(&str, &str)] = &[("(", ")"), ("[", "]"), ("{", "}")];

/// For the data languages where a parenthesis is not a bracket at all.
const BRACKETS_NO_PARENS: &[(&str, &str)] = &[("[", "]"), ("{", "}")];

pub(crate) const QUOTES_DOUBLE: &[&str] = &["\""];
const QUOTES_DOUBLE_SINGLE: &[&str] = &["\"", "'"];
const QUOTES_DOUBLE_BACKTICK: &[&str] = &["\"", "`"];
const QUOTES_DOUBLE_SINGLE_BACKTICK: &[&str] = &["\"", "'", "`"];

/// Every language compiled into the binary, in resolution order.
///
/// Order is load-bearing: an extension claimed by two languages (`.h` is
/// C, C++ and Objective-C; `.ts`, `.m`, `.pl` collide too) goes to
/// whichever appears first here. Reordering this table therefore changes
/// user-visible behaviour — `first_match_wins_in_catalog_order` pins it.
///
/// Plain text is *not* in this table: the registry reserves index 0 for it
/// (see [`Language::PLAIN_TEXT`]) because it has no grammar at all.
pub const BUILTIN_LANGUAGES: &[LanguageDef] = &[
    LanguageDef {
        id: "rust",
        name: "Rust",
        extensions: &["rs"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_rust::LANGUAGE.into(),
        queries: queries!("rust"),
    },
    LanguageDef {
        id: "json",
        name: "JSON",
        extensions: &["json"],
        filenames: &[],
        line_comment: None,
        block_comment: None,
        brackets: BRACKETS_NO_PARENS,
        quotes: QUOTES_DOUBLE,
        grammar: || tree_sitter_json::LANGUAGE.into(),
        queries: queries!("json"),
    },
    LanguageDef {
        id: "csharp",
        name: "C#",
        extensions: &["cs"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_c_sharp::LANGUAGE.into(),
        queries: queries!("csharp"),
    },
    LanguageDef {
        id: "java",
        name: "Java",
        extensions: &["java"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_java::LANGUAGE.into(),
        queries: queries!("java"),
    },
    LanguageDef {
        id: "php",
        name: "PHP",
        extensions: &["php"],
        filenames: &[],
        // `LANGUAGE_PHP` (the grammar that also parses the markup around
        // `<?php … ?>`), not `LANGUAGE_PHP_ONLY`. The body-only grammar
        // was the v1 choice because there was no HTML row to hand the
        // markup to; R4d added one, so a real-world `.php` file — a
        // template with PHP embedded in it — now highlights instead of
        // parsing as one long error. See php/injections.scm.
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_php::LANGUAGE_PHP.into(),
        queries: queries!("php", injections),
    },
    LanguageDef {
        id: "python",
        name: "Python",
        extensions: &["py", "pyi", "pyw"],
        filenames: &[],
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_python::LANGUAGE.into(),
        queries: queries!("python"),
    },
    // C precedes C++ deliberately: both claim `.h`, and first match wins,
    // so a bare header opens as C (`first_match_wins_in_catalog_order`
    // pins that). C++ stays reachable through its own extensions.
    LanguageDef {
        id: "c",
        name: "C",
        extensions: &["c", "h"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_c::LANGUAGE.into(),
        queries: queries!("c"),
    },
    LanguageDef {
        id: "cpp",
        name: "C++",
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "ipp"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_cpp::LANGUAGE.into(),
        queries: queries!("cpp"),
    },
    LanguageDef {
        id: "go",
        name: "Go",
        extensions: &["go"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_BACKTICK,
        grammar: || tree_sitter_go::LANGUAGE.into(),
        queries: queries!("go"),
    },
    LanguageDef {
        id: "typescript",
        name: "TypeScript",
        // `.ts` is also MPEG transport stream; in a code editor
        // TypeScript is the only useful reading.
        extensions: &["ts", "mts", "cts"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE_BACKTICK,
        grammar: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        queries: queries!("typescript"),
    },
    LanguageDef {
        id: "tsx",
        name: "TSX",
        // A separate row, not a `.tsx` extension on `typescript`: TSX is
        // its own grammar (`<T>x` is a type assertion in .ts and a JSX
        // element in .tsx), and `grammar` is per row.
        extensions: &["tsx"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE_BACKTICK,
        grammar: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        queries: queries!("tsx"),
    },
    LanguageDef {
        id: "javascript",
        name: "JavaScript",
        // The grammar includes JSX, so `.jsx` needs no separate row.
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE_BACKTICK,
        grammar: || tree_sitter_javascript::LANGUAGE.into(),
        queries: queries!("javascript"),
    },
    LanguageDef {
        id: "bash",
        name: "Bash",
        extensions: &["sh", "bash", "zsh", "ksh"],
        // Shell dotfiles are extensionless by convention; without these
        // rows the file a user edits most often would open as plain text.
        filenames: &[
            ".bashrc",
            ".bash_profile",
            ".bash_logout",
            ".bash_aliases",
            ".profile",
            ".zshrc",
            ".zshenv",
            ".zprofile",
            ".zlogin",
            ".zlogout",
        ],
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_bash::LANGUAGE.into(),
        queries: queries!("bash"),
    },
    LanguageDef {
        id: "yaml",
        name: "YAML",
        extensions: &["yaml", "yml"],
        filenames: &[],
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS_NO_PARENS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_yaml::LANGUAGE.into(),
        queries: queries!("yaml"),
    },
    LanguageDef {
        id: "toml",
        name: "TOML",
        extensions: &["toml"],
        // `Cargo.lock` is TOML but claiming `.lock` outright would be
        // wrong — most lock files are not.
        filenames: &["Cargo.lock"],
        // `tree-sitter-toml-ng`, not `tree-sitter-toml`: the latter is
        // pinned to the tree-sitter 0.20 runtime and its `language()`
        // returns that crate's `Language`, which is a different type from
        // the 0.26 one this workspace uses.
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS_NO_PARENS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_toml_ng::LANGUAGE.into(),
        queries: queries!("toml"),
    },
    LanguageDef {
        id: "sql",
        name: "SQL",
        extensions: &["sql"],
        filenames: &[],
        // `tree-sitter-sequel` is the crate name of derekstride's SQL
        // grammar — not a typo, and not a different language.
        line_comment: Some("--"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_sequel::LANGUAGE.into(),
        queries: queries!("sql"),
    },
    LanguageDef {
        id: "ruby",
        name: "Ruby",
        extensions: &["rb", "rake", "gemspec", "ru"],
        // Ruby's build and config DSLs are extensionless by convention;
        // without these rows the `Gemfile` a user edits daily opens as
        // plain text.
        filenames: &[
            "Gemfile",
            "Rakefile",
            "rakefile",
            "Guardfile",
            "Vagrantfile",
            "Brewfile",
            "Podfile",
            "Capfile",
        ],
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_ruby::LANGUAGE.into(),
        queries: queries!("ruby"),
    },
    LanguageDef {
        id: "lua",
        name: "Lua",
        extensions: &["lua", "rockspec"],
        filenames: &[".luacheckrc"],
        line_comment: Some("--"),
        block_comment: Some(("--[[", "]]")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_lua::LANGUAGE.into(),
        queries: queries!("lua"),
    },
    LanguageDef {
        id: "make",
        name: "Makefile",
        extensions: &["mk", "mak"],
        // Make is normally reached by whole file name, not extension.
        // `filenames` is exact and case-sensitive, so every spelling GNU
        // make itself looks for gets its own entry; `Makefile.local` and
        // friends come from the suffix step of `language_for_path`.
        filenames: &[
            "Makefile",
            "makefile",
            "GNUmakefile",
            "Makefile.am",
            "Makefile.in",
        ],
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_make::LANGUAGE.into(),
        queries: queries!("make"),
    },
    LanguageDef {
        id: "dockerfile",
        name: "Dockerfile",
        extensions: &["dockerfile", "containerfile"],
        // `Dockerfile.<stage>` resolves through the suffix step of
        // `language_for_path`, and the `.dockerfile` extension above
        // covers the `<stage>.dockerfile` spelling.
        filenames: &["Dockerfile", "dockerfile", "Containerfile", "containerfile"],
        // `tree-sitter-containerfile`, not `tree-sitter-dockerfile`: the
        // latter is pinned to the tree-sitter 0.20 runtime and would drag a
        // second one into the build, exactly like `tree-sitter-toml` above.
        line_comment: Some("#"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_containerfile::LANGUAGE.into(),
        queries: queries!("dockerfile"),
    },
    LanguageDef {
        id: "kotlin",
        name: "Kotlin",
        extensions: &["kt", "kts"],
        filenames: &[],
        // `tree-sitter-kotlin-ng` (the tree-sitter-grammars fork), not
        // `tree-sitter-kotlin`: the fork is the maintained one and ships a
        // grammar the 0.26 runtime loads. It ships no queries, so
        // queries/kotlin/highlights.scm is hand-written.
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_kotlin_ng::LANGUAGE.into(),
        queries: queries!("kotlin"),
    },
    LanguageDef {
        id: "swift",
        name: "Swift",
        extensions: &["swift"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE,
        grammar: || tree_sitter_swift::LANGUAGE.into(),
        queries: queries!("swift"),
    },
    LanguageDef {
        id: "scala",
        name: "Scala",
        extensions: &["scala", "sc"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_scala::LANGUAGE.into(),
        queries: queries!("scala"),
    },
    LanguageDef {
        id: "zig",
        name: "Zig",
        // Not `.zon`: that is ZON, a separate grammar this catalog does
        // not carry, and the Zig grammar does not parse it.
        extensions: &["zig"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_zig::LANGUAGE.into(),
        queries: queries!("zig"),
    },
    LanguageDef {
        id: "haskell",
        name: "Haskell",
        extensions: &["hs"],
        filenames: &[],
        line_comment: Some("--"),
        block_comment: Some(("{-", "-}")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_haskell::LANGUAGE.into(),
        queries: queries!("haskell"),
    },
    LanguageDef {
        id: "fsharp",
        name: "F#",
        // Implementation files only. The crate also exposes
        // `LANGUAGE_SIGNATURE` for `.fsi` signature files, which is a
        // *different* grammar with different node names; since `grammar`
        // is per row (the same reason `tsx` is its own row), registering
        // `.fsi` would mean a second row with a second query set. Signature
        // files are rare enough that shipping them as plain text is
        // honest, where highlighting them with the implementation grammar
        // would not be.
        extensions: &["fs", "fsx", "fsscript"],
        filenames: &[],
        line_comment: Some("//"),
        block_comment: Some(("(*", "*)")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
        queries: queries!("fsharp"),
    },
    // The markup tranche (R4d). These four are the languages injections
    // exist for: an HTML file without its `<script>`/`<style>` regions,
    // or a Markdown file without its fenced blocks, is mostly uncoloured.
    LanguageDef {
        id: "markdown",
        name: "Markdown",
        extensions: &["md", "markdown", "mdown", "mkd"],
        filenames: &[],
        // `tree-sitter-md` is two grammars. This is the block one; it
        // leaves every run of prose as one opaque `(inline)` node and
        // injects the `markdown_inline` row over it (markdown/
        // injections.scm).
        line_comment: None,
        block_comment: Some(("<!--", "-->")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE,
        grammar: || tree_sitter_md::LANGUAGE.into(),
        queries: queries!("markdown", injections),
    },
    LanguageDef {
        id: "carve",
        name: "Carve",
        extensions: &["crv", "carve"],
        filenames: &[],
        line_comment: Some("%%"),
        block_comment: Some(("%%%", "%%%")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE,
        grammar: tree_sitter_carve::language,
        queries: QuerySet {
            highlights: Some(include_str!("../queries/carve/highlights.scm")),
            locals: Some(include_str!("../queries/carve/locals.scm")),
            folds: Some(include_str!("../queries/carve/folds.scm")),
            tags: Some(include_str!("../queries/carve/tags.scm")),
            inherits: Some(include_str!("../queries/carve/inherits.scm")),
            injections: Some(tree_sitter_carve::INJECTIONS_QUERY),
        },
    },
    LanguageDef {
        id: "markdown_inline",
        name: "Markdown (inline)",
        // Deliberately pattern-less: no file is written in the inline
        // grammar, it is only ever reached by injection from the
        // `markdown` row above. `declared_patterns_resolve_back_to_a_claimant`
        // allows that precisely because another catalog row injects this id.
        extensions: &[],
        filenames: &[],
        line_comment: None,
        block_comment: Some(("<!--", "-->")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE,
        grammar: || tree_sitter_md::INLINE_LANGUAGE.into(),
        queries: queries!("markdown_inline", injections),
    },
    LanguageDef {
        id: "mermaid",
        name: "Mermaid",
        // Kept in step by hand with the `previews` contribution in
        // crates/plugin-host/builtin/markdown-preview/plugin.toml, which
        // claims the same two extensions for the diagram preview;
        // `plugin-host` cannot depend on this crate (layering.md), so
        // nothing but this comment enforces the pair.
        extensions: &["mermaid", "mmd"],
        filenames: &[],
        // `%%` to end of line. Mermaid has no block comment at all — its
        // own docs call `%%` the only comment form.
        line_comment: Some("%%"),
        block_comment: None,
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE,
        grammar: || tree_sitter_mermaid::LANGUAGE.into(),
        // The grammar ships its own injections (a typed Event Modeling data
        // block carries JSON, Markdown or HTML; an XY chart's label text is
        // Markdown), so this is one of the few rows that opts in.
        queries: queries!("mermaid", injections),
    },
    LanguageDef {
        id: "html",
        name: "HTML",
        extensions: &["html", "htm", "xhtml"],
        filenames: &[],
        line_comment: None,
        block_comment: Some(("<!--", "-->")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_html::LANGUAGE.into(),
        queries: queries!("html", injections),
    },
    LanguageDef {
        id: "css",
        name: "CSS",
        extensions: &["css"],
        filenames: &[],
        line_comment: None,
        block_comment: Some(("/*", "*/")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_css::LANGUAGE.into(),
        queries: queries!("css"),
    },
    LanguageDef {
        id: "xml",
        name: "XML",
        // `LANGUAGE_XML`, not the crate's second `LANGUAGE_DTD` grammar:
        // a `.dtd` file is rare enough in an editor that it does not earn
        // a catalog row, and pointing the `xml` row at it would break
        // every actual XML document.
        extensions: &["xml", "xsd", "xsl", "xslt", "svg", "rss", "wsdl"],
        filenames: &[],
        line_comment: None,
        block_comment: Some(("<!--", "-->")),
        brackets: BRACKETS,
        quotes: QUOTES_DOUBLE_SINGLE,
        grammar: || tree_sitter_xml::LANGUAGE_XML.into(),
        queries: queries!("xml"),
    },
];
