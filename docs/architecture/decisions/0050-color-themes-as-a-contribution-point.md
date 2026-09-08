# 0050. Colour themes as a `color-themes` contribution point

## Status

Accepted

## Context

Before this plan, `crates/syntax-core/src/theme.rs` held three colour themes — `DARCULA`, `LIGHT`, `VSCODE_DARK` — as Rust static tables, matched by name (`theme_by_name`), and `crates/ui-shell/cpp/theme.cpp` mirrored that same three-way branch for chrome, semantic, diff and terminal colours: six separate `themeName == "..."` compare chains, one per colour family.
Adding a fourth theme meant editing all of them, in lockstep, in two languages.
The GitHub (Primer) theme family — nine variants a user would recognise from VS Code's Marketplace — was the forcing case: shipping it as more hardcoded Rust/C++ branches would have tripled the compare chains for data that has nothing language-specific about it, and a user who already owns a VS Code theme file would still have had no way to use it here at all.

Four questions had to be answered before any code could be written, because each has a wrong-by-construction answer that only "it renders the same three themes" would hide:

1. **What file format does a colour theme ship as?**
   TOML is this codebase's native format for everything else a plugin contributes (`plugin.toml`, `pack.toml`).
   VS Code's own theme files are JSON with a `colors` workbench map and a `tokenColors` TextMate array — a format tens of thousands of existing themes already exist in, including the nine Primer variants this plan needed to vendor.
2. **Where does the VS Code workbench-key-to-token mapping live?**
   A one-off Python script could convert a VS Code JSON theme to this app's TOML shape at import time and never run again.
   That would still leave every *other* VS Code theme a user might already have on disk unusable without first finding and running that script themselves.
3. **Where does theme *resolution* — built-in-vs-plugin precedence, a scope's parent-scope inheritance when a theme has no entry of its own — live?**
   `syntax-core` already owns exactly this resolution logic for the three original themes (`palette()`, `theme_by_name`), and `color-theme` is a new, deliberately small crate with no existing consumer of a resolved palette.
4. **Does the chevron/shadow art need one asset per theme, or one per light/dark appearance?**
   `crates/ui-shell/resources/icons/` had a dedicated `chevron_vscode_dark.png` alongside a generic dark and light chevron — three theme names, but only two visually distinct chevrons, because VSCode Dark and the original Dark theme share the same near-black background a light-grey chevron reads fine against.

## Decision

### 1. TOML-native, VS Code JSON accepted at runtime

`color_theme::parse_toml` is the crate's native parser and the format every built-in theme (`core-themes`, `github-vscode-theme`) ships as.
`color_theme::parse_vscode_json` is a second, equally first-class entry point: `app_core::color_themes::parse_by_extension` dispatches on the contributed file's own extension, `.toml` to one parser and `.json` to the other, so a `color-themes` contribution can point at either format and the join in `app-core` never needs to know which one a given plugin chose.
Neither format is preferred at the type level — `ColorTheme` is the same struct whichever parser produced it — so a raw, unmodified VS Code theme JSON dropped into `<config_dir>/plugins/` is offered and resolves exactly like a native TOML theme, proven by the integration test in `crates/plugin-host/tests/core_themes.rs`.

### 2. The workbench-key mapping is runtime code, not an import-only script

`crates/color-theme/src/vscode.rs` is a full, always-available VS Code-to-`ColorTheme` mapping table: the workbench `colors` keys with their fallback chains, and the `SCOPES` table mapping this app's roughly thirty tree-sitter capture scopes onto TextMate selectors so `tokenColors` rules resolve the same way VS Code's own token colorizer would.
This is what lets a user install a VS Code theme JSON they already own directly, with no conversion step and no dependency on this repository's own tooling, rather than only being able to use themes someone has already run a script over.
`scripts/import-vscode-theme.py` and `crates/color-theme/examples/import_vscode_theme.rs` exist *in addition* to this — a regeneration path for producing the vendored native-TOML copies in `third_party/github-vscode-theme/` from upstream, not a replacement for the runtime parser. Both paths share the same mapping table so they can never drift against each other.

### 3. Theme resolution stays in `syntax-core`, not `color-theme`

`color-theme` parses a theme file into data and stops there: it has no notion of "the active theme," no precedence between a built-in and an installed plugin, and no inheritance walk.
`crates/syntax-core/src/theme.rs` gained `ThemeStyles` and `build_palette()` (T4) as the resolution entry point for a `ColorTheme`-shaped theme, sitting alongside the existing name-keyed `palette()` rather than replacing it — see the deviation in decision 5 below — and it does the parent-scope inheritance walk (`resolve_scopes`/`parent`) that decides what a scope with no entry of its own inherits.
This split is deliberate and mirrors `icon-theme`'s own isolation rule: `color-theme` must never depend on `syntax-core`, so a TextMate scope selector is a plain `&str` on both sides of that boundary and no `Scope` enum crosses it.
Putting resolution in `color-theme` instead would have given the crate a `syntax-core` edge for a single join it does not otherwise need, and would have duplicated `syntax-core`'s existing inheritance walk rather than reusing it.

### 4. Chevron and shadow art keyed by appearance, not by theme name

The six `themeName == "..."` compare chains in `theme.cpp` (`chromePaletteForTheme`, `semanticColorsForTheme`, `diffColorsForTheme`, `terminalPaletteForTheme`, and the two chevron/shadow lookups) are gone; all four palette functions now read through `ThemeProvider`'s per-contribution data instead of a name branch, and the chevron/shadow assets are chosen by `Appearance` (light or dark), not by theme id.
`chevron_vscode_dark.png` is deleted: VSCode Dark and Dark share `Appearance::Dark`, and the plan's own justification — one grey glyph per appearance, not one per theme — is the same reasoning a `ponytail`-style read of the ladder gives for any asset variant that only exists because nobody asked whether it needed to: twelve theme ids (three built-in plus nine Primer variants) would otherwise want up to twelve near-identical PNGs for an artifact that only ever has two visually distinct states.

### 5. Deliberate deviation: `syntax-core`'s old static tables were not deleted

The original plan text said `DARCULA`/`LIGHT`/`VSCODE_DARK`, `BUILTIN_THEMES`, `theme_by_name`, and the name-based `palette()` would be deleted once `ThemeStyles`/`build_palette` landed.
They were kept, on purpose: `crates/markdown-preview/src/highlight.rs` still calls the name-based `palette()` directly, and rewiring markdown-preview's own theming onto the contribution-based path was out of this plan's scope — it was never a task in the Progress table, and doing it as a drive-by inside T4 would have widened this plan into a second one with its own risk surface.
This is recorded honestly as a known follow-up, not papered over: `syntax-core` currently carries two parallel ways to get a palette (name-keyed, for `markdown-preview`; contribution-keyed, for everything reached through `ui-shell`'s `ThemeProvider`), and a future task should retire the first once `markdown-preview` no longer needs it.

## Alternatives considered

| Option | Why rejected |
|---|---|
| TOML-only, require a VS Code theme to be pre-converted | Locks out every VS Code theme a user already owns unless they run a conversion tool first; defeats the plugin model's own promise that dropping a file into `<config_dir>/plugins/` is enough. |
| JSON-only (VS Code's shape as the native format) | This app's own built-in themes and every other plugin manifest are TOML; adopting VS Code's JSON as the *native* format would make `color-theme` the one contribution point that reads unlike every other one, for no benefit to a user who never needs to hand-author a theme in VS Code's own dialect. |
| Workbench-key mapping as an import-only script, no runtime `parse_vscode_json` | Produces pre-converted TOML a user can install, but never lets them install a VS Code theme JSON file directly — see decision 2. |
| Theme resolution (precedence, inheritance) inside `color-theme` | Gives a crate designed to be a leaf a `syntax-core` dependency for one join, and duplicates the inheritance walk `syntax-core::theme::resolve_scopes` already has — see decision 3. |
| One chevron/shadow asset per theme id | Twelve near-identical PNGs (three built-in, nine Primer) for two visually distinct states; keeping `chevron_vscode_dark.png` around after VSCode Dark and Dark already share a background was never buying anything. |
| Delete `syntax-core`'s old static theme tables in this plan, rewiring `markdown-preview` too | Widens T4 into a second, unplanned migration of `markdown-preview`'s theming with its own risk surface, for a plan whose Progress table never scoped that work — see decision 5. |

## Consequences

- Positive: adding another theme — built-in or user-installed, TOML or VS Code JSON — costs a manifest entry and a data file, never a new branch in `theme.cpp` or a new static table in `syntax-core`.
- Positive: a user can install any VS Code theme JSON they already have, unmodified, with the same install path as any other plugin; proven by `crates/plugin-host/tests/core_themes.rs` and `crates/app-core/src/color_themes.rs`'s own integration tests.
- Positive: persisted `settings.toml` theme ids (`"dark"`, `"light"`, `"vscode-dark"`) are unchanged, so no settings migration was needed for existing installs.
- Negative / accepted: `syntax-core` now carries two ways to resolve a palette — the old name-keyed `palette()`/`BUILTIN_THEMES` (kept for `markdown-preview`) and the new `ThemeStyles`/`build_palette()` (used everywhere else). This is drift, not a bug, and is tracked as a follow-up: retire the name-keyed path once `markdown-preview`'s theming is rewired onto contributions.
- Negative / accepted: `ui-shell` gained a direct dependency on `color-theme` (for `ThemeProviderRust`'s field-by-field translation into `FfiChromePalette`/`FfiSemanticColors`/`FfiDiffColors`/`FfiColorThemeChoice`) that the original plan text did not anticipate — the same shape `convert.rs` already has for `syntax_core::theme::ScopeStyle` → `FfiScopeStyle`, and recorded in `docs/architecture/layering.md`'s `ui-shell` row rather than left as an undocumented edge.

## Related

- [ADR-0026: plugin host](0026-plugin-host.md) — the contribution-point pattern (`plugin-api` as leaf contract, `plugin-host` as discovery/registry) `color-themes` follows.
- [ADR-0027: icon themes](0027-icon-themes.md) — the isolation precedent (`icon-theme` depends on neither `syntax-core` nor `plugin-host`) `color-theme`'s own leaf status mirrors.
- [ADR-0018: single-source language detection](0018-single-source-language-detection.md) — the same one-table rule that keeps a TextMate scope selector a plain string rather than a second `Scope`-like enum.
- `docs/architecture/color-themes-plan.md` — the full task-by-task history (T1–T11) this ADR summarizes the end state of.
