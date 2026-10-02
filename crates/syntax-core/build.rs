//! Compiles the vendored Twig and Blade tree-sitter grammars (ADR-0071).
//!
//! Neither grammar is published on crates.io, so their generated C sources
//! live under `grammars/` and are built here. Plain C11 through the `cc`
//! crate, no flags beyond the standard, so the MinGW cross build (MXE) and
//! the native ones compile the same files.

fn main() {
    for grammar in ["twig", "blade"] {
        let dir = std::path::Path::new("grammars").join(grammar);
        let mut build = cc::Build::new();
        build
            .include(&dir)
            .std("c11")
            .warnings(false)
            .file(dir.join("parser.c"));
        let scanner = dir.join("scanner.c");
        if scanner.exists() {
            build.file(scanner);
        }
        build.compile(&format!("tree-sitter-{grammar}"));
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
}
