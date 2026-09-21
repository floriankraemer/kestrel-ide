//! Unit tests for [`super::PluginManifest`] and its contribution types.
//! Split out of `mod.rs` to keep that file under the file-size ceiling as new
//! contribution points are added (A1, jvm-build-tools plan).
use super::*;

const MINIMAL: &str = r#"
        id = "material-icons"
        name = "Material Icon Theme"
        version = "5.38.1"
        api_version = 1
    "#;

pub(super) fn with(extra: &str) -> String {
    format!("{MINIMAL}\n{extra}")
}

#[test]
fn a_minimal_manifest_parses() {
    let manifest = PluginManifest::from_toml_str(MINIMAL).expect("valid");
    assert_eq!(manifest.id, "material-icons");
    assert!(manifest.contributes.is_empty());
    assert!(manifest.wasm.is_none());
    assert!(manifest.capabilities.read_files.is_empty());
    assert!(!manifest.capabilities.notify);
}

#[test]
fn an_icon_theme_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"
            "#,
    ))
    .expect("valid");
    let themes = &manifest.contributes.icon_themes;
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].id, "material");
    assert_eq!(themes[0].label, "Material");
    assert_eq!(themes[0].pack, PathBuf::from("pack.toml"));
}

#[test]
fn a_color_theme_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight"
            path = "midnight.toml"
            "#,
    ))
    .expect("valid");
    let themes = &manifest.contributes.color_themes;
    assert_eq!(themes.len(), 1);
    assert_eq!(themes[0].id, "midnight");
    assert_eq!(themes[0].label, "Midnight");
    assert_eq!(themes[0].path, PathBuf::from("midnight.toml"));
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::ColorThemes.key(), "color-themes");
}

#[test]
fn two_color_theme_contributions_may_not_claim_one_id() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight"
            path = "a.toml"

            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight, again"
            path = "b.toml"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "color-themes",
            id: "midnight".to_string(),
        }
    );
}

#[test]
fn a_typo_in_a_key_is_refused_rather_than_ignored() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            lable = "Material"
            pack = "pack.toml"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_newer_contract_is_refused_outright() {
    let text = MINIMAL.replace("api_version = 1", "api_version = 2");
    assert_eq!(
        PluginManifest::from_toml_str(&text).unwrap_err(),
        LoadErrorKind::UnsupportedApiVersion(2)
    );
    let text = MINIMAL.replace("api_version = 1", "api_version = 0");
    assert_eq!(
        PluginManifest::from_toml_str(&text).unwrap_err(),
        LoadErrorKind::UnsupportedApiVersion(0)
    );
}

#[test]
fn an_id_that_could_climb_out_of_the_plugins_directory_is_refused() {
    for bad in ["../evil", "/etc", "Material", "café", "", &"x".repeat(65)] {
        let text = MINIMAL.replace("material-icons", bad);
        let err = PluginManifest::from_toml_str(&text).unwrap_err();
        assert!(
            matches!(err, LoadErrorKind::MalformedId { field: "id", .. }),
            "`{bad}` was accepted as an id: {err}"
        );
    }
}

#[test]
fn an_id_may_be_dotted_lowercase() {
    let text = MINIMAL.replace("material-icons", "org.example.plugin_2");
    assert!(PluginManifest::from_toml_str(&text).is_ok());
}

#[test]
fn a_pack_path_may_not_escape_the_plugin_directory() {
    for bad in ["/etc/passwd", "../../pack.toml", "themes/../../pack.toml"] {
        let err = PluginManifest::from_toml_str(&with(&format!(
            r#"
                [[contributes.icon-themes]]
                id = "material"
                label = "Material"
                pack = "{bad}"
                "#
        )))
        .unwrap_err();
        assert!(
            matches!(err, LoadErrorKind::UnsafePath { .. }),
            "`{bad}` was accepted as a pack path: {err}"
        );
    }
}

#[test]
fn a_nested_pack_path_is_fine() {
    assert!(PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "themes/material/pack.toml"
            "#,
    ))
    .is_ok());
}

#[test]
fn two_contributions_may_not_claim_one_id() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "a.toml"

            [[contributes.icon-themes]]
            id = "material"
            label = "Material Light"
            pack = "b.toml"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "icon-themes",
            id: "material".to_string(),
        }
    );
}

#[test]
fn commands_need_a_component_to_run_them() {
    let commands = r#"
            [[contributes.commands]]
            id = "example.hello"
            title = "Example: Hello"
        "#;
    assert_eq!(
        PluginManifest::from_toml_str(&with(commands)).unwrap_err(),
        LoadErrorKind::CommandsWithoutComponent
    );

    let manifest = PluginManifest::from_toml_str(&with(&format!(
        "{commands}\n[wasm]\ncomponent = \"plugin.wasm\"\n"
    )))
    .expect("valid");
    assert_eq!(
        manifest.component_path(),
        Some(Path::new("plugin.wasm")),
        "the component path is what the host opens"
    );
}

#[test]
fn a_capability_path_must_be_scoped_to_the_plugin_directory() {
    for bad in ["/etc", "${workspace_root}/secrets", "../elsewhere"] {
        let err = PluginManifest::from_toml_str(&with(&format!(
            "[capabilities]\nread-files = [\"{bad}\"]\n"
        )))
        .unwrap_err();
        assert_eq!(err, LoadErrorKind::UnscopedCapabilityPath(bad.to_string()));
    }
}

#[test]
fn a_capability_path_climbing_out_through_the_token_is_refused() {
    let bad = "${plugin_dir}/../../etc";
    let err = PluginManifest::from_toml_str(&with(&format!(
        "[capabilities]\nread-files = [\"{bad}\"]\n"
    )))
    .unwrap_err();
    assert_eq!(err, LoadErrorKind::UnscopedCapabilityPath(bad.to_string()));
}

#[test]
fn a_scoped_capability_path_expands_under_the_plugin_directory() {
    let manifest = PluginManifest::from_toml_str(&with(
        "[capabilities]\nread-files = [\"${plugin_dir}/data\"]\nnotify = true\n",
    ))
    .expect("valid");
    assert!(manifest.capabilities.notify);
    let dir = Path::new("/home/u/.config/ide/plugins/material-icons");
    assert_eq!(
        expand_capability_path(&manifest.capabilities.read_files[0], dir),
        dir.join("data")
    );
    assert_eq!(expand_capability_path("${plugin_dir}", dir), dir);
}

#[test]
fn contribution_point_keys_match_the_manifest_keys() {
    // The enum is what the host looks contributions up by; the strings
    // are what a plugin author writes. They must not drift.
    assert_eq!(ContributionPoint::IconThemes.key(), "icon-themes");
    assert_eq!(ContributionPoint::Commands.key(), "commands");
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"
            "#,
    ))
    .expect("valid");
    assert!(!manifest.contributes.is_empty());
}

#[test]
fn a_previews_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md", "markdown"]
            "#,
    ))
    .expect("valid");
    let previews = &manifest.contributes.previews;
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].id, "markdown");
    assert_eq!(previews[0].label, "Markdown");
    assert_eq!(previews[0].extensions, vec!["md", "markdown"]);
    assert!(!manifest.contributes.is_empty());
}

#[test]
fn a_previews_contribution_needs_at_least_one_extension() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = []
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.previews.extensions")
    );
}

#[test]
fn an_extension_with_a_dot_or_a_separator_or_uppercase_is_rejected() {
    // A backslash is not exercised here: TOML's own string escaping
    // rejects a bare `\` before this code ever sees it, so that case is
    // already covered by `a_typo_in_a_key_is_refused_rather_than_ignored`'s
    // sibling, `MalformedManifest`, not `InvalidExtension`.
    for bad in [".md", "md/x", "MD", "m d", ""] {
        let err = PluginManifest::from_toml_str(&with(&format!(
            r#"
                [[contributes.previews]]
                id = "markdown"
                label = "Markdown"
                extensions = ["{bad}"]
                "#
        )))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::InvalidExtension(bad.to_string()),
            "`{bad}` should have been rejected"
        );
    }
}

#[test]
fn duplicate_preview_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md"]

            [[contributes.previews]]
            id = "markdown"
            label = "Markdown, again"
            extensions = ["mkd"]
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "previews",
            id: "markdown".to_string(),
        }
    );
}

#[test]
fn previews_without_a_component_are_accepted_unlike_commands() {
    // The asymmetry with `commands_need_a_component_to_run_them` is the
    // point: a `previews` contribution may be served entirely by the
    // host's own native renderer table.
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md"]
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn an_unknown_contribution_point_is_ignored_not_an_error() {
    // The property `API_VERSION`'s doc comment promises: a manifest
    // naming a point this build has never heard of still loads, with
    // everything else about it intact.
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"

            [[contributes.some-future-point]]
            id = "whatever"
            "#,
    ))
    .expect("an unrecognised point must not fail the whole manifest");
    assert_eq!(manifest.contributes.icon_themes.len(), 1);
}

#[test]
fn a_typo_in_a_known_previews_field_is_still_refused() {
    // `Contributes` dropped `deny_unknown_fields` so an *unrecognised
    // point* is tolerated; a typo inside a point this build does know
    // must still be a load error, or nobody would ever notice one.
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.previews]]
            id = "markdown"
            lable = "Markdown"
            extensions = ["md"]
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_language_server_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"
            args = ["--loglevel", "warning"]
            settings-section = "csharp"

            [contributes.language-servers.settings]
            analyzersEnabled = true
            "#,
    ))
    .expect("valid");
    let servers = &manifest.contributes.language_servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "csharp-ls");
    assert_eq!(servers[0].language_id, "csharp");
    assert_eq!(servers[0].name, "csharp-ls");
    assert_eq!(servers[0].command, "csharp-ls");
    assert_eq!(servers[0].args, vec!["--loglevel", "warning"]);
    assert_eq!(servers[0].settings_section.as_deref(), Some("csharp"));
    assert_eq!(
        servers[0].settings.get("analyzersEnabled"),
        Some(&toml::Value::Boolean(true))
    );
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::LanguageServers.key(), "language-servers");
}

#[test]
fn a_language_server_needs_no_wasm_component() {
    // Unlike `commands`, a language server is a native process the host
    // launches by `command`/`args` — the same shape `ServerDef` already
    // has for the built-in catalog table.
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn a_language_server_without_a_command_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = ""
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.language-servers.command")
    );
}

#[test]
fn duplicate_language_server_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"

            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls, again"
            command = "csharp-ls"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "language-servers",
            id: "csharp-ls".to_string(),
        }
    );
}

#[test]
fn an_analyzer_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["vendor/bin/phpstan", "phpstan"]
            args = ["analyse", "--error-format=checkstyle"]
            output-format = "checkstyle-xml"

            [contributes.analyzers.severity-map]
            error = "error"
            warning = "warning"
            "#,
    ))
    .expect("valid");
    let analyzers = &manifest.contributes.analyzers;
    assert_eq!(analyzers.len(), 1);
    assert_eq!(analyzers[0].id, "phpstan");
    assert_eq!(analyzers[0].name, "PHPStan");
    assert_eq!(
        analyzers[0].program_candidates,
        vec!["vendor/bin/phpstan", "phpstan"]
    );
    assert_eq!(
        analyzers[0].args,
        vec!["analyse", "--error-format=checkstyle"]
    );
    assert_eq!(analyzers[0].output_format, "checkstyle-xml");
    assert_eq!(
        analyzers[0].severity_map.get("error").map(String::as_str),
        Some("error")
    );
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::Analyzers.key(), "analyzers");
}

#[test]
fn an_analyzer_needs_no_wasm_component() {
    // Same reasoning as a language server: a native process launched
    // by argv needs no sandboxed guest to run it.
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn an_analyzer_needs_at_least_one_program_candidate() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = []
            output-format = "checkstyle-xml"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.analyzers.program-candidates")
    );
}

#[test]
fn an_analyzer_without_an_output_format_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = ""
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.analyzers.output-format")
    );
}

#[test]
fn duplicate_analyzer_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"

            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan, again"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "analyzers",
            id: "phpstan".to_string(),
        }
    );
}

#[test]
fn a_test_framework_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["vendor/bin/phpunit", "phpunit"]
            args = ["--teamcity"]
            filter-flag = "--filter"
            output-format = "teamcity"
            config-file-candidates = ["phpunit.xml", "phpunit.xml.dist"]
            "#,
    ))
    .expect("valid");
    let frameworks = &manifest.contributes.test_frameworks;
    assert_eq!(frameworks.len(), 1);
    assert_eq!(frameworks[0].id, "phpunit");
    assert_eq!(frameworks[0].name, "PHPUnit");
    assert_eq!(
        frameworks[0].program_candidates,
        vec!["vendor/bin/phpunit", "phpunit"]
    );
    assert_eq!(frameworks[0].args, vec!["--teamcity"]);
    assert_eq!(frameworks[0].filter_flag.as_deref(), Some("--filter"));
    assert_eq!(frameworks[0].filter_template, None);
    assert_eq!(frameworks[0].requires_toolchain, None);
    assert_eq!(frameworks[0].report_glob, None);
    assert_eq!(frameworks[0].filter_dialect, None);
    assert_eq!(frameworks[0].output_format, "teamcity");
    assert_eq!(
        frameworks[0].config_file_candidates,
        vec!["phpunit.xml", "phpunit.xml.dist"]
    );
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::TestFrameworks.key(), "test-frameworks");
}

#[test]
fn a_test_framework_needs_no_wasm_component() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn a_test_framework_needs_at_least_one_program_candidate() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = []
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.test-frameworks.program-candidates")
    );
}

#[test]
fn a_test_framework_needs_a_filter_flag() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = ""
            output-format = "teamcity"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.test-frameworks.filter-flag")
    );
}

#[test]
fn duplicate_test_framework_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"

            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit, again"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "test-frameworks",
            id: "phpunit".to_string(),
        }
    );
}

#[test]
fn a_test_framework_may_use_a_filter_template_instead_of_a_flag() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "junit-maven"
            name = "JUnit (Maven)"
            program-candidates = ["./mvnw", "mvn"]
            filter-template = "-Dtest={pattern}"
            output-format = "junit-xml"
            report-glob = "**/target/surefire-reports/TEST-*.xml"
            requires-toolchain = "maven"
            "#,
    ))
    .expect("valid");
    let framework = &manifest.contributes.test_frameworks[0];
    assert_eq!(framework.filter_flag, None);
    assert_eq!(
        framework.filter_template.as_deref(),
        Some("-Dtest={pattern}")
    );
    assert_eq!(
        framework.report_glob.as_deref(),
        Some("**/target/surefire-reports/TEST-*.xml")
    );
    assert_eq!(framework.requires_toolchain.as_deref(), Some("maven"));
}

#[test]
fn a_test_framework_may_declare_a_non_default_filter_dialect() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "junit-maven"
            name = "JUnit (Maven)"
            program-candidates = ["./mvnw", "mvn"]
            filter-template = "-Dtest={pattern}"
            output-format = "junit-xml"
            filter-dialect = "surefire"
            "#,
    ))
    .expect("valid");
    let framework = &manifest.contributes.test_frameworks[0];
    assert_eq!(framework.filter_dialect.as_deref(), Some("surefire"));
}

#[test]
fn an_unknown_filter_dialect_is_a_load_error_not_a_silent_fallback() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"
            filter-dialect = "some-future-dialect"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_test_framework_may_not_set_both_filter_flag_and_filter_template() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            filter-template = "--filter={pattern}"
            output-format = "teamcity"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_test_framework_must_set_a_filter_flag_or_a_filter_template() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            output-format = "teamcity"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_filter_template_must_contain_the_pattern_placeholder() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.test-frameworks]]
            id = "junit-maven"
            name = "JUnit (Maven)"
            program-candidates = ["mvn"]
            filter-template = "-Dtest=nothing-to-splice-in"
            output-format = "junit-xml"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_build_tool_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.build-tools]]
            id = "gradle"
            name = "Gradle"
            toolchain = "gradle"
            build-files = ["build.gradle", "build.gradle.kts", "settings.gradle.kts"]
            init-script = "ide-model.init.gradle"

            [[contributes.build-tools]]
            id = "maven"
            name = "Maven"
            toolchain = "maven"
            build-files = ["**/pom.xml", ".mvn/**"]
            "#,
    ))
    .expect("valid");
    let tools = &manifest.contributes.build_tools;
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].id, "gradle");
    assert_eq!(tools[0].toolchain, "gradle");
    assert_eq!(
        tools[0].build_files,
        vec!["build.gradle", "build.gradle.kts", "settings.gradle.kts"]
    );
    assert_eq!(
        tools[0].init_script,
        Some(PathBuf::from("ide-model.init.gradle"))
    );
    assert_eq!(tools[1].id, "maven");
    assert_eq!(tools[1].init_script, None);
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::BuildTools.key(), "build-tools");
}

#[test]
fn a_build_tool_needs_no_wasm_component() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.build-tools]]
            id = "maven"
            name = "Maven"
            toolchain = "maven"
            build-files = ["**/pom.xml"]
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn a_build_tool_needs_at_least_one_build_file_pattern() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.build-tools]]
            id = "maven"
            name = "Maven"
            toolchain = "maven"
            build-files = []
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.build-tools.build-files")
    );
}

#[test]
fn a_build_tool_init_script_may_not_escape_the_plugin_directory() {
    for bad in ["/etc/passwd", "../../ide-model.init.gradle"] {
        let err = PluginManifest::from_toml_str(&with(&format!(
            r#"
                [[contributes.build-tools]]
                id = "gradle"
                name = "Gradle"
                toolchain = "gradle"
                build-files = ["build.gradle"]
                init-script = "{bad}"
                "#
        )))
        .unwrap_err();
        assert!(
            matches!(err, LoadErrorKind::UnsafePath { .. }),
            "`{bad}` was accepted as an init-script path: {err}"
        );
    }
}

#[test]
fn duplicate_build_tool_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.build-tools]]
            id = "gradle"
            name = "Gradle"
            toolchain = "gradle"
            build-files = ["build.gradle"]

            [[contributes.build-tools]]
            id = "gradle"
            name = "Gradle, again"
            toolchain = "gradle"
            build-files = ["build.gradle.kts"]
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "build-tools",
            id: "gradle".to_string(),
        }
    );
}

#[test]
fn a_tool_window_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "buildTools"
            title = "Build Tools"
            area = "right"

            [[contributes.tool-windows]]
            id = "databaseResults"
            title = "Database Results"
            area = "bottom"
            "#,
    ))
    .expect("valid");
    let windows = &manifest.contributes.tool_windows;
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id, "buildTools");
    assert_eq!(windows[0].title, "Build Tools");
    assert_eq!(windows[0].area, ToolWindowArea::Right);
    assert_eq!(windows[1].area, ToolWindowArea::Bottom);
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::ToolWindows.key(), "tool-windows");
}

#[test]
fn a_tool_window_needs_no_wasm_component() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "containers"
            title = "Containers"
            area = "bottom"
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
}

#[test]
fn every_tool_window_area_parses() {
    for (area, expected) in [
        ("left", ToolWindowArea::Left),
        ("right", ToolWindowArea::Right),
        ("bottom", ToolWindowArea::Bottom),
        ("center", ToolWindowArea::Center),
    ] {
        let manifest = PluginManifest::from_toml_str(&with(&format!(
            r#"
                [[contributes.tool-windows]]
                id = "x"
                title = "X"
                area = "{area}"
                "#
        )))
        .expect("valid");
        assert_eq!(manifest.contributes.tool_windows[0].area, expected);
    }
}

#[test]
fn an_unknown_tool_window_area_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "x"
            title = "X"
            area = "top"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn a_tool_window_needs_a_non_empty_title() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "x"
            title = ""
            area = "left"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.tool-windows.title")
    );
}

#[test]
fn a_malformed_tool_window_id_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "Not Valid"
            title = "X"
            area = "left"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedId { .. }), "{err}");
}

#[test]
fn duplicate_tool_window_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.tool-windows]]
            id = "database"
            title = "Database"
            area = "right"

            [[contributes.tool-windows]]
            id = "database"
            title = "Database, again"
            area = "bottom"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "tool-windows",
            id: "database".to_string(),
        }
    );
}

#[test]
fn a_settings_page_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.settings-pages]]
            id = "buildTools"
            title = "Build Tools"
            scope = "project"

            [[contributes.settings-pages]]
            id = "aiProviders"
            title = "AI Providers"
            scope = "global"
            "#,
    ))
    .expect("valid");
    let pages = &manifest.contributes.settings_pages;
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].id, "buildTools");
    assert_eq!(pages[0].scope, SettingsPageScope::Project);
    assert_eq!(pages[1].scope, SettingsPageScope::Global);
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::SettingsPages.key(), "settings-pages");
}

#[test]
fn an_unknown_settings_page_scope_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.settings-pages]]
            id = "x"
            title = "X"
            scope = "workspace"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
}

#[test]
fn duplicate_settings_page_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.settings-pages]]
            id = "containers"
            title = "Containers"
            scope = "project"

            [[contributes.settings-pages]]
            id = "containers"
            title = "Containers, again"
            scope = "global"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "settings-pages",
            id: "containers".to_string(),
        }
    );
}

#[test]
fn a_settings_page_needs_a_non_empty_title() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.settings-pages]]
            id = "x"
            title = ""
            scope = "global"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::EmptyField("contributes.settings-pages.title")
    );
}
