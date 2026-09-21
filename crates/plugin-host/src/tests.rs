//! The rules that earn a test: what loads, what is skipped and why, what
//! the user's disabled list does, and that a reload cannot pull the ground
//! out from under a consumer.

use std::fs;
use std::path::{Path, PathBuf};

use plugin_api::LoadErrorKind;
use tempfile::TempDir;

use super::*;

/// A config directory with a plugins root, written the way a user's would
/// be.
struct Fixture(TempDir);

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir(dir.path().join(PLUGINS_DIR)).expect("plugins root");
        Self(dir)
    }

    fn config_dir(&self) -> &Path {
        self.0.path()
    }

    fn plugins_dir(&self) -> PathBuf {
        self.0.path().join(PLUGINS_DIR)
    }

    /// Install a plugin directory named `dir_name` holding `manifest`.
    fn install(&self, dir_name: &str, manifest: &str) -> PathBuf {
        let dir = self.plugins_dir().join(dir_name);
        fs::create_dir_all(&dir).expect("plugin dir");
        fs::write(dir.join(MANIFEST_FILE), manifest).expect("manifest");
        dir
    }

    fn quarantine(&self, id: &str) {
        let dir = self.plugins_dir().join(QUARANTINE_DIR);
        fs::create_dir_all(&dir).expect("quarantine dir");
        fs::write(dir.join(id), "").expect("marker");
    }

    fn load(&self, disabled: &[String]) -> PluginRegistry {
        load(self.config_dir(), &[], disabled)
    }
}

fn manifest_for(id: &str) -> String {
    format!(
        r#"
        id = "{id}"
        name = "Plugin {id}"
        version = "1.0.0"
        api_version = 1

        [[contributes.icon-themes]]
        id = "{id}"
        label = "Theme {id}"
        pack = "pack.toml"
        "#
    )
}

const BUILTIN_MANIFEST: &str = r#"
id = "material-icons"
name = "Material Icon Theme"
version = "5.38.1"
api_version = 1

[[contributes.icon-themes]]
id = "material"
label = "Material"
pack = "pack.toml"
"#;

const BUILTIN: BuiltinPlugin = BuiltinPlugin {
    manifest: BUILTIN_MANIFEST,
    files: &[("pack.toml", b"default = \"file\"\n")],
};

#[test]
fn a_good_manifest_loads_with_its_contributions() {
    let fixture = Fixture::new();
    fixture.install("acme.icons", &manifest_for("acme.icons"));

    let registry = fixture.load(&[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());
    assert_eq!(registry.plugins().len(), 1);

    let plugin = registry.by_id("acme.icons").expect("loaded");
    assert_eq!(plugin.source(), PluginSource::Installed);
    assert_eq!(
        plugin.dir(),
        Some(fixture.plugins_dir().join("acme.icons").as_path())
    );

    let themes: Vec<_> = registry.icon_themes().collect();
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].0.id(), "acme.icons");
    assert_eq!(themes[0].1.pack, PathBuf::from("pack.toml"));
    assert_eq!(registry.commands().count(), 0);
}

#[test]
fn a_color_theme_contribution_is_readable_from_the_registry() {
    let fixture = Fixture::new();
    fixture.install(
        "acme.themes",
        r#"
        id = "acme.themes"
        name = "Acme Themes"
        version = "1.0.0"
        api_version = 1

        [[contributes.color-themes]]
        id = "midnight"
        label = "Midnight"
        path = "midnight.toml"
        "#,
    );

    let registry = fixture.load(&[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let themes: Vec<_> = registry.color_themes().collect();
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].0.id(), "acme.themes");
    assert_eq!(themes[0].1.id, "midnight");
    assert_eq!(themes[0].1.path, PathBuf::from("midnight.toml"));
}

#[test]
fn a_missing_plugins_directory_is_not_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let registry = load(dir.path(), &[], &[]);
    assert!(registry.plugins().is_empty());
    assert!(registry.errors().is_empty());
}

/// The whole point of fail-soft: the broken one is reported and every
/// other plugin is untouched by it.
#[test]
fn a_broken_manifest_is_skipped_and_the_rest_still_load() {
    let fixture = Fixture::new();
    fixture.install("good.one", &manifest_for("good.one"));
    fixture.install("broken", "id = \"broken\"\nthis is not toml");
    fixture.install("good.two", &manifest_for("good.two"));

    let registry = fixture.load(&[]);
    assert_eq!(registry.plugins().len(), 2);
    assert!(registry.by_id("good.one").is_some());
    assert!(registry.by_id("good.two").is_some());

    assert_eq!(registry.errors().len(), 1);
    let error = &registry.errors()[0];
    assert_eq!(error.id, "broken");
    assert!(
        matches!(error.kind, LoadErrorKind::MalformedManifest(_)),
        "{error}"
    );
}

#[test]
fn a_directory_without_a_manifest_is_unreadable_rather_than_ignored() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.plugins_dir().join("empty")).expect("dir");

    let registry = fixture.load(&[]);
    assert!(registry.plugins().is_empty());
    assert!(
        matches!(registry.errors()[0].kind, LoadErrorKind::Unreadable { .. }),
        "{}",
        registry.errors()[0]
    );
}

#[test]
fn a_newer_api_version_is_refused_as_a_typed_error() {
    let fixture = Fixture::new();
    let newer = manifest_for("from.the.future").replace("api_version = 1", "api_version = 99");
    fixture.install("from.the.future", &newer);

    let registry = fixture.load(&[]);
    assert!(registry.plugins().is_empty());
    assert_eq!(
        registry.errors()[0].kind,
        LoadErrorKind::UnsupportedApiVersion(99)
    );
}

#[test]
fn a_disabled_plugin_is_filtered_rather_than_failed() {
    let fixture = Fixture::new();
    fixture.install("acme.icons", &manifest_for("acme.icons"));
    fixture.install("other", &manifest_for("other"));

    let registry = fixture.load(&["acme.icons".to_string()]);
    assert!(registry.by_id("acme.icons").is_none());
    assert!(registry.by_id("other").is_some());
    assert!(
        registry.errors().is_empty(),
        "the user's own choice must not be reported as a problem: {:?}",
        registry.errors()
    );
}

/// Disabling is keyed on the directory name so that it works even when the
/// manifest cannot be read — otherwise a plugin that is broken *and*
/// disabled would keep an error row nobody can clear.
#[test]
fn disabling_a_broken_plugin_silences_it_completely() {
    let fixture = Fixture::new();
    fixture.install("broken", "not a manifest at all");

    let registry = fixture.load(&["broken".to_string()]);
    assert!(registry.plugins().is_empty());
    assert!(registry.errors().is_empty());
}

#[test]
fn a_disabled_builtin_is_filtered_too() {
    let fixture = Fixture::new();
    let registry = load(
        fixture.config_dir(),
        &[BUILTIN],
        &["material-icons".to_string()],
    );
    assert!(registry.plugins().is_empty());
    assert!(registry.errors().is_empty());
}

#[test]
fn a_dot_directory_is_never_read_as_a_plugin() {
    let fixture = Fixture::new();
    fixture.install(".quarantine", "not a manifest");
    fixture.install(".hidden", "not a manifest");
    fixture.install("real", &manifest_for("real"));

    let registry = fixture.load(&[]);
    assert_eq!(registry.plugins().len(), 1);
    assert!(registry.errors().is_empty());
}

#[test]
fn a_quarantine_marker_disables_the_plugin_it_names() {
    let fixture = Fixture::new();
    fixture.install("crashy", &manifest_for("crashy"));
    fixture.install("innocent", &manifest_for("innocent"));
    fixture.quarantine("crashy");

    let registry = fixture.load(&[]);
    assert!(registry.by_id("crashy").is_none());
    assert!(registry.by_id("innocent").is_some());
    let marker = fixture.plugins_dir().join(QUARANTINE_DIR).join("crashy");
    assert_eq!(
        registry.errors()[0].kind,
        LoadErrorKind::Quarantined { marker }
    );
}

#[test]
fn a_quarantine_marker_disables_a_builtin_as_well() {
    let fixture = Fixture::new();
    fixture.quarantine("material-icons");
    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    assert!(registry.plugins().is_empty());
    assert!(matches!(
        registry.errors()[0].kind,
        LoadErrorKind::Quarantined { .. }
    ));
}

/// The id is what the disabled list, the settings page and the quarantine
/// marker key on, so a manifest that claims an id other than its own
/// directory name is refused rather than quietly renamed.
#[test]
fn an_id_that_disagrees_with_its_directory_is_refused() {
    let fixture = Fixture::new();
    fixture.install("on.disk", &manifest_for("in.manifest"));

    let registry = fixture.load(&[]);
    assert!(registry.plugins().is_empty());
    let error = &registry.errors()[0];
    assert_eq!(error.id, "on.disk");
    assert!(
        matches!(error.kind, LoadErrorKind::MalformedManifest(_)),
        "{error}"
    );
}

#[test]
fn a_builtin_loads_through_the_same_path_as_an_installed_plugin() {
    let fixture = Fixture::new();
    fixture.install("acme.icons", &manifest_for("acme.icons"));

    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());
    assert_eq!(registry.plugins().len(), 2);

    let builtin = registry.by_id("material-icons").expect("built-in loaded");
    assert_eq!(builtin.source(), PluginSource::Builtin);
    assert_eq!(builtin.dir(), None, "a built-in has no directory on disk");
    assert_eq!(
        registry.icon_themes().count(),
        2,
        "both sources contribute through the same accessor"
    );
}

/// Shadowing is how a user replaces a bundled plugin without a new build
/// of the editor, so the installed copy has to win — and the built-in it
/// displaced is recorded rather than vanishing.
#[test]
fn an_installed_plugin_shadows_a_builtin_of_the_same_id() {
    let fixture = Fixture::new();
    fixture.install("material-icons", &manifest_for("material-icons"));

    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    let winner = registry
        .by_id("material-icons")
        .expect("one of them loaded");
    assert_eq!(winner.source(), PluginSource::Installed);
    assert_eq!(registry.plugins().len(), 1);
    assert_eq!(registry.errors()[0].kind, LoadErrorKind::DuplicateId);
    assert_eq!(registry.errors()[0].id, "material-icons");
}

#[test]
fn an_asset_is_read_the_same_way_from_disk_and_from_the_binary() {
    let fixture = Fixture::new();
    let dir = fixture.install("acme.icons", &manifest_for("acme.icons"));
    fs::write(dir.join("pack.toml"), b"default = \"file\"\n").expect("pack");

    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    for id in ["acme.icons", "material-icons"] {
        let plugin = registry.by_id(id).expect("loaded");
        let bytes = plugin
            .read_asset(Path::new("pack.toml"))
            .expect("the pack reads");
        assert_eq!(&*bytes, b"default = \"file\"\n", "{id}");
    }
}

#[test]
fn an_asset_path_that_climbs_out_of_the_plugin_directory_is_refused() {
    let fixture = Fixture::new();
    let dir = fixture.install("acme.icons", &manifest_for("acme.icons"));
    fs::write(dir.parent().expect("plugins root").join("secret"), b"nope").expect("secret");

    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    for id in ["acme.icons", "material-icons"] {
        let plugin = registry.by_id(id).expect("loaded");
        for bad in ["../secret", "/etc/passwd", ""] {
            let err = plugin
                .read_asset(Path::new(bad))
                .expect_err(&format!("`{bad}` was read for {id}"));
            assert!(matches!(err, LoadErrorKind::UnsafePath { .. }), "{err}");
        }
    }
}

/// A path with no `..` in it can still leave the plugin directory by
/// pointing at a symlink, which only the filesystem can tell us.
#[cfg(unix)]
#[test]
fn an_asset_reached_through_a_symlink_out_of_the_directory_is_refused() {
    let fixture = Fixture::new();
    let dir = fixture.install("acme.icons", &manifest_for("acme.icons"));
    let secret = fixture.config_dir().join("secret");
    fs::write(&secret, b"nope").expect("secret");
    std::os::unix::fs::symlink(&secret, dir.join("pack.toml")).expect("symlink");

    let registry = fixture.load(&[]);
    let plugin = registry.by_id("acme.icons").expect("loaded");
    let err = plugin
        .read_asset(Path::new("pack.toml"))
        .expect_err("a symlink out of the directory was followed");
    assert!(matches!(err, LoadErrorKind::UnsafePath { .. }), "{err}");
}

#[test]
fn a_missing_asset_reports_which_file_was_missing() {
    let fixture = Fixture::new();
    fixture.install("acme.icons", &manifest_for("acme.icons"));

    let registry = load(fixture.config_dir(), &[BUILTIN], &[]);
    for id in ["acme.icons", "material-icons"] {
        let err = registry
            .by_id(id)
            .expect("loaded")
            .read_asset(Path::new("absent.toml"))
            .expect_err("a file that is not there");
        match err {
            LoadErrorKind::Unreadable { file, .. } => assert!(file.contains("absent.toml")),
            other => panic!("{other}"),
        }
    }
}

/// The reason the registry is an `Arc` behind the lock: a consumer that
/// took a snapshot keeps using it across a reload, with no lock held and
/// no half-swapped state.
#[test]
fn reload_swaps_the_registry_while_an_older_snapshot_stays_usable() {
    let fixture = Fixture::new();
    fixture.install("first", &manifest_for("first"));
    reload(fixture.config_dir(), &[]);

    let before = registry();
    assert!(before.by_id("first").is_some());

    fixture.install("second", &manifest_for("second"));
    let errors = reload(fixture.config_dir(), &[]);
    assert!(errors.is_empty(), "{errors:?}");

    let after = registry();
    assert!(after.by_id("second").is_some());
    assert!(
        before.by_id("second").is_none(),
        "the old snapshot must not change under its holder"
    );
    assert_eq!(
        before.by_id("first").expect("still there").id(),
        "first",
        "the pre-swap snapshot is still usable"
    );
}

#[test]
fn the_markdown_preview_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::MARKDOWN_PREVIEW], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry
        .by_id("markdown-preview")
        .expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    // Two contributions from the one built-in: Markdown documents, and
    // standalone Mermaid files, which are a different renderer behind the
    // same plugin (ADR-0043).
    let previews: Vec<_> = registry.previews().collect();
    assert_eq!(previews.len(), 2);
    for (owner, _) in &previews {
        assert_eq!(owner.id(), "markdown-preview");
    }
    assert_eq!(previews[0].1.id, "markdown");
    assert_eq!(
        previews[0].1.extensions,
        vec!["md", "markdown", "mdown", "mkd"]
    );
    assert_eq!(previews[1].1.id, "mermaid");
    assert_eq!(previews[1].1.extensions, vec!["mermaid", "mmd"]);
}

#[test]
fn the_csharp_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::CSHARP], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry.by_id("csharp").expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let servers: Vec<_> = registry.language_servers().collect();
    assert_eq!(servers.len(), 1);
    let (owner, contribution) = servers[0];
    assert_eq!(owner.id(), "csharp");
    assert_eq!(contribution.id, "csharp-ls");
    assert_eq!(contribution.language_id, "csharp");
    assert_eq!(contribution.command, "csharp-ls");
    assert_eq!(contribution.args, vec!["--loglevel", "warning"]);
    assert_eq!(contribution.settings_section.as_deref(), Some("csharp"));
    assert_eq!(
        contribution
            .settings
            .get("analyzersEnabled")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
}

#[test]
fn the_carve_builtin_loads_preview_and_language_server_contributions() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::CARVE], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry.by_id("carve").expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let previews: Vec<_> = registry.previews().collect();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].1.id, "carve");
    assert_eq!(previews[0].1.extensions, vec!["crv", "carve"]);

    let servers: Vec<_> = registry.language_servers().collect();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].1.language_id, "carve");
    assert_eq!(servers[0].1.command, "carve-lsp");
}

#[test]
fn a_language_server_contribution_needs_no_wasm_component_either() {
    // Same asymmetry as `previews` (M1 vs the built-in Markdown preview):
    // a native process launched by `command`/`args` is not
    // `CommandsWithoutComponent`.
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::CSHARP], &[]);
    let plugin = registry.by_id("csharp").expect("loaded");
    assert!(plugin.manifest().wasm.is_none());
}

#[test]
fn the_csharp_builtin_survives_the_disabled_plugin_filter() {
    let fixture = Fixture::new();
    let registry = load(
        fixture.config_dir(),
        &[builtins::CSHARP],
        &["material-icons".to_string()],
    );
    assert!(registry.by_id("csharp").is_some());
    assert_eq!(registry.language_servers().count(), 1);
}

#[test]
fn a_disabled_csharp_builtin_is_filtered_like_any_other() {
    let fixture = Fixture::new();
    let registry = load(
        fixture.config_dir(),
        &[builtins::CSHARP],
        &["csharp".to_string()],
    );
    assert!(registry.by_id("csharp").is_none());
    assert_eq!(registry.language_servers().count(), 0);
    assert!(registry.errors().is_empty());
}

#[test]
fn the_php_tools_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::PHP_TOOLS], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry.by_id("php-tools").expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let analyzers: Vec<_> = registry.analyzers().collect();
    assert_eq!(analyzers.len(), 2, "{analyzers:?}");
    for (owner, _) in &analyzers {
        assert_eq!(owner.id(), "php-tools");
    }

    let phpstan = analyzers
        .iter()
        .find(|(_, a)| a.id == "phpstan")
        .expect("phpstan contributed")
        .1;
    assert_eq!(phpstan.name, "PHPStan");
    assert_eq!(
        phpstan.program_candidates,
        vec!["vendor/bin/phpstan", "phpstan.phar", "phpstan"]
    );
    assert_eq!(
        phpstan.args,
        vec!["analyse", "--error-format=checkstyle", "--no-progress"]
    );
    assert_eq!(phpstan.output_format, "checkstyle-xml");
    assert_eq!(
        phpstan.severity_map.get("error").map(String::as_str),
        Some("error")
    );

    let phpcs = analyzers
        .iter()
        .find(|(_, a)| a.id == "phpcs")
        .expect("phpcs contributed")
        .1;
    assert_eq!(phpcs.name, "PHP_CodeSniffer");
    assert_eq!(
        phpcs.program_candidates,
        vec!["vendor/bin/phpcs", "phpcs.phar", "phpcs"]
    );
    assert_eq!(phpcs.args, vec!["--report=checkstyle"]);
    assert_eq!(phpcs.output_format, "checkstyle-xml");
    assert_eq!(
        phpcs.severity_map.get("warning").map(String::as_str),
        Some("warning")
    );

    let frameworks: Vec<_> = registry.test_frameworks().collect();
    assert_eq!(frameworks.len(), 1, "{frameworks:?}");
    let (owner, phpunit) = &frameworks[0];
    assert_eq!(owner.id(), "php-tools");
    assert_eq!(phpunit.id, "phpunit");
    assert_eq!(phpunit.name, "PHPUnit");
    assert_eq!(
        phpunit.program_candidates,
        vec!["vendor/bin/phpunit", "phpunit.phar", "phpunit"]
    );
    assert_eq!(phpunit.args, vec!["--teamcity"]);
    assert_eq!(phpunit.filter_flag.as_deref(), Some("--filter"));
    assert_eq!(phpunit.output_format, "teamcity");
    assert_eq!(
        phpunit.config_file_candidates,
        vec!["phpunit.xml", "phpunit.xml.dist"]
    );
}

#[test]
fn an_analyzer_contribution_needs_no_wasm_component_either() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::PHP_TOOLS], &[]);
    let plugin = registry.by_id("php-tools").expect("loaded");
    assert!(plugin.manifest().wasm.is_none());
}

#[test]
fn a_disabled_php_tools_builtin_is_filtered_like_any_other() {
    let fixture = Fixture::new();
    let registry = load(
        fixture.config_dir(),
        &[builtins::PHP_TOOLS],
        &["php-tools".to_string()],
    );
    assert!(registry.by_id("php-tools").is_none());
    assert_eq!(registry.analyzers().count(), 0);
    assert!(registry.errors().is_empty());
}

#[test]
fn previews_without_a_component_load_and_need_no_wasm_tier() {
    // The whole point of the asymmetry with `commands` (M1): a `previews`
    // contribution with no `[wasm]` section is not `CommandsWithoutComponent`
    // — it is served by the host's own native renderer table, which this
    // crate does not know about (`app-core`'s join does).
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::MARKDOWN_PREVIEW], &[]);
    let plugin = registry.by_id("markdown-preview").expect("loaded");
    assert!(plugin.manifest().wasm.is_none());
}

#[test]
fn analyzers_are_listed_with_the_plugin_that_offers_them() {
    let fixture = Fixture::new();
    let manifest = r#"
        id = "php-tools"
        name = "PHP Tools"
        version = "1.0.0"
        api_version = 1

        [[contributes.analyzers]]
        id = "phpstan"
        name = "PHPStan"
        program-candidates = ["vendor/bin/phpstan", "phpstan"]
        output-format = "checkstyle-xml"
        "#;
    fixture.install("php-tools", manifest);
    let registry = fixture.load(&[]);

    let analyzers: Vec<_> = registry.analyzers().collect();
    assert_eq!(analyzers.len(), 1);
    assert_eq!(analyzers[0].0.id(), "php-tools");
    assert_eq!(analyzers[0].1.id, "phpstan");
    assert_eq!(analyzers[0].1.output_format, "checkstyle-xml");
}

#[test]
fn the_jvm_build_tools_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::JVM_BUILD_TOOLS], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry
        .by_id("jvm-build-tools")
        .expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let tools: Vec<_> = registry.build_tools().collect();
    assert_eq!(tools.len(), 2, "{tools:?}");
    let gradle = tools
        .iter()
        .find(|(_, t)| t.id == "gradle")
        .expect("gradle contributed")
        .1;
    assert_eq!(gradle.toolchain, "gradle");
    assert_eq!(
        gradle.init_script.as_deref(),
        Some(Path::new("ide-model.init.gradle"))
    );
    let maven = tools
        .iter()
        .find(|(_, t)| t.id == "maven")
        .expect("maven contributed")
        .1;
    assert_eq!(maven.init_script, None);

    let frameworks: Vec<_> = registry.test_frameworks().collect();
    assert_eq!(frameworks.len(), 2, "{frameworks:?}");
}

/// Every path a `jvm-build-tools` contribution names — an `init-script`, or
/// an `${asset_dir}/<file>` token inside a `test-frameworks` row's `args` —
/// must actually be one of this plugin's embedded `files`, or the plugin
/// would validate cleanly and then fail the very first sync/test run that
/// tries to read the file (A3's own requirement).
#[test]
fn every_asset_the_jvm_build_tools_manifest_names_exists_in_its_files() {
    let manifest =
        PluginManifest::from_toml_str(builtins::JVM_BUILD_TOOLS.manifest).expect("valid");
    let file_names: Vec<&str> = builtins::JVM_BUILD_TOOLS
        .files
        .iter()
        .map(|(name, _)| *name)
        .collect();

    for tool in &manifest.contributes.build_tools {
        if let Some(script) = &tool.init_script {
            let name = script.to_str().expect("utf-8 path");
            assert!(
                file_names.contains(&name),
                "build-tools.{} names init-script `{name}`, which is not in `files`",
                tool.id
            );
        }
    }

    const TOKEN: &str = "${asset_dir}/";
    for framework in &manifest.contributes.test_frameworks {
        for arg in &framework.args {
            if let Some(name) = arg.strip_prefix(TOKEN) {
                assert!(
                    file_names.contains(&name),
                    "test-frameworks.{} names asset `{name}` in args, which is not in `files`",
                    framework.id
                );
            }
        }
    }
}

#[test]
fn the_jvm_build_tools_init_script_is_materialised_on_demand() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::JVM_BUILD_TOOLS], &[]);
    let plugin = registry.by_id("jvm-build-tools").expect("loaded");
    let dir = plugin
        .asset_dir(fixture.config_dir())
        .expect("materialises");
    let script = dir.join("ide-model.init.gradle");
    assert!(script.is_file(), "{}", script.display());
    let contents = fs::read_to_string(&script).expect("readable");
    assert!(contents.contains("ideModel"));
    assert!(contents.contains("IdeTeamCityListener"));
}

#[test]
fn the_database_tools_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::DATABASE_TOOLS], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry
        .by_id("database-tools")
        .expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let drivers: Vec<_> = registry.database_drivers().collect();
    assert_eq!(drivers.len(), 5, "{drivers:?}");
    let sqlite = drivers
        .iter()
        .find(|(_, d)| d.id == "sqlite")
        .expect("sqlite contributed")
        .1;
    assert_eq!(sqlite.native_id.as_deref(), Some("sqlite"));
    let postgresql = drivers
        .iter()
        .find(|(_, d)| d.id == "postgresql")
        .expect("postgresql contributed")
        .1;
    assert_eq!(postgresql.default_port, Some(5432));
    let mongodb = drivers
        .iter()
        .find(|(_, d)| d.id == "mongodb")
        .expect("mongodb contributed")
        .1;
    assert_eq!(mongodb.default_port, Some(27017));
    let redis = drivers
        .iter()
        .find(|(_, d)| d.id == "redis")
        .expect("redis contributed")
        .1;
    assert_eq!(redis.default_port, Some(6379));
    let cassandra = drivers
        .iter()
        .find(|(_, d)| d.id == "cassandra")
        .expect("cassandra contributed")
        .1;
    assert_eq!(cassandra.default_port, Some(9042));

    let dialects: Vec<_> = registry.sql_dialects().collect();
    assert_eq!(dialects.len(), 3, "{dialects:?}");
}

/// Every `keywords` path a `database-tools` `sql-dialects` row names must
/// actually be one of this plugin's embedded `files`, the same "would
/// validate cleanly and then fail the first real use" requirement
/// `every_asset_the_jvm_build_tools_manifest_names_exists_in_its_files`
/// already states for that plugin's own assets.
#[test]
fn every_asset_the_database_tools_manifest_names_exists_in_its_files() {
    let manifest = PluginManifest::from_toml_str(builtins::DATABASE_TOOLS.manifest).expect("valid");
    let file_names: Vec<&str> = builtins::DATABASE_TOOLS
        .files
        .iter()
        .map(|(name, _)| *name)
        .collect();

    for dialect in &manifest.contributes.sql_dialects {
        if let Some(keywords) = &dialect.keywords {
            let name = keywords.to_str().expect("utf-8 path");
            assert!(
                file_names.contains(&name),
                "sql-dialects.{} names keywords `{name}`, which is not in `files`",
                dialect.id
            );
        }
    }
}

#[test]
fn a_tool_window_and_settings_page_contribution_are_readable_from_the_registry() {
    let fixture = Fixture::new();
    fixture.install(
        "acme.tools",
        r#"
        id = "acme.tools"
        name = "Acme Tools"
        version = "1.0.0"
        api_version = 1

        [[contributes.tool-windows]]
        id = "acmeTree"
        title = "Acme Tree"
        area = "left"

        [[contributes.settings-pages]]
        id = "acmeSettings"
        title = "Acme"
        scope = "global"
        "#,
    );

    let registry = fixture.load(&[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let windows: Vec<_> = registry.tool_windows().collect();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].0.id(), "acme.tools");
    assert_eq!(windows[0].1.id, "acmeTree");
    assert_eq!(windows[0].1.area, plugin_api::ToolWindowArea::Left);

    let pages: Vec<_> = registry.settings_pages().collect();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].1.id, "acmeSettings");
    assert_eq!(pages[0].1.scope, plugin_api::SettingsPageScope::Global);
}

/// The rule `PluginRegistry::claim` already enforces for a plugin id
/// collision, extended to a dock/settings-page id: first claim wins, and
/// the second plugin — the whole plugin, since a manifest cannot drop one
/// row from itself — is rejected with an error row rather than silently
/// hiding the first one's dock.
#[test]
fn two_plugins_racing_for_one_tool_window_id_rejects_the_second() {
    let fixture = Fixture::new();
    fixture.install(
        "first",
        r#"
        id = "first"
        name = "First"
        version = "1.0.0"
        api_version = 1

        [[contributes.tool-windows]]
        id = "database"
        title = "Database"
        area = "right"
        "#,
    );
    fixture.install(
        "second",
        r#"
        id = "second"
        name = "Second"
        version = "1.0.0"
        api_version = 1

        [[contributes.tool-windows]]
        id = "database"
        title = "Database, again"
        area = "bottom"
        "#,
    );

    let registry = fixture.load(&[]);
    assert_eq!(registry.plugins().len(), 1);
    assert!(registry.by_id("first").is_some());
    assert!(registry.by_id("second").is_none());
    assert_eq!(registry.tool_windows().count(), 1);

    let error = &registry.errors()[0];
    assert_eq!(error.id, "second");
    assert_eq!(
        error.kind,
        LoadErrorKind::DuplicateContributionId {
            point: "tool-windows",
            id: "database".to_string(),
        }
    );
}

#[test]
fn two_plugins_racing_for_one_settings_page_id_rejects_the_second() {
    let fixture = Fixture::new();
    fixture.install(
        "first",
        r#"
        id = "first"
        name = "First"
        version = "1.0.0"
        api_version = 1

        [[contributes.settings-pages]]
        id = "shared"
        title = "Shared"
        scope = "global"
        "#,
    );
    fixture.install(
        "second",
        r#"
        id = "second"
        name = "Second"
        version = "1.0.0"
        api_version = 1

        [[contributes.settings-pages]]
        id = "shared"
        title = "Shared, again"
        scope = "project"
        "#,
    );

    let registry = fixture.load(&[]);
    assert_eq!(registry.plugins().len(), 1);
    assert!(registry.by_id("first").is_some());
    assert_eq!(
        registry.errors()[0].kind,
        LoadErrorKind::DuplicateContributionId {
            point: "settings-pages",
            id: "shared".to_string(),
        }
    );
}

#[test]
fn the_containers_builtin_loads_through_the_real_path() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::CONTAINERS], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let plugin = registry.by_id("containers").expect("the built-in loaded");
    assert_eq!(plugin.source(), PluginSource::Builtin);

    let windows: Vec<_> = registry
        .tool_windows()
        .filter(|(owner, _)| owner.id() == "containers")
        .collect();
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0].1.id, "containers");

    let pages: Vec<_> = registry
        .settings_pages()
        .filter(|(owner, _)| owner.id() == "containers")
        .collect();
    assert_eq!(pages.len(), 1, "{pages:?}");
    assert_eq!(pages[0].1.id, "containers");
}

#[test]
fn a_disabled_containers_builtin_is_filtered_like_any_other() {
    let fixture = Fixture::new();
    let registry = load(
        fixture.config_dir(),
        &[builtins::CONTAINERS],
        &["containers".to_string()],
    );
    assert!(registry.by_id("containers").is_none());
    assert!(registry.errors().is_empty());
}

#[test]
fn the_jvm_build_tools_builtin_contributes_its_tool_window_and_settings_page() {
    let fixture = Fixture::new();
    let registry = load(fixture.config_dir(), &[builtins::JVM_BUILD_TOOLS], &[]);
    assert!(registry.errors().is_empty(), "{:?}", registry.errors());

    let windows: Vec<_> = registry.tool_windows().collect();
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0].1.id, "buildTools");
    assert_eq!(windows[0].1.area, plugin_api::ToolWindowArea::Right);

    let pages: Vec<_> = registry.settings_pages().collect();
    assert_eq!(pages.len(), 1, "{pages:?}");
    assert_eq!(pages[0].1.id, "buildTools");
    assert_eq!(pages[0].1.scope, plugin_api::SettingsPageScope::Project);
}
