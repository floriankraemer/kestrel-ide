//! Rasterise an SVG into the 32x32 raw alpha mask `ui-shell`'s `maskIcon`
//! tints at runtime (`crates/ui-shell/resources/icons/**.a8`).
//!
//! The app has no Qt6Svg, so every glyph it draws in code ships as a mask
//! that was rasterised ahead of time. This is the one tool that produces
//! them — keep the SVG next to each `.a8` it made, the way
//! `resources/icons/symbols/` does.
//!
//! ```sh
//! cargo run -p icon-theme --example svg_to_a8 -- in.svg out.a8
//! ```

use std::process::ExitCode;

use resvg::{tiny_skia, usvg};

const SIDE: u32 = 32;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = args.as_slice() else {
        eprintln!("usage: svg_to_a8 <in.svg> <out.a8>");
        return ExitCode::FAILURE;
    };
    match convert(input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("svg_to_a8: {message}");
            ExitCode::FAILURE
        }
    }
}

fn convert(input: &str, output: &str) -> Result<(), String> {
    let svg = std::fs::read(input).map_err(|e| format!("reading {input}: {e}"))?;
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
        .map_err(|e| format!("parsing {input}: {e}"))?;
    let mut pixmap = tiny_skia::Pixmap::new(SIDE, SIDE).ok_or("allocating the pixmap")?;
    let size = tree.size();
    let scale = (SIDE as f32 / size.width()).min(SIDE as f32 / size.height());
    let dx = (SIDE as f32 - size.width() * scale) / 2.0;
    let dy = (SIDE as f32 - size.height() * scale) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // Premultiplied RGBA in, one alpha byte per pixel out — the shape is the
    // coverage; the colour comes from whoever tints the mask.
    let alpha: Vec<u8> = pixmap.pixels().iter().map(|p| p.alpha()).collect();
    std::fs::write(output, alpha).map_err(|e| format!("writing {output}: {e}"))
}
