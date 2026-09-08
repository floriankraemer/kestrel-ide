//! The built-in `core-themes` and `github-vscode-theme` plugins, loaded
//! the way the editor loads them.
//!
//! An integration test rather than a unit one, for the same reason
//! `material_icon_theme.rs` is: what it guards is the whole seam —
//! `builtins.rs` embedded the theme files, `plugin-host` parsed the
//! manifests pointing at them, and `color-theme` has to be able to parse
//! what comes back. It lives in `plugin-host` (with a dev-dependency on
//! `color-theme`) because the artefact under test is `BUILTIN_PLUGINS`,
//! which is this crate's; `color-theme` is the validator, not the subject.
//!
//! Both plugins vendor their themes as native TOML (`parse_toml`), not VS
//! Code JSON: `github-vscode-theme`'s upstream JSON was converted to TOML
//! ahead of time by `scripts/import-vscode-theme.py` (color-themes plan
//! T9), the same as `core-themes`'s own three. The `.json`/`parse_vscode_json`
//! runtime path is only exercised when an end user installs an unmodified
//! VS Code theme file directly — that is T10's job, not this one's.

use color_theme::parse_toml;
use plugin_host::BUILTIN_PLUGINS;

const EXPECTED_IDS: &[&str] = &[
    "dark",
    "light",
    "vscode-dark",
    "github-light-default",
    "github-light-high-contrast",
    "github-light-colorblind",
    "github-dark-default",
    "github-dark-high-contrast",
    "github-dark-colorblind",
    "github-dark-dimmed",
    "github-light",
    "github-dark",
];

#[test]
fn every_core_theme_contribution_reads_and_parses() {
    let config_dir = tempfile::tempdir().expect("temp dir");
    let registry = plugin_host::load(config_dir.path(), BUILTIN_PLUGINS, &[]);
    assert_eq!(
        registry.errors(),
        &[],
        "the built-in theme plugins must load clean"
    );

    let mut ids: Vec<&str> = registry
        .color_themes()
        .map(|(_, contribution)| contribution.id.as_str())
        .collect();
    ids.sort_unstable();
    let mut expected: Vec<&str> = EXPECTED_IDS.to_vec();
    expected.sort_unstable();
    assert_eq!(ids, expected);

    for (plugin, contribution) in registry.color_themes() {
        let bytes = plugin
            .read_asset(&contribution.path)
            .unwrap_or_else(|err| panic!("{}: asset reads: {err}", contribution.id));
        let theme = parse_toml(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|err| panic!("{}: theme parses: {err}", contribution.id));
        assert_eq!(theme.id, contribution.id);
    }
}

/// A regression check that the vendored `themes/*.toml` and the live
/// `color_theme::parse_vscode_json` stay in sync: this reads one of the
/// upstream JSON files vendored alongside the generated TOML (not the
/// generated TOML itself) and asserts the runtime JSON parser — the one an
/// end user's own dropped-in `.json` theme file goes through — still
/// produces a sane, non-panicking result. If this ever fails, the vendored
/// TOML is stale: re-run `scripts/import-vscode-theme.py`.
#[test]
fn a_real_vendored_upstream_json_file_parses_via_parse_vscode_json() {
    let json = include_str!("../../../third_party/github-vscode-theme/upstream/dark-default.json");
    let theme = color_theme::parse_vscode_json(json).expect("upstream JSON parses");
    assert_eq!(theme.appearance, color_theme::Appearance::Dark);
    assert!(!theme.id.is_empty());
    assert!(!theme.syntax.is_empty());
}
