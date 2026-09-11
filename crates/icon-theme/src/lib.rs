//! Per-file-type icons: which icon a row gets, and what its pixels are.
//!
//! Two halves that are deliberately separable, because they cost very
//! different amounts:
//!
//! * [`IconPack`] — `pack.toml` and the resolution order over it. Pure data
//!   and cheap enough to run per visible row on every repaint.
//! * [`IconRenderer`] — `resvg` rasterisation to premultiplied RGBA8,
//!   memoised by `(pack id, icon id, px)`. Expensive, and run once per
//!   distinct icon and size.
//!
//! ## What this crate does not know
//!
//! **Which language a file is.** [`IconPack::file_icon`] takes an
//! already-resolved `language_id: Option<&str>` rather than depending on
//! `syntax-core`, because [ADR-0018] makes that crate's registry the single
//! source of file-to-language detection and a second extension table here
//! would be exactly the thing that decision forbids. The caller detects and
//! passes the id in.
//!
//! **Where a plugin's files are.** A built-in plugin's SVGs are embedded in
//! the binary and an installed plugin's are on disk, so reading one is the
//! caller's job through [`IconAssets`]. This crate also does not depend on
//! `plugin-host`: per [ADR-0026] a contribution is data, and the host and
//! its consumers are joined in `app-core`, not wired to each other.
//!
//! [ADR-0018]: ../../../docs/architecture/decisions/0018-single-source-language-detection.md
//! [ADR-0026]: ../../../docs/architecture/decisions/0026-plugin-host.md

mod pack;
mod render;

use std::fmt;

pub use pack::{Appearance, IconPack, ICONS_DIR};
pub use render::{rasterise_svg, IconAssets, IconRenderer, RenderedIcon};

/// Why an icon could not be produced.
///
/// Typed rather than a formatted string, for the same reason
/// `plugin_api::LoadErrorKind` is: the Plugins page groups by cause and
/// offers a different action per group, and it never prints a Rust error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconError {
    /// `pack.toml` is not valid TOML, or has the wrong shape.
    MalformedPack(String),
    /// A required field of `pack.toml` is present but empty.
    EmptyField(&'static str),
    /// An asset could not be read. `path` is relative to the pack.
    UnreadableAsset { path: String, message: String },
    /// The SVG parsed by `usvg` was rejected.
    MalformedSvg { icon: String, message: String },
    /// A rasterisation was asked for at a size no pixmap can hold.
    UnsupportedSize(u32),
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedPack(message) => write!(f, "malformed pack.toml: {message}"),
            Self::EmptyField(field) => write!(f, "pack.toml field {field} is empty"),
            Self::UnreadableAsset { path, message } => {
                write!(f, "cannot read icon asset {path}: {message}")
            }
            Self::MalformedSvg { icon, message } => {
                write!(f, "icon {icon} is not usable SVG: {message}")
            }
            Self::UnsupportedSize(px) => write!(f, "{px} is not a usable icon size"),
        }
    }
}

impl std::error::Error for IconError {}
