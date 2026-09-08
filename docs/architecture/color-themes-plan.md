# Colour themes as plugins + the GitHub (Primer) theme family

Full plan: see the PR description / original plan doc for context and rationale.
Summary: colour themes become a plugin contribution (`color-theme` crate + `color-themes`
manifest point), the three existing built-in themes migrate to data with unchanged ids, and
the nine `github-vscode-theme` (Primer) variants ship as a vendored built-in plugin, all reachable
through the same `<config_dir>/plugins/` install path a user-supplied theme uses.

## Progress

| # | Task | Status | Commit |
|---|------|--------|--------|
| T1 | `color-theme` crate: `Rgba`, the structs, `parse_toml`, errors + unit tests | Done | fa59c0b |
| T2 | `color-theme`: `parse_vscode_json`, workbench-key mapping, TextMate→`SCOPES` table + tests | Done | 92b65e9 |
| T3 | `plugin-api`: `ColorThemes` point, `ColorThemeContribution`, registry lookup + tests | Done | 3ca3cf8 |
| T4 | `syntax-core`: `ThemeStyles` + `build_palette()` added; `ui-shell`'s two name-based callers (`convert.rs`, `settings.rs`) rewired onto it in T7. Static tables/`BUILTIN_THEMES`/name-based `palette()` deliberately remain: `markdown-preview::highlight` still calls them and rewiring its theming is out of this plan's scope. | Done | d1bf96b |
| T5 | `core-themes` built-in plugin: the three existing themes as TOML, wired into `builtins.rs` | Done | 0c98fc3 |
| T6 | `app_core::color_themes`: choices, `ColorThemeService`, `appearance_for_theme` from data + tests | Done | d579154 |
| T7 | FFI + `ThemeProvider`; `theme.cpp` reads the palette from the seam | Done | 1df6835, 1754db2, d1bf96b |
| T8 | `appearance_page.cpp`: combo from the registry | Done | d25a8d9 |
| T9 | `scripts/import-vscode-theme.py`, vendor `third_party/github-vscode-theme/`, nine TOMLs | Not started | |
| T10 | Integration test: theme plugin installed into a temp `<config_dir>/plugins/` | Not started | |
| T11 | ADR-0050, `layering.md`, `docs/README.md`, overview + `language-platform-ui.md` truth-up | Not started | |

Note: the plan's suggested ADR number 0049 was taken by another merged change
(`0049-ui-internationalization.md`) by the time this work started; this work uses **ADR-0050**.
