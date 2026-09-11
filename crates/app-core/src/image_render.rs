//! Rasterising an SVG image tab's file for the view.
//!
//! Reuses `icon-theme`'s own `resvg` pipeline (`icon_theme::rasterise_svg`)
//! rather than a second SVG rasteriser, the same join `app_core::icons`
//! already makes with that crate for file-tree art.

use std::fs;
use std::path::Path;

pub use icon_theme::{IconError, RenderedIcon};

/// Read `path` and rasterise it as an SVG. The read failure and the parse
/// failure share [`IconError::UnreadableAsset`]/[`IconError::MalformedSvg`]
/// with the icon pipeline, so a caller already showing one of those a
/// sentence needs nothing new.
pub fn render_svg_file(path: &Path) -> Result<RenderedIcon, IconError> {
    let bytes = fs::read(path).map_err(|err| IconError::UnreadableAsset {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    icon_theme::rasterise_svg(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_valid_svg_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.svg");
        fs::write(
            &path,
            br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#fff"/></svg>"##,
        )
        .unwrap();

        let icon = render_svg_file(&path).unwrap();
        assert_eq!((icon.width, icon.height), (10, 10));
    }

    #[test]
    fn a_missing_file_is_an_unreadable_asset_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.svg");
        assert!(matches!(
            render_svg_file(&missing),
            Err(IconError::UnreadableAsset { .. })
        ));
    }
}
