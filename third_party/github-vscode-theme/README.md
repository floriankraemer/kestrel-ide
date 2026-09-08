# github-vscode-theme

Vendored from `primer/github-vscode-theme` v6.3.5, MIT licensed.
Source: the published VSIX from open-vsx.org (`PKief`-style pinning is not used here since there is only one file per theme, not a zipped icon set).
Not built from source: the upstream repository generates these JSON files at package time from `@primer/primitives`, and does not commit the built output, so there is no buildable source tree to vendor instead.

## Layout

- `upstream/*.json` — the nine theme files exactly as shipped in the VSIX, kept so the importer below is re-runnable against a newer release.
- `themes/*.toml` — this IDE's native colour-theme shape (`crates/color-theme`'s `parse_toml`), derived from `upstream/*.json`.
- `plugin.toml` — the plugin manifest, hand-written (nine static `[[contributes.color-themes]]` entries that do not change shape between releases).
- `LICENSE` — the upstream MIT licence, copied verbatim.

## Regenerating `themes/*.toml`

`scripts/import-vscode-theme.py` re-derives every `themes/*.toml` file from the matching `upstream/*.json` file, using `color_theme::parse_vscode_json` — the same VS Code theme parser this IDE's Import Theme feature runs at runtime — rather than a second, hand-rolled mapping that could drift from it.
To pick up a new upstream release: replace the files under `upstream/` with the new VSIX's `extension/themes/*.json`, then re-run the script.
See that script's header comment for invocation details.
