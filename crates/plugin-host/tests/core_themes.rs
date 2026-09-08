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

// --- T10: an installed (not built-in) theme plugin --------------------
//
// Everything above exercises only the embedded-built-in path
// (`BuiltinPlugin`/`include_bytes!`). These two tests prove the actual
// user-facing install path: a plugin directory dropped into
// `<config_dir>/plugins/` with a hand-written manifest and asset file on
// disk, loaded through the same `plugin_host::load` a real launch uses.

/// Writes `<config_dir>/plugins/<dir_name>/plugin.toml` plus one asset file
/// next to it, the way a user's own plugin install would look on disk.
fn install_plugin(
    config_dir: &std::path::Path,
    dir_name: &str,
    manifest: &str,
    asset_name: &str,
    asset_contents: &str,
) {
    let plugin_dir = config_dir.join(plugin_host::PLUGINS_DIR).join(dir_name);
    std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
    std::fs::write(plugin_dir.join(plugin_api::MANIFEST_FILE), manifest).expect("manifest");
    std::fs::write(plugin_dir.join(asset_name), asset_contents).expect("asset");
}

/// A minimal but complete native TOML theme, self-contained (not copied
/// from `builtin/core-themes/dark.toml`) so this test does not depend on
/// that file's future contents.
fn minimal_native_theme_toml(id: &str, label: &str) -> String {
    format!(
        r##"
        id = "{id}"
        label = "{label}"
        appearance = "dark"

        [chrome]
        canvas = "#101010"
        surface = "#202020"
        surface2 = "#252525"
        raised = "#303030"
        border = "#3a3a3a"
        text = "#eeeeee"
        text_dim = "#999999"
        accent = "#4488ff"
        accent_ink = "#ffffff"
        selection = "#334466"
        status_bar = "#202020"

        [semantic]
        error = "#ff5555"
        warning = "#ffb454"
        info = "#59a1e0"
        ok = "#7fd962"
        muted = "#8a8f98"

        [diff]
        added_line = "#1e3a1e"
        added_inline = "#2d5a2d"
        added_marker = "#4caf50"
        modified_line = "#1e2f4a"
        modified_inline = "#2d4a70"
        modified_marker = "#3574f0"
        deleted_line = "#3a1e1e"
        deleted_inline = "#5a2d2d"
        deleted_marker = "#f44336"

        [terminal]
        black = "#000000"
        red = "#ff5555"
        green = "#7fd962"
        yellow = "#ffb454"
        blue = "#59a1e0"
        magenta = "#c678dd"
        cyan = "#56b6c2"
        white = "#dfe1e5"
        bright_black = "#5c6370"
        bright_red = "#ff6e67"
        bright_green = "#9ae37c"
        bright_yellow = "#ffd479"
        bright_blue = "#7fb8ea"
        bright_magenta = "#d8a1e6"
        bright_cyan = "#79cdd8"
        bright_white = "#ffffff"
        background = "#101010"
        foreground = "#eeeeee"
        cursor = "#eeeeee"
        selection = "#334466"

        [syntax.keyword]
        fg = "#cc7832"
        bold = true
        "##
    )
}

/// A minimal, hand-written VS Code theme JSON fixture — not one of the
/// vendored `github-vscode-theme` variants — the shape a real user's
/// unmodified `.json` theme file installed alongside a manifest has.
fn minimal_vscode_json(name: &str) -> String {
    format!(
        r##"{{
            "name": "{name}",
            "type": "dark",
            "colors": {{
                "editor.background": "#112233",
                "editor.foreground": "#eeeeee",
                "focusBorder": "#4488ff",
                "editorError.foreground": "#ff5555",
                "terminal.ansiBlack": "#000000",
                "terminal.ansiRed": "#ff5555"
            }},
            "tokenColors": [
                {{
                    "scope": "keyword",
                    "settings": {{ "foreground": "#ff5555" }}
                }}
            ]
        }}"##
    )
}

#[test]
fn an_installed_toml_theme_is_offered_alongside_the_built_ins_and_reads_and_parses() {
    let config_dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(config_dir.path().join(plugin_host::PLUGINS_DIR))
        .expect("plugins root");
    let manifest = r#"
        id = "my-theme"
        name = "My Theme Plugin"
        version = "1.0.0"
        api_version = 1

        [[contributes.color-themes]]
        id = "my-toml-theme"
        label = "My TOML Theme"
        path = "my-theme.toml"
    "#;
    install_plugin(
        config_dir.path(),
        "my-theme",
        manifest,
        "my-theme.toml",
        &minimal_native_theme_toml("my-toml-theme", "My TOML Theme"),
    );

    let registry = plugin_host::load(config_dir.path(), BUILTIN_PLUGINS, &[]);
    assert_eq!(
        registry.errors(),
        &[],
        "the installed plugin must load clean"
    );

    let ids: Vec<&str> = registry
        .color_themes()
        .map(|(_, contribution)| contribution.id.as_str())
        .collect();
    // Addition, not replacement: the installed theme is offered alongside
    // the built-ins, not instead of them.
    assert!(ids.contains(&"my-toml-theme"));
    assert!(
        ids.contains(&"dark"),
        "built-ins are still offered: {ids:?}"
    );

    let (plugin, contribution) = registry
        .color_themes()
        .find(|(_, c)| c.id == "my-toml-theme")
        .expect("installed theme is in the registry");
    let bytes = plugin
        .read_asset(&contribution.path)
        .expect("installed theme asset reads");
    let theme = parse_toml(&String::from_utf8_lossy(&bytes)).expect("installed theme parses");
    assert_eq!(theme.id, "my-toml-theme");
    assert_eq!(theme.appearance, color_theme::Appearance::Dark);
    assert_eq!(
        theme.chrome.canvas,
        color_theme::Rgba::new(0x10, 0x10, 0x10, 255)
    );
    assert_eq!(
        theme.semantic.error,
        color_theme::Rgba::new(0xff, 0x55, 0x55, 255)
    );
}

#[test]
fn an_installed_unmodified_vscode_json_theme_is_offered_alongside_the_built_ins_and_reads_and_parses(
) {
    let config_dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(config_dir.path().join(plugin_host::PLUGINS_DIR))
        .expect("plugins root");
    let manifest = r#"
        id = "my-vscode-theme"
        name = "My VS Code Theme Plugin"
        version = "1.0.0"
        api_version = 1

        [[contributes.color-themes]]
        id = "my-json-theme"
        label = "My JSON Theme"
        path = "my-vscode-theme.json"
    "#;
    install_plugin(
        config_dir.path(),
        "my-vscode-theme",
        manifest,
        "my-vscode-theme.json",
        &minimal_vscode_json("My JSON Theme"),
    );

    let registry = plugin_host::load(config_dir.path(), BUILTIN_PLUGINS, &[]);
    assert_eq!(
        registry.errors(),
        &[],
        "the installed plugin must load clean"
    );

    let ids: Vec<&str> = registry
        .color_themes()
        .map(|(_, contribution)| contribution.id.as_str())
        .collect();
    assert!(ids.contains(&"my-json-theme"));
    assert!(
        ids.contains(&"dark"),
        "built-ins are still offered: {ids:?}"
    );

    let (plugin, contribution) = registry
        .color_themes()
        .find(|(_, c)| c.id == "my-json-theme")
        .expect("installed theme is in the registry");

    // Exercises the real extension-dispatch path (`.json` -> `parse_vscode_json`),
    // the same dispatch `app_core::color_themes::parse_by_extension` does,
    // rather than calling `parse_vscode_json` directly.
    let bytes = plugin
        .read_asset(&contribution.path)
        .expect("installed theme asset reads");
    let text = String::from_utf8_lossy(&bytes);
    let theme = match contribution.path.extension().and_then(|e| e.to_str()) {
        Some("json") => color_theme::parse_vscode_json(&text),
        Some("toml") => parse_toml(&text),
        other => panic!("unexpected extension: {other:?}"),
    }
    .expect("installed theme parses");

    assert_eq!(theme.label, "My JSON Theme");
    assert_eq!(theme.appearance, color_theme::Appearance::Dark);
    assert_eq!(
        theme.chrome.canvas,
        color_theme::Rgba::new(0x11, 0x22, 0x33, 255)
    );
    assert_eq!(
        theme.semantic.error,
        color_theme::Rgba::new(0xff, 0x55, 0x55, 255)
    );
}
