//! Window geometry and named layouts — what the view persists about the
//! shape of the window rather than about its contents.

use serde::{Deserialize, Serialize};

/// Window position and size, as last saved by the view (`QMainWindow`
/// geometry). Every field is individually defaulted so a TOML file that only
/// sets some of them still parses.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct WindowGeometry {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl WindowGeometry {
    /// Whether this geometry is worth persisting or restoring. A zero-sized
    /// rect is what the window reports while it is minimised or already torn
    /// down, and restoring it next launch would open a window nobody can see.
    pub fn is_usable(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// A named workspace arrangement: the dock layout and the editor split grid,
/// with no files in it. Applying a layout rearranges the workspace and leaves
/// the open documents where they are — which is the whole difference between
/// a layout and the implicit session state in [`Settings::window_state`] /
/// [`Settings::editor_layout`], and the reason this is a separate type rather
/// than a second copy of those two fields.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// Base64 `ads::CDockManager::saveState()`, same encoding and same reason
    /// as [`Settings::window_state`]: the blob is binary and this field is a
    /// Rust `String`, which must be valid UTF-8.
    #[serde(default)]
    pub window_state: String,
    /// The editor splitter tree as JSON, serialized by the view — the format
    /// [`Settings::editor_layout`] uses, with every group's files omitted.
    #[serde(default)]
    pub editor_grid: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_window_geometry_is_not_usable() {
        assert!(!WindowGeometry::default().is_usable());
        assert!(!WindowGeometry {
            x: 10,
            y: 10,
            width: 800,
            height: 0,
        }
        .is_usable());
        assert!(WindowGeometry {
            x: 10,
            y: 10,
            width: 800,
            height: 600,
        }
        .is_usable());
    }
}
