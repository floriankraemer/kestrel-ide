//! The colour-theme join: plugins on one side, [`color_theme::ColorTheme`]
//! on the other.
//!
//! Same shape as [`crate::icons`] (ADR-0026): `plugin-host` stores
//! contribution payloads and never interprets one, `color-theme` parses a
//! theme file and never learns where it lives, and neither may depend on
//! the other. This module is the one place that knows a `color-themes`
//! contribution names a file [`color_theme::parse_toml`] or
//! [`color_theme::parse_vscode_json`] can read, dispatched by the
//! contribution's own file extension.
//!
//! What crosses the FFI seam is the whole resolved [`ColorTheme`] via
//! [`ColorThemeService::active`] — T7 (`ui-shell`) translates every field
//! into its own FFI structs from there.

use std::path::Path;
use std::sync::Arc;

use color_theme::{Appearance, ColorTheme, ThemeError};
use plugin_host::{LoadedPlugin, PluginRegistry};

/// One entry of the Appearance page's colour-theme combo.
///
/// The id is the contribution's, not the plugin's: one plugin may offer
/// several colour themes, and `Settings::color_theme` (once T7 wires it)
/// persists exactly this id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorThemeChoice {
    pub id: String,
    pub label: String,
}

/// Every colour theme the loaded plugins offer, in registry order.
///
/// A disabled plugin contributes nothing, so it is absent here — which is
/// what keeps the combo a list of themes the user can actually get.
pub fn color_themes(registry: &PluginRegistry) -> Vec<ColorThemeChoice> {
    registry
        .color_themes()
        .map(|(_, theme)| ColorThemeChoice {
            id: theme.id.clone(),
            label: theme.label.clone(),
        })
        .collect()
}

/// Reads and parses the theme file `contribution` names, dispatched by its
/// extension. `.toml` is this crate's native format; `.json` is a VS Code
/// theme. Anything else is a [`ThemeError::Parse`] naming the extension,
/// rather than a guess at which parser to try.
fn parse_by_extension(path: &Path, text: &str) -> Result<ColorTheme, ThemeError> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "toml" => color_theme::parse_toml(text),
        "json" => color_theme::parse_vscode_json(text),
        other => Err(ThemeError::Parse {
            message: format!("unsupported colour theme file extension \"{other}\""),
        }),
    }
}

/// Reads `path` back out of the plugin that contributed it — the same
/// indirection `icon-theme` gets from [`crate::icons`]: an embedded
/// built-in and an installed plugin take the same path — and parses it.
/// `None` on any failure (unreadable asset, invalid UTF-8, or a theme that
/// doesn't parse); the caller decides what "no theme here" means.
fn read_and_parse(plugin: &LoadedPlugin, path: &Path) -> Option<ColorTheme> {
    let bytes = plugin.read_asset(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    parse_by_extension(path, text).ok()
}

/// The colour theme that is active right now, if any.
///
/// Always constructible, even with no plugin offering a colour theme: the
/// FFI seam asks for the active theme on every appearance change and needs
/// an answer rather than a missing object, and "no colour theme" is a
/// legitimate answer [`ColorThemeService::active`] spells as `None`.
#[derive(Debug, Default)]
pub struct ColorThemeService {
    registry: Arc<PluginRegistry>,
    active: Option<ColorTheme>,
}

impl ColorThemeService {
    /// Scan `<config_dir>/plugins` minus the plugins in `disabled`, swap the
    /// result into the live registry, and resolve the colour theme
    /// `preferred` names.
    pub fn load(config_dir: &Path, disabled: &[String], preferred: &str) -> Self {
        plugin_host::reload(config_dir, disabled);
        Self::from_registry(plugin_host::registry(), preferred)
    }

    /// Build the service over an already-scanned registry.
    ///
    /// The registry is a parameter so a test can drive the real resolution
    /// path over its own fixtures without touching the process-wide one.
    pub fn from_registry(registry: Arc<PluginRegistry>, preferred: &str) -> Self {
        let active = choose(&registry, preferred);
        Self { registry, active }
    }

    /// The active theme, or `None` when nothing offered resolved.
    pub fn active(&self) -> Option<&ColorTheme> {
        self.active.as_ref()
    }

    /// Switch the active theme to `id`, re-running the same fallback
    /// [`choose`] applies at construction — the live-preview/revert
    /// mechanism `ThemeProvider::applyColorTheme` (T7) needs, over the
    /// registry this service was already built with.
    pub fn set_preferred(&mut self, id: &str) {
        self.active = choose(&self.registry, id);
    }
}

/// Resolves the active colour theme.
///
/// Unlike [`crate::icons::ActiveTheme::choose`] — where any offered pack is
/// an acceptable fallback — losing the colour theme leaves the whole app
/// unthemed, so a broken *preferred* theme is not the end of it: this tries,
/// in order,
///
/// 1. the `color-themes` contribution whose id is `preferred`;
/// 2. failing that, the first offered theme that parses to a dark
///    [`Appearance`] — dark is the safe default the rest of the app already
///    assumes (see [`crate::icons::icon_appearance`]);
/// 3. failing that, the first offered theme that parses at all, whatever
///    its appearance.
///
/// `None` only when nothing offered parses, or nothing is offered at all.
fn choose(registry: &PluginRegistry, preferred: &str) -> Option<ColorTheme> {
    let offered: Vec<_> = registry.color_themes().collect();

    offered
        .iter()
        .filter(|(_, theme)| theme.id == preferred)
        .find_map(|(plugin, theme)| read_and_parse(plugin, &theme.path))
        .or_else(|| {
            offered.iter().find_map(|(plugin, theme)| {
                read_and_parse(plugin, &theme.path).filter(|t| t.appearance == Appearance::Dark)
            })
        })
        .or_else(|| {
            offered
                .iter()
                .find_map(|(plugin, theme)| read_and_parse(plugin, &theme.path))
        })
}

/// Converts a resolved [`ColorTheme`]'s syntax colours into the
/// `syntax_core::ThemeStyles` shape `syntax_core::build_palette` resolves
/// against (T7, finishing what T4 deferred).
///
/// `by_language` is always empty: a colour theme carries no per-language
/// overrides, only the flat `[syntax.*]` table `theme.syntax` already is.
fn syntax_theme_styles(theme: &ColorTheme) -> syntax_core::theme::ThemeStyles {
    syntax_core::theme::ThemeStyles {
        base: theme
            .syntax
            .iter()
            .map(|(name, style)| (name.clone(), to_scope_style(*style)))
            .collect(),
        by_language: std::collections::HashMap::new(),
    }
}

fn to_scope_style(style: color_theme::ScopeStyle) -> syntax_core::theme::ScopeStyle {
    syntax_core::theme::ScopeStyle {
        fg: Some(syntax_core::theme::Rgb::new(
            style.fg.r, style.fg.g, style.fg.b,
        )),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
    }
}

/// Resolves the [`syntax_core::Palette`] for `theme_name`/`language_id`
/// through the colour-theme plugin registry, converting the theme's syntax
/// colours via [`syntax_theme_styles`] rather than the old name-based
/// `syntax_core::theme::palette`.
///
/// A fresh one-shot resolve over `registry` — acceptable for the two
/// `ui-shell` callers this exists for (a settings-page preview and a
/// per-editor highlighter), neither of which is a per-repaint hot loop for
/// an *arbitrary* theme name; the currently active theme is resolved once
/// and cached by [`ThemeProvider`] instead (see `crates/ui-shell/src/bridge/theme.rs`).
///
/// [`ThemeProvider`]: ../../ui_shell/bridge/theme/struct.ThemeProviderRust.html
pub fn build_palette(
    registry: &PluginRegistry,
    theme_name: &str,
    language_id: &str,
    user: &syntax_core::theme::UserStyles,
) -> syntax_core::theme::Palette {
    let theme = choose(registry, theme_name);
    let styles = theme.as_ref().map(syntax_theme_styles).unwrap_or_default();
    syntax_core::theme::build_palette(&styles, language_id, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `core-themes` built-in, embedded in the binary, loaded
    /// through the real registry — the join is only worth testing against
    /// the thing it joins.
    fn builtin_registry(disabled: &[String]) -> Arc<PluginRegistry> {
        Arc::new(plugin_host::load(
            Path::new("/nonexistent-config-dir"),
            plugin_host::BUILTIN_PLUGINS,
            disabled,
        ))
    }

    #[test]
    fn the_combo_lists_what_the_loaded_plugins_offer_in_registry_order() {
        assert_eq!(
            color_themes(&builtin_registry(&[])),
            vec![
                ColorThemeChoice {
                    id: "dark".to_string(),
                    label: "Dark".to_string(),
                },
                ColorThemeChoice {
                    id: "light".to_string(),
                    label: "Light".to_string(),
                },
                ColorThemeChoice {
                    id: "vscode-dark".to_string(),
                    label: "VS Code Dark".to_string(),
                },
                ColorThemeChoice {
                    id: "github-light-default".to_string(),
                    label: "GitHub Light Default".to_string(),
                },
                ColorThemeChoice {
                    id: "github-light-high-contrast".to_string(),
                    label: "GitHub Light High Contrast".to_string(),
                },
                ColorThemeChoice {
                    id: "github-light-colorblind".to_string(),
                    label: "GitHub Light Colorblind (Beta)".to_string(),
                },
                ColorThemeChoice {
                    id: "github-dark-default".to_string(),
                    label: "GitHub Dark Default".to_string(),
                },
                ColorThemeChoice {
                    id: "github-dark-high-contrast".to_string(),
                    label: "GitHub Dark High Contrast".to_string(),
                },
                ColorThemeChoice {
                    id: "github-dark-colorblind".to_string(),
                    label: "GitHub Dark Colorblind (Beta)".to_string(),
                },
                ColorThemeChoice {
                    id: "github-dark-dimmed".to_string(),
                    label: "GitHub Dark Dimmed".to_string(),
                },
                ColorThemeChoice {
                    id: "github-light".to_string(),
                    label: "GitHub Light".to_string(),
                },
                ColorThemeChoice {
                    id: "github-dark".to_string(),
                    label: "GitHub Dark".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_disabled_plugin_offers_no_colour_themes() {
        let without =
            builtin_registry(&["core-themes".to_string(), "github-vscode-theme".to_string()]);
        assert!(color_themes(&without).is_empty());
        assert!(ColorThemeService::from_registry(without, "dark")
            .active()
            .is_none());
    }

    #[test]
    fn resolving_vscode_dark_gives_back_that_theme_dark() {
        let service = ColorThemeService::from_registry(builtin_registry(&[]), "vscode-dark");
        let theme = service.active().expect("vscode-dark is a built-in theme");
        assert_eq!(theme.id, "vscode-dark");
        assert_eq!(theme.appearance, Appearance::Dark);
    }

    #[test]
    fn resolving_light_gives_back_the_light_appearance() {
        let service = ColorThemeService::from_registry(builtin_registry(&[]), "light");
        let theme = service.active().expect("light is a built-in theme");
        assert_eq!(theme.id, "light");
        assert_eq!(theme.appearance, Appearance::Light);
    }

    #[test]
    fn a_chosen_theme_that_no_longer_exists_falls_back_to_a_dark_theme() {
        // The setting outlives the plugin that offered it: uninstalled,
        // renamed, or simply a typo in a hand-edited settings.toml. The
        // fallback's exact id is not part of the contract, only that it is
        // dark — this asserts the guarantee, not an implementation detail.
        let service = ColorThemeService::from_registry(builtin_registry(&[]), "no-such-theme");
        let theme = service.active().expect("a dark theme is always offered");
        assert_eq!(theme.appearance, Appearance::Dark);
    }

    #[test]
    fn set_preferred_switches_the_active_theme_in_place() {
        let mut service = ColorThemeService::from_registry(builtin_registry(&[]), "dark");
        assert_eq!(service.active().unwrap().id, "dark");
        service.set_preferred("light");
        assert_eq!(service.active().unwrap().id, "light");
    }

    #[test]
    fn build_palette_resolves_an_arbitrary_theme_by_name() {
        let registry = builtin_registry(&[]);
        let user = syntax_core::theme::UserStyles::default();
        let palette = build_palette(&registry, "vscode-dark", "rust", &user);
        let scope = syntax_core::Scope::resolve("keyword").expect("known scope");
        assert_eq!(
            palette.style(scope).fg,
            Some(syntax_core::theme::Rgb::new(0x56, 0x9c, 0xd6))
        );
    }

    #[test]
    fn build_palette_falls_back_when_the_registry_offers_nothing() {
        let user = syntax_core::theme::UserStyles::default();
        let palette = build_palette(&PluginRegistry::default(), "vscode-dark", "rust", &user);
        let scope = syntax_core::Scope::resolve("keyword").expect("known scope");
        assert_eq!(palette.style(scope).fg, None);
    }

    #[test]
    fn an_empty_registry_answers_none_rather_than_panicking() {
        let service = ColorThemeService::from_registry(Arc::new(PluginRegistry::default()), "");
        assert!(service.active().is_none());
        assert!(ColorThemeService::default().active().is_none());
    }

    // --- T10: an installed (not built-in) theme plugin, end to end -----
    //
    // The rest of this module's tests only ever load `BUILTIN_PLUGINS` from
    // a nonexistent config dir. These two drive the full stack a real user
    // install goes through: a plugin directory written to a temp
    // `<config_dir>/plugins/`, scanned by `plugin_host::load`, and resolved
    // by `ColorThemeService` — proving install -> registry -> service
    // resolution works for an installed plugin, not just the registry
    // layer.

    fn write_plugin(
        config_dir: &std::path::Path,
        dir_name: &str,
        manifest: &str,
        asset_name: &str,
        asset_contents: &str,
    ) {
        let plugin_dir = config_dir.join(plugin_host::PLUGINS_DIR).join(dir_name);
        std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
        // "plugin.toml" is `plugin_api::MANIFEST_FILE`; not worth a new
        // dev-dependency on `plugin-api` just to name it symbolically.
        std::fs::write(plugin_dir.join("plugin.toml"), manifest).expect("manifest");
        std::fs::write(plugin_dir.join(asset_name), asset_contents).expect("asset");
    }

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
    fn an_installed_toml_theme_resolves_as_active_alongside_the_built_ins() {
        let config_dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(config_dir.path().join(plugin_host::PLUGINS_DIR))
            .expect("plugins root");
        write_plugin(
            config_dir.path(),
            "my-theme",
            r#"
            id = "my-theme"
            name = "My Theme Plugin"
            version = "1.0.0"
            api_version = 1

            [[contributes.color-themes]]
            id = "my-toml-theme"
            label = "My TOML Theme"
            path = "my-theme.toml"
            "#,
            "my-theme.toml",
            &minimal_native_theme_toml("my-toml-theme", "My TOML Theme"),
        );

        let registry = Arc::new(plugin_host::load(
            config_dir.path(),
            plugin_host::BUILTIN_PLUGINS,
            &[],
        ));

        let ids: Vec<String> = color_themes(&registry).into_iter().map(|c| c.id).collect();
        assert!(ids.contains(&"my-toml-theme".to_string()));
        assert!(
            ids.contains(&"dark".to_string()),
            "built-ins still offered: {ids:?}"
        );

        let service = ColorThemeService::from_registry(registry, "my-toml-theme");
        assert_eq!(
            service.active().map(|t| &t.id),
            Some(&"my-toml-theme".to_string())
        );
        assert_eq!(
            service.active().unwrap().appearance,
            color_theme::Appearance::Dark
        );
    }

    #[test]
    fn an_installed_unmodified_vscode_json_theme_resolves_as_active_alongside_the_built_ins() {
        let config_dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(config_dir.path().join(plugin_host::PLUGINS_DIR))
            .expect("plugins root");
        write_plugin(
            config_dir.path(),
            "my-vscode-theme",
            r#"
            id = "my-vscode-theme"
            name = "My VS Code Theme Plugin"
            version = "1.0.0"
            api_version = 1

            [[contributes.color-themes]]
            id = "my-json-theme"
            label = "My JSON Theme"
            path = "my-vscode-theme.json"
            "#,
            "my-vscode-theme.json",
            &minimal_vscode_json("My JSON Theme"),
        );

        let registry = Arc::new(plugin_host::load(
            config_dir.path(),
            plugin_host::BUILTIN_PLUGINS,
            &[],
        ));

        let ids: Vec<String> = color_themes(&registry).into_iter().map(|c| c.id).collect();
        assert!(ids.contains(&"my-json-theme".to_string()));
        assert!(
            ids.contains(&"dark".to_string()),
            "built-ins still offered: {ids:?}"
        );

        // Drives the real `.json` -> `parse_vscode_json` dispatch in
        // `parse_by_extension` via `ColorThemeService`, not a direct call.
        let service = ColorThemeService::from_registry(registry, "my-json-theme");
        let theme = service.active().expect("installed VS Code theme resolves");
        assert_eq!(theme.label, "My JSON Theme");
        assert_eq!(theme.appearance, color_theme::Appearance::Dark);
    }
}
