//! The previews join: plugins on one side, `markdown-preview`'s renderer
//! on the other. Same shape as [`crate::icons`]'s icon-theme join, and for
//! the same reason (ADR-0033): `plugin-host` stores contribution payloads
//! and never interprets one, `markdown-preview` renders documents and
//! knows nothing about plugins, so the two meet here, in the application
//! layer, and this module is the only place that knows a `previews`
//! contribution names an extension list a renderer can serve.
//!
//! Dispatch, for one extension:
//!
//! 1. The provider whose contribution claims it — installed shadows
//!    built-in, then first by plugin id, exactly the direction
//!    [`plugin_host::PluginRegistry::claim`] already resolves an id
//!    collision, because `registry.previews()` walks the registry in that
//!    same order and the first match per extension wins.
//! 2. If that plugin ships a `[wasm]` component, [`plugin_host::WasmTier`]
//!    renders it — a trap or a failure there disables that one plugin,
//!    same as [`plugin_host::WasmTier::invoke`], and is never silently
//!    swapped for the native path: two different answers for one file
//!    would be worse than one honest error.
//! 3. Otherwise a recognised built-in provider (`markdown`, `mermaid`, or
//!    `carve`) is served by its native renderer directly.
//!    A `previews` id this build does not recognise and that names no
//!    component is inert — not a load error, because the manifest itself
//!    is well-formed; `plugin-api` already accepts a componentless
//!    `previews` contribution for exactly the built-in's sake.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use markdown_preview::{Anchor, PreviewImage, RenderOptions};
use plugin_host::{PluginRegistry, WasmError, WasmTier};

/// What one document rendered to. Same shape as
/// `markdown_preview::Rendered`, kept as its own type here rather than a
/// re-export: a wasm-served preview has no anchors at all (the WIT world
/// carries none), and a type this module owns is what lets that be a
/// documented `Vec::new()` rather than a silent gap in someone else's
/// struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub html: String,
    pub images: Vec<PreviewImage>,
    pub anchors: Vec<Anchor>,
}

/// Why a document could not be previewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    /// No loaded plugin's `previews` contribution claims this extension.
    NoProvider,
    /// The provider is a wasm plugin and it failed — trapped, refused to
    /// activate, or was already disabled. Carries the same message the
    /// Plugins page would show for it.
    Provider(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => write!(f, "no plugin previews this file type"),
            Self::Provider(message) => write!(f, "{message}"),
        }
    }
}

/// Which plugin serves one extension, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Provider {
    plugin_id: String,
    contribution_id: String,
    has_component: bool,
}

/// Extension → provider, installed shadowing built-in — see the module
/// doc for why registry order alone is enough to get that direction.
fn providers(registry: &PluginRegistry) -> HashMap<String, Provider> {
    let mut map = HashMap::new();
    for (plugin, contribution) in registry.previews() {
        let provider = Provider {
            plugin_id: plugin.id().to_string(),
            contribution_id: contribution.id.clone(),
            has_component: plugin.manifest().wasm.is_some(),
        };
        for extension in &contribution.extensions {
            map.entry(extension.to_lowercase())
                .or_insert_with(|| provider.clone());
        }
    }
    map
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

/// Serves a preview for whichever extensions the loaded plugins' `previews`
/// contributions claim, dispatching each render to the built-in native
/// renderer or a running wasm plugin as the module doc describes.
pub struct PreviewService {
    tier: Arc<WasmTier>,
    providers: HashMap<String, Provider>,
    renderer: markdown_preview::Renderer,
    options: RenderOptions,
}

impl PreviewService {
    /// Build the service over an already-scanned registry and a running
    /// tier. Both are parameters, not read from the process-wide
    /// singletons, so a test can drive the real resolution path over its
    /// own fixtures — the same reason `IconService::from_registry` takes
    /// one.
    pub fn from_registry(registry: Arc<PluginRegistry>, tier: Arc<WasmTier>) -> Self {
        Self {
            providers: providers(&registry),
            tier,
            renderer: markdown_preview::Renderer::new(),
            options: RenderOptions::default(),
        }
    }

    /// The theme fenced code is highlighted with — set once, after a theme
    /// change, alongside clearing the diagram cache (colours baked into a
    /// cached diagram no longer apply either).
    pub fn set_theme(&mut self, theme_name: &str) {
        self.options.theme_name = theme_name.to_string();
        self.renderer.clear_diagram_cache();
    }

    /// Does any loaded plugin preview `path`'s extension? Drives the dock's
    /// enabled/empty state without rendering anything.
    pub fn has_preview(&self, path: &Path) -> bool {
        extension_of(path).is_some_and(|ext| self.providers.contains_key(&ext))
    }

    /// Render `source` (already read from `path`'s buffer — this module
    /// never touches a filesystem) at `width_px`.
    pub fn render(
        &mut self,
        path: &Path,
        source: &str,
        width_px: u32,
    ) -> Result<Rendered, PreviewError> {
        let extension = extension_of(path).ok_or(PreviewError::NoProvider)?;
        let provider = self
            .providers
            .get(&extension)
            .cloned()
            .ok_or(PreviewError::NoProvider)?;

        if provider.has_component {
            return self.render_wasm(&provider, source, width_px);
        }
        if provider.contribution_id == "markdown" {
            let rendered = self.renderer.render(source, width_px, &self.options);
            return Ok(Rendered {
                html: rendered.html,
                images: rendered.images,
                anchors: rendered.anchors,
            });
        }
        // A standalone `.mermaid`/`.mmd` file: the whole document is one
        // diagram, so it skips comrak entirely rather than being wrapped in
        // a synthetic fence — see `Renderer::render_diagram`. One extra arm
        // here rather than a second provider table: which renderer serves a
        // contribution is exactly the decision this function already makes.
        if provider.contribution_id == "mermaid" {
            let rendered = self.renderer.render_diagram(source, width_px);
            return Ok(Rendered {
                html: rendered.html,
                images: rendered.images,
                anchors: rendered.anchors,
            });
        }
        if provider.contribution_id == "carve" {
            let rendered = self.renderer.render_carve(source);
            return Ok(Rendered {
                html: rendered.html,
                images: rendered.images,
                anchors: rendered.anchors,
            });
        }
        // A `previews` id this build does not recognise and that names no
        // component: well-formed, inert. See the module doc, point 3.
        Err(PreviewError::NoProvider)
    }

    fn render_wasm(
        &mut self,
        provider: &Provider,
        source: &str,
        width_px: u32,
    ) -> Result<Rendered, PreviewError> {
        let wasm = self
            .tier
            .render(&provider.plugin_id, &provider.contribution_id, source)
            .map_err(|err| PreviewError::Provider(explain_wasm_error(&err)))?;

        let mut images = Vec::with_capacity(wasm.images.len());
        let html = wasm.html;
        for image in wasm.images {
            match self.renderer.rasterise_guest_svg(&image.svg, width_px) {
                Ok(raster) => images.push(PreviewImage {
                    key: image.key,
                    width: raster.width,
                    height: raster.height,
                    pixels: raster.pixels,
                }),
                Err(err) => {
                    // Left as the guest's own `<img>` tag: this crate does
                    // not control what a third-party provider's HTML looks
                    // like the way it controls its own, so a targeted
                    // string replace here would be a guess. Qt shows a
                    // broken-image glyph, which is an honest picture of
                    // what happened.
                    let _ = err;
                }
            }
        }

        Ok(Rendered {
            html,
            images,
            anchors: Vec::new(),
        })
    }
}

fn explain_wasm_error(err: &WasmError) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_registry(disabled: &[String]) -> Arc<PluginRegistry> {
        Arc::new(plugin_host::load(
            Path::new("/nonexistent-config-dir"),
            plugin_host::BUILTIN_PLUGINS,
            disabled,
        ))
    }

    fn empty_tier() -> Arc<WasmTier> {
        Arc::new(WasmTier::default())
    }

    #[test]
    fn the_builtin_markdown_provider_serves_its_registered_extensions() {
        let service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        for ext in ["md", "markdown", "mdown", "mkd"] {
            assert!(
                service.has_preview(Path::new(&format!("readme.{ext}"))),
                "{ext}"
            );
        }
        assert!(!service.has_preview(Path::new("main.rs")));
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        assert!(service.has_preview(Path::new("README.MD")));
    }

    #[test]
    fn a_file_with_no_extension_has_no_preview() {
        let service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        assert!(!service.has_preview(Path::new("Makefile")));
    }

    #[test]
    fn rendering_a_markdown_file_reaches_the_native_renderer() {
        let mut service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        let rendered = service
            .render(Path::new("a.md"), "# Title\n", 800)
            .expect("markdown is served natively");
        assert!(rendered.html.contains("Title"));
    }

    #[test]
    fn the_builtin_also_serves_standalone_mermaid_files() {
        let service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        for ext in ["mermaid", "mmd"] {
            assert!(
                service.has_preview(Path::new(&format!("erd.{ext}"))),
                "{ext}"
            );
        }
    }

    #[test]
    fn rendering_a_mermaid_file_reaches_the_whole_file_diagram_renderer() {
        let mut service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        let rendered = service
            .render(Path::new("erd.mermaid"), "graph TD\nA-->B\n", 400)
            .expect("mermaid is served natively");
        // The diagram path, not the Markdown one: comrak would have wrapped
        // this in a `<p>` of prose rather than rasterising it.
        assert_eq!(rendered.images.len(), 1);
        assert!(rendered.html.contains("ide-preview:"), "{}", rendered.html);
    }

    #[test]
    fn rendering_a_carve_file_reaches_the_native_carve_renderer() {
        let mut service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        let rendered = service
            .render(
                Path::new("guide.crv"),
                "# Carve\n\n/clear/ and *strong*.\n",
                800,
            )
            .expect("Carve is served natively");

        assert!(
            rendered.html.contains("<h1>Carve</h1>"),
            "{}",
            rendered.html
        );
        assert!(
            rendered.html.contains("<em>clear</em>"),
            "{}",
            rendered.html
        );
        assert!(
            rendered.html.contains("<strong>strong</strong>"),
            "{}",
            rendered.html
        );
    }

    #[test]
    fn both_carve_extensions_are_previewed() {
        let service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        assert!(service.has_preview(Path::new("guide.crv")));
        assert!(service.has_preview(Path::new("guide.carve")));
    }

    #[test]
    fn a_file_with_no_provider_is_a_typed_error_not_a_panic() {
        let mut service = PreviewService::from_registry(builtin_registry(&[]), empty_tier());
        assert_eq!(
            service.render(Path::new("a.rs"), "fn main() {}", 800),
            Err(PreviewError::NoProvider)
        );
    }

    #[test]
    fn disabling_the_builtin_plugin_removes_its_provider() {
        let service = PreviewService::from_registry(
            builtin_registry(&["markdown-preview".to_string()]),
            empty_tier(),
        );
        assert!(!service.has_preview(Path::new("readme.md")));
    }

    #[test]
    fn an_installed_provider_shadows_the_builtin_for_the_same_extension() {
        // `registry.previews()` walks installed plugins before built-ins
        // (`plugin_host::load`'s own order), so the first entry for a
        // shared extension is the installed one — this asserts that
        // property holds through this module's own provider table, not
        // just through the registry it is built from.
        use tempfile::TempDir;
        let config = TempDir::new().expect("temp dir");
        let plugin_dir = config.path().join("plugins").join("acme-preview");
        std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            "id = \"acme-preview\"\nname = \"Acme\"\nversion = \"1\"\napi_version = 1\n\
             [[contributes.previews]]\nid = \"acme-markdown\"\nlabel = \"Acme Markdown\"\n\
             extensions = [\"md\"]\n",
        )
        .expect("manifest");

        let registry = Arc::new(plugin_host::load(
            config.path(),
            plugin_host::BUILTIN_PLUGINS,
            &[],
        ));
        let service = PreviewService::from_registry(registry, empty_tier());
        let provider = service.providers.get("md").expect("a provider for md");
        assert_eq!(provider.plugin_id, "acme-preview");
    }
}
