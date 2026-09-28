//! The vendored Material pack, loaded the way the editor loads it.
//!
//! An integration test rather than a unit one, because what it guards is
//! the whole seam: `scripts/import-material-icons.py` produced a pack,
//! `builtins.rs` embedded it, `plugin-host` parsed its manifest and
//! `icon-theme` has to be able to resolve and rasterise out of it. A bad
//! regeneration passes every unit test in both crates and fails here.
//!
//! It lives in `plugin-host` (with a dev-dependency on `icon-theme`)
//! because the artefact under test is `BUILTIN_PLUGINS`, which is this
//! crate's; `icon-theme` is the validator, not the subject. The dependency
//! is dev-only, so the layering rule that keeps the two crates unaware of
//! each other at runtime (ADR-0026, ADR-0027) is untouched.
//!
//! The assertions are properties, not the table: the pack has 13 020
//! entries and pinning them here would only re-state the generated file.

use std::path::Path;

use icon_theme::{Appearance, IconAssets, IconError, IconPack, IconRenderer};
use plugin_host::{LoadedPlugin, PluginSource, BUILTIN_PLUGINS};

/// The built-in plugin, loaded through the real discovery path with an
/// empty plugins directory — exactly what a first launch does.
fn material() -> LoadedPlugin {
    let config_dir = tempfile::tempdir().expect("temp dir");
    let registry = plugin_host::load(config_dir.path(), BUILTIN_PLUGINS, &[]);
    assert_eq!(
        registry.errors(),
        &[],
        "the vendored plugin must load clean"
    );
    let plugin = registry
        .by_id("material-icons")
        .expect("the Material icon theme is a built-in");
    assert_eq!(plugin.source(), PluginSource::Builtin);
    plugin.clone()
}

/// Reads the embedded files, which is what `app-core` will do at P5.
struct Assets(LoadedPlugin);

impl IconAssets for Assets {
    fn read(&self, relative: &Path) -> Result<Vec<u8>, IconError> {
        self.0
            .read_asset(relative)
            .map(|bytes| bytes.into_owned())
            .map_err(|err| IconError::UnreadableAsset {
                path: relative.display().to_string(),
                message: err.to_string(),
            })
    }
}

fn pack(plugin: &LoadedPlugin) -> IconPack {
    let theme = plugin
        .manifest()
        .contributes
        .icon_themes
        .first()
        .expect("the plugin contributes one icon theme");
    let text = plugin
        .read_asset(&theme.pack)
        .expect("pack.toml is embedded");
    IconPack::from_toml_str(&String::from_utf8_lossy(&text)).expect("pack.toml parses")
}

#[test]
fn the_vendored_pack_parses_and_resolves() {
    let plugin = material();
    let pack = pack(&plugin);
    assert_eq!(pack.id, "material");

    // Extension, then file name, then folder, in both states.
    assert_eq!(pack.file_icon("main.rs", None, Appearance::Dark), "rust");
    assert_eq!(
        pack.file_icon("Dockerfile", None, Appearance::Dark),
        "docker"
    );
    assert_eq!(
        pack.folder_icon("src", false, Appearance::Dark),
        "folder-src"
    );
    assert_eq!(
        pack.folder_icon("src", true, Appearance::Dark),
        "folder-src-open"
    );

    // Upstream 5.38.1 has no `cargo` icon and no `cargo.toml` file-name
    // entry, so `Cargo.toml` resolves through the extension table. Asserted
    // rather than assumed: if a later import adds one, this line is where
    // that shows up.
    assert_eq!(pack.file_icon("Cargo.toml", None, Appearance::Dark), "toml");

    // The compressed light table substitutes after resolution.
    assert_eq!(
        pack.file_icon("Cargo.toml", None, Appearance::Light),
        "toml_light"
    );

    // Nothing matches: the pack default, not a panic and not an empty id.
    assert_eq!(pack.file_icon("notes.qqzz", None, Appearance::Dark), "file");
}

#[test]
fn a_resolved_icon_rasterises() {
    let plugin = material();
    let pack = pack(&plugin);
    let icon_id = pack.file_icon("main.rs", None, Appearance::Dark);

    let mut renderer = IconRenderer::new();
    let icon = renderer
        .render(&pack, &Assets(plugin), icon_id, 16)
        .expect("the SVG behind a resolved icon rasterises");

    assert_eq!((icon.width, icon.height), (16, 16));
    // Not merely the right size: a pack whose art failed to parse would
    // render a fully transparent square.
    assert!(
        icon.pixels
            .iter()
            .skip(3)
            .step_by(4)
            .any(|&alpha| alpha > 0),
        "the rasterised icon is entirely transparent"
    );

    // `IconRenderer` falls back to the default file icon when an id has no
    // usable art, so "it produced pixels" alone would also pass for a
    // missing `rust.svg`. These pixels have to be the Rust ones.
    let default = renderer
        .render(&pack, &Assets(material()), "file", 16)
        .expect("the default file icon rasterises");
    assert_ne!(icon.pixels, default.pixels);
}
