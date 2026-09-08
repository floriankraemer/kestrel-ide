//! The built-in `core-themes` plugin, loaded the way the editor loads it.
//!
//! An integration test rather than a unit one, for the same reason
//! `material_icon_theme.rs` is: what it guards is the whole seam —
//! `builtins.rs` embedded three theme files, `plugin-host` parsed the
//! manifest pointing at them, and `color-theme` has to be able to parse
//! what comes back. It lives in `plugin-host` (with a dev-dependency on
//! `color-theme`) because the artefact under test is `BUILTIN_PLUGINS`,
//! which is this crate's; `color-theme` is the validator, not the subject.

use color_theme::parse_toml;
use plugin_host::BUILTIN_PLUGINS;

#[test]
fn every_core_theme_contribution_reads_and_parses() {
    let config_dir = tempfile::tempdir().expect("temp dir");
    let registry = plugin_host::load(config_dir.path(), BUILTIN_PLUGINS, &[]);
    assert_eq!(
        registry.errors(),
        &[],
        "the built-in theme plugin must load clean"
    );

    let mut ids: Vec<&str> = registry
        .color_themes()
        .map(|(_, contribution)| contribution.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["dark", "light", "vscode-dark"]);

    for (plugin, contribution) in registry.color_themes() {
        let bytes = plugin
            .read_asset(&contribution.path)
            .unwrap_or_else(|err| panic!("{}: asset reads: {err}", contribution.id));
        let theme = parse_toml(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|err| panic!("{}: theme parses: {err}", contribution.id));
        assert_eq!(theme.id, contribution.id);
    }
}
