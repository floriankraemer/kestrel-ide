//! Rasterising a resolved icon id into pixels a view can blit.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use resvg::tiny_skia;
use resvg::usvg;

use crate::{Appearance, IconError, IconPack};

/// Where a pack's files come from.
///
/// A trait rather than a path, because a built-in plugin's SVGs are
/// embedded in the binary and an installed plugin's are on disk, and the
/// renderer must work the same for both. The implementation lives with the
/// caller — this crate deliberately does not depend on `plugin-host`
/// (ADR-0026).
///
/// `relative` is what [`IconPack::asset_path`] returned: relative to the
/// pack description, never absolute.
pub trait IconAssets {
    /// Read one asset, or say why not.
    fn read(&self, relative: &Path) -> Result<Vec<u8>, IconError>;
}

/// One rasterised icon.
///
/// `pixels` is **premultiplied RGBA8**, `width * height * 4` bytes, row
/// major. The format is not arbitrary: the FFI seam wraps these bytes in
/// `QImage::Format_RGBA8888_Premultiplied`, which matches tiny-skia's byte
/// order exactly. Qt's `Format_ARGB32_Premultiplied` is BGRA on
/// little-endian and would need a per-pixel swizzle, so "simplifying" this
/// to ARGB32 turns every icon's red and blue channels around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedIcon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Rasterises icons and remembers what it rasterised.
///
/// Keyed by `(pack id, icon id, px)` — the pack id is part of the key
/// because two packs may both call an icon `rust`, and the size because an
/// SVG scaled up from a 16px raster looks like exactly that.
#[derive(Debug, Default)]
pub struct IconRenderer {
    // The key is owned, so a lookup allocates two short strings even on a
    // hit. That is deliberate: the view memoises `QIcon`s by the same key
    // (P5), so this map is consulted once per distinct icon and size, not
    // once per painted row.
    cache: HashMap<(String, String, u32), Arc<RenderedIcon>>,
}

impl IconRenderer {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rasterise `icon_id` from `pack` at `px` by `px`, or serve it from
    /// the cache.
    ///
    /// An icon id with no asset behind it — a pack table naming art that
    /// was never shipped — falls back to the pack's default file icon
    /// rather than failing the row: a tree with one wrong icon is better
    /// than a tree with an error dialog. Only a default that is *itself*
    /// missing is an error.
    pub fn render(
        &mut self,
        pack: &IconPack,
        assets: &dyn IconAssets,
        icon_id: &str,
        px: u32,
    ) -> Result<Arc<RenderedIcon>, IconError> {
        let key = (pack.id.clone(), icon_id.to_owned(), px);
        if let Some(icon) = self.cache.get(&key) {
            return Ok(Arc::clone(icon));
        }

        let fallback = pack.default_file_icon(Appearance::Dark);
        let icon = match rasterise(pack, assets, icon_id, px) {
            Ok(icon) => icon,
            // The reason is dropped on purpose: a pack whose art is
            // incomplete is reported once by the Plugins page (P7), not once
            // per painted row.
            Err(_) if icon_id != fallback => rasterise(pack, assets, fallback, px)?,
            Err(err) => return Err(err),
        };

        let icon = Arc::new(icon);
        // Cached under the *requested* id, so a repeatedly missing icon is
        // read from the asset store once rather than on every repaint.
        self.cache.insert(key, Arc::clone(&icon));
        Ok(icon)
    }

    /// Forget everything. Called when the pack or the colour theme
    /// changes; the cache key does not carry an appearance because the
    /// light substitution happens before this point, on the icon id.
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

/// Safety ceiling on a rasterised SVG's longer side, so an SVG with an
/// absurd `viewBox` never produces a multi-hundred-megabyte pixmap.
const MAX_SVG_DIMENSION: u32 = 4096;

/// Rasterises arbitrary SVG bytes at their own intrinsic size (scaled down
/// to fit [`MAX_SVG_DIMENSION`] when larger), for viewers that show an SVG
/// as an image rather than a themed icon.
///
/// Reuses this crate's own `resvg`/`usvg` pipeline rather than a second SVG
/// rasteriser: the byte order and premultiplication of [`RenderedIcon`] are
/// exactly [`IconRenderer::render`]'s, so a consumer decodes both the same
/// way.
pub fn rasterise_svg(svg_bytes: &[u8]) -> Result<RenderedIcon, IconError> {
    let tree = usvg::Tree::from_data(svg_bytes, &usvg::Options::default()).map_err(|e| {
        IconError::MalformedSvg {
            icon: "<image>".to_owned(),
            message: e.to_string(),
        }
    })?;

    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let scale = (MAX_SVG_DIMENSION as f32 / longest).min(1.0);
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or(IconError::UnsupportedSize(width.max(height)))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Ok(RenderedIcon {
        width,
        height,
        pixels: pixmap.take(),
    })
}

fn rasterise(
    pack: &IconPack,
    assets: &dyn IconAssets,
    icon_id: &str,
    px: u32,
) -> Result<RenderedIcon, IconError> {
    let svg = assets.read(&pack.asset_path(icon_id))?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).map_err(|e| {
        IconError::MalformedSvg {
            icon: icon_id.to_owned(),
            message: e.to_string(),
        }
    })?;

    let mut pixmap = tiny_skia::Pixmap::new(px, px).ok_or(IconError::UnsupportedSize(px))?;
    let size = tree.size();
    // Fit and centre rather than stretch: an icon authored on a non-square
    // canvas should keep its proportions, and every row in the tree is the
    // same square regardless.
    let scale = (px as f32 / size.width()).min(px as f32 / size.height());
    let dx = (px as f32 - size.width() * scale) / 2.0;
    let dy = (px as f32 - size.height() * scale) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Ok(RenderedIcon {
        width: px,
        height: px,
        pixels: pixmap.take(),
    })
}

#[cfg(test)]
mod rasterise_svg_tests {
    use super::*;

    fn svg_with_viewbox(width: u32, height: u32) -> Vec<u8> {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}"><rect width="{width}" height="{height}" fill="#dea584"/></svg>"##
        )
        .into_bytes()
    }

    #[test]
    fn a_small_svg_rasterises_at_its_own_intrinsic_size() {
        let icon = rasterise_svg(&svg_with_viewbox(32, 16)).expect("renders");
        assert_eq!((icon.width, icon.height), (32, 16));
        assert_eq!(icon.pixels.len(), 32 * 16 * 4);
        assert!(
            icon.pixels.as_chunks::<4>().0.iter().any(|px| px[3] != 0),
            "a filled rect must produce opaque pixels"
        );
    }

    #[test]
    fn an_oversized_svg_is_scaled_down_to_the_ceiling_preserving_aspect() {
        let icon = rasterise_svg(&svg_with_viewbox(20_000, 10_000)).expect("renders");
        assert_eq!(icon.width, MAX_SVG_DIMENSION);
        assert_eq!(icon.height, MAX_SVG_DIMENSION / 2);
    }

    #[test]
    fn malformed_svg_bytes_are_a_typed_error_rather_than_a_panic() {
        assert!(matches!(
            rasterise_svg(b"not an svg"),
            Err(IconError::MalformedSvg { .. })
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use super::*;

    const PACK: &str = r#"
id = "fixture"
label = "Fixture"
default_file = "file"
default_folder = "folder"
default_folder_open = "folder-open"
default_root_folder = "folder-root"

[file_extensions]
rs = "rust"
xyz = "never-shipped"
"#;

    /// A filled square, so every pixel of the output carries alpha.
    fn svg(colour: &str) -> Vec<u8> {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" fill="{colour}"/></svg>"#
        )
        .into_bytes()
    }

    #[derive(Default)]
    struct MapAssets {
        files: HashMap<PathBuf, Vec<u8>>,
        reads: Cell<usize>,
    }

    impl IconAssets for MapAssets {
        fn read(&self, relative: &Path) -> Result<Vec<u8>, IconError> {
            self.reads.set(self.reads.get() + 1);
            self.files
                .get(relative)
                .cloned()
                .ok_or_else(|| IconError::UnreadableAsset {
                    path: relative.display().to_string(),
                    message: "no such asset".into(),
                })
        }
    }

    fn fixture() -> (IconPack, MapAssets) {
        let pack = IconPack::from_toml_str(PACK).expect("fixture pack parses");
        let mut assets = MapAssets::default();
        assets.files.insert(pack.asset_path("rust"), svg("#dea584"));
        assets.files.insert(pack.asset_path("file"), svg("#6d8086"));
        assets
            .files
            .insert(pack.asset_path("broken"), b"<svg".to_vec());
        (pack, assets)
    }

    #[test]
    fn a_rendered_icon_has_the_requested_size_and_a_non_empty_alpha_channel() {
        let (pack, assets) = fixture();
        let icon = IconRenderer::new()
            .render(&pack, &assets, "rust", 24)
            .expect("renders");

        assert_eq!((icon.width, icon.height), (24, 24));
        assert_eq!(icon.pixels.len(), 24 * 24 * 4);
        assert!(
            icon.pixels.as_chunks::<4>().0.iter().any(|px| px[3] != 0),
            "a filled square must produce opaque pixels"
        );
    }

    #[test]
    fn an_icon_id_with_no_asset_falls_back_to_the_pack_default() {
        let (pack, assets) = fixture();
        let mut renderer = IconRenderer::new();

        let fallback = renderer
            .render(&pack, &assets, "never-shipped", 16)
            .expect("falls back rather than failing the row");
        let default = renderer
            .render(&pack, &assets, "file", 16)
            .expect("renders");

        assert_eq!(fallback.pixels, default.pixels);
    }

    #[test]
    fn a_missing_default_icon_is_an_error_rather_than_an_endless_fallback() {
        let pack = IconPack::from_toml_str(PACK).expect("parses");
        let assets = MapAssets::default();
        assert!(matches!(
            IconRenderer::new().render(&pack, &assets, "file", 16),
            Err(IconError::UnreadableAsset { .. })
        ));
    }

    #[test]
    fn malformed_svg_is_a_typed_error_rather_than_a_panic() {
        let (_, assets) = fixture();
        // Asked for directly, with the default present, `broken` falls back
        // — so ask for it *as* the default to see the error itself.
        let pack_with_broken_default = IconPack::from_toml_str(
            &PACK.replace(r#"default_file = "file""#, r#"default_file = "broken""#),
        )
        .expect("parses");
        assert!(matches!(
            IconRenderer::new().render(&pack_with_broken_default, &assets, "broken", 16),
            Err(IconError::MalformedSvg { .. })
        ));
    }

    #[test]
    fn a_second_request_for_the_same_icon_and_size_is_served_from_the_cache() {
        let (pack, assets) = fixture();
        let mut renderer = IconRenderer::new();

        renderer
            .render(&pack, &assets, "rust", 16)
            .expect("renders");
        let after_first = assets.reads.get();
        renderer
            .render(&pack, &assets, "rust", 16)
            .expect("renders");
        assert_eq!(assets.reads.get(), after_first, "no second asset read");

        // A different size is a different entry — an SVG scaled up from a
        // 16px raster is what the cache key's third component prevents.
        renderer
            .render(&pack, &assets, "rust", 32)
            .expect("renders");
        assert!(assets.reads.get() > after_first);
    }

    #[test]
    fn a_size_no_pixmap_can_hold_is_a_typed_error() {
        let (pack, assets) = fixture();
        assert!(matches!(
            IconRenderer::new().render(&pack, &assets, "rust", 0),
            Err(IconError::UnsupportedSize(0))
        ));
    }
}
