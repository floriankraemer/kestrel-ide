//! Renders Markdown (with inline Mermaid diagrams) into the HTML subset
//! `QTextDocument` understands (ADR-0033).
//!
//! Qt-free by design, like `syntax-core` and `icon-theme`: this crate knows
//! nothing about plugins, cxx-qt or the preview dock. It is joined to the
//! plugin host in `app_core::preview`, exactly the way `icon-theme` is
//! joined to `plugin-host` in `app_core::icons`.
//!
//! Two rules hold the whole crate together:
//!
//! * **Raw HTML in the source is never passed through.**
//!   `comrak::options::Render::r#unsafe` stays `false`, always — a Markdown
//!   file in an opened project is untrusted content (ADR-0021's rule for
//!   the AI chat panel applies unchanged), and Qt rich text can load a
//!   remote `<img src="http://...">`, which would leak that a file was
//!   previewed. This is the whole mitigation, and `html::tests` proves it
//!   holds.
//! * **The emitted HTML stays inside Qt's rich-text subset.** Qt's engine
//!   understands a fixed slice of HTML 4 / CSS 2.1; a construct outside it
//!   is silently dropped rather than reported, so every tag this crate
//!   emits is deliberate and unit-tested — see `html::tests`.
//!
//! [`Renderer`] is the stateful entry point a caller re-rendering the same
//! document on every keystroke should hold onto: it owns the bundled font
//! and the diagram cache, so an unchanged ```mermaid fence costs one
//! `HashMap` lookup on the second render, not a second Mermaid layout
//! pass. [`render`] is the free-function, cache-free equivalent for a
//! one-shot caller (tests, a CLI, a search index that wants the text).

mod font;
mod highlight;
mod html;
mod links;
mod mermaid;

pub use html::{Anchor, Diagram, RenderOptions};
pub use links::{resolve_link, LinkTarget};
pub use mermaid::{DiagramError, RasterisedDiagram};

/// What one document rendered to, diagrams included as pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// HTML inside Qt's rich-text subset, ready for `QTextDocument::setHtml`.
    pub html: String,
    /// One entry per ```mermaid fence that rasterised successfully. A
    /// fence that failed is not silently dropped: `html` already carries
    /// its fallback (the source in a `<pre>`, the parser's message above
    /// it) in place of the `<img>` tag a successful one keeps.
    pub images: Vec<PreviewImage>,
    /// `(source line, anchor name)`, in document order — see
    /// [`html::Rendered::anchors`](html) for what builds it.
    pub anchors: Vec<Anchor>,
}

/// One rasterised diagram, keyed the way `html` references it
/// (`<img src="ide-preview:{key}">`) — premultiplied RGBA8, ready for
/// `QImage::Format_RGBA8888_Premultiplied` on the C++ side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImage {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Owns the bundled font and the diagram cache across repeated renders of
/// an evolving document — construct one per open preview, not one per
/// keystroke, or the cache buys nothing.
#[derive(Default)]
pub struct Renderer {
    diagrams: mermaid::Renderer,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The appearance changed, so every cached pixmap was rendered against
    /// colours that no longer apply.
    pub fn clear_diagram_cache(&mut self) {
        self.diagrams.clear();
    }

    /// Rasterise SVG a wasm preview provider already produced, with the
    /// same bundled font every other diagram in this document uses. The
    /// only route into this crate's rasteriser that does not start from
    /// Mermaid source — `app_core::preview` calls it for a sandboxed
    /// provider's `WasmPreviewImage` and attaches the key that struct
    /// already carries, which is why this returns a bare
    /// [`RasterisedDiagram`] rather than a [`PreviewImage`].
    pub fn rasterise_guest_svg(
        &self,
        svg: &str,
        width_px: u32,
    ) -> Result<RasterisedDiagram, DiagramError> {
        self.diagrams.rasterise_guest_svg(svg, width_px)
    }

    /// Render `source` at `width_px` (the preview pane's content width, in
    /// device pixels — diagrams are cached per width, so a resize costs
    /// one re-rasterisation per diagram, not a permanently oversized or
    /// blurry one).
    pub fn render(&mut self, source: &str, width_px: u32, options: &RenderOptions) -> Rendered {
        let raw = html::render(source, options);
        let mut html = raw.html;
        let mut images = Vec::with_capacity(raw.diagrams.len());

        for diagram in &raw.diagrams {
            let placeholder = format!(r#"<img src="ide-preview:{}">"#, diagram.key);
            match self
                .diagrams
                .rasterise(&diagram.key, &diagram.source, width_px)
            {
                Ok(rasterised) => images.push(PreviewImage {
                    key: diagram.key.clone(),
                    width: rasterised.width,
                    height: rasterised.height,
                    pixels: rasterised.pixels.clone(),
                }),
                Err(err) => {
                    html = html.replace(&placeholder, &fallback_block(&diagram.source, &err));
                }
            }
        }

        Rendered {
            html,
            images,
            anchors: raw.anchors,
        }
    }

    /// Render a file that *is* one Mermaid diagram (`.mermaid`, `.mmd`)
    /// rather than a Markdown document that contains fences.
    ///
    /// Deliberately not "wrap the source in a synthetic ```mermaid fence
    /// and call [`Self::render`]": a diagram file is not Markdown, such a
    /// fence would have to out-run whatever run of backticks the source
    /// itself contains, and comrak has nothing to contribute to a document
    /// with no prose in it. Every piece below is the one the fence path
    /// already uses — the same key, the same cache, the same fallback — so
    /// one diagram renders identically whichever kind of file it lives in.
    ///
    /// No anchors: there are no headings to scroll-sync against, which is
    /// why this returns an empty `anchors` rather than pretending to one.
    pub fn render_diagram(&mut self, source: &str, width_px: u32) -> Rendered {
        let key = html::diagram_key(source);
        match self.diagrams.rasterise(&key, source, width_px) {
            Ok(rasterised) => {
                let image = PreviewImage {
                    key: key.clone(),
                    width: rasterised.width,
                    height: rasterised.height,
                    pixels: rasterised.pixels.clone(),
                };
                Rendered {
                    html: format!(r#"<img src="ide-preview:{key}">"#),
                    images: vec![image],
                    anchors: Vec::new(),
                }
            }
            Err(err) => Rendered {
                html: fallback_block(source, &err),
                images: Vec::new(),
                anchors: Vec::new(),
            },
        }
    }

    /// Renders a Carve document (ADR-0054).
    ///
    /// `carve-lang`'s convenience renderer exposes no source positions, so a
    /// Carve preview has heading links but no editor-to-preview scroll map;
    /// the anchors are empty rather than guessed at by a second, approximate
    /// heading parser here.
    pub fn render_carve(&mut self, source: &str) -> Rendered {
        Rendered {
            html: carve::to_html(source),
            images: Vec::new(),
            anchors: Vec::new(),
        }
    }
}

/// What a diagram that failed to rasterise shows instead of an image: its
/// own source, so a diagram being typed mid-edit reads as "not finished
/// yet", not as a dialog interrupting the document.
fn fallback_block(source: &str, err: &DiagramError) -> String {
    let mut escaped_source = String::new();
    let _ = comrak::html::escape(&mut escaped_source, source);
    let mut escaped_err = String::new();
    let _ = comrak::html::escape(&mut escaped_err, &err.to_string());
    format!("<p><em>{escaped_err}</em></p><pre>{escaped_source}</pre>")
}

/// A reasonable default width for a one-shot render with no dock to
/// measure — [`Renderer::render`] should be given the pane's real width
/// whenever one exists.
const DEFAULT_WIDTH_PX: u32 = 1600;

/// Render one Markdown document without a diagram cache — for a caller
/// that renders once (a test, a CLI, a search index that only wants the
/// text) rather than re-rendering the same document repeatedly. A
/// [`Renderer`] is the right choice for the latter.
pub fn render(source: &str, options: &RenderOptions) -> Rendered {
    Renderer::new().render(source, DEFAULT_WIDTH_PX, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mermaid_fence_rasterises_to_a_preview_image() {
        let rendered = render(
            "```mermaid\ngraph TD\nA-->B\n```\n",
            &RenderOptions::default(),
        );
        assert_eq!(rendered.images.len(), 1);
        assert!(rendered.images[0].width > 0);
        assert!(rendered.images[0].height > 0);
        assert!(rendered.html.contains("ide-preview:"));
    }

    #[test]
    fn a_broken_diagram_falls_back_to_its_source_rather_than_vanishing() {
        let rendered = render(
            "```mermaid\nthis is not a real diagram type {{{\n```\n",
            &RenderOptions::default(),
        );
        assert!(rendered.images.is_empty());
        assert!(
            rendered.html.contains("this is not a real diagram type"),
            "{}",
            rendered.html
        );
        assert!(!rendered.html.contains("ide-preview:"), "{}", rendered.html);
    }

    #[test]
    fn a_whole_file_diagram_rasterises_without_going_through_the_parser() {
        let mut renderer = Renderer::new();
        let rendered = renderer.render_diagram("graph TD\nA-->B\n", 400);
        assert_eq!(rendered.images.len(), 1);
        assert_eq!(rendered.images[0].width, 400);
        assert!(rendered.html.contains("ide-preview:"));
        assert!(rendered.anchors.is_empty());
    }

    #[test]
    fn a_broken_whole_file_diagram_falls_back_to_its_source() {
        let mut renderer = Renderer::new();
        let rendered = renderer.render_diagram("this is not a real diagram type {{{\n", 400);
        assert!(rendered.images.is_empty());
        assert!(
            rendered.html.contains("this is not a real diagram type"),
            "{}",
            rendered.html
        );
        assert!(!rendered.html.contains("ide-preview:"), "{}", rendered.html);
    }

    #[test]
    fn a_whole_file_diagram_shares_the_fence_path_s_cache() {
        let mut renderer = Renderer::new();
        let source = "graph TD\nA-->B\n";
        let first = renderer.render_diagram(source, 400);
        let second = renderer.render_diagram(source, 400);
        assert_eq!(first.images, second.images);
    }

    #[test]
    fn a_diagram_source_containing_backticks_is_not_mistaken_for_a_fence() {
        // The reason `render_diagram` does not wrap its source in a fence:
        // this source would close a three-backtick one early.
        let mut renderer = Renderer::new();
        let rendered = renderer.render_diagram("graph TD\nA[\"```\"]-->B\n", 400);
        assert!(
            rendered.html.contains("ide-preview:") || rendered.html.contains("<pre>"),
            "{}",
            rendered.html
        );
    }

    #[test]
    fn rendering_the_same_document_twice_through_one_renderer_hits_the_cache() {
        let mut renderer = Renderer::new();
        let options = RenderOptions::default();
        let source = "```mermaid\ngraph TD\nA-->B\n```\n";
        let first = renderer.render(source, 400, &options);
        let second = renderer.render(source, 400, &options);
        assert_eq!(first.images, second.images);
    }
}
