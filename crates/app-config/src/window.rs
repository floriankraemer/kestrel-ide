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

    /// Where the window should open given the screens' available rects (logical
    /// pixels, primary first).
    ///
    /// A usable saved rect whose title strip is still reachable on some screen
    /// is kept: shrunk and nudged to fit the screen showing most of the strip
    /// (monitors left of or above the primary have negative coordinates and are
    /// legitimate), or untouched when all four corners lie on screens, as for a
    /// window spanning two monitors. Anything else
    /// — nothing saved, a rect from a monitor that is gone, a stale off-screen
    /// position — opens at the default size centred on the primary screen.
    ///
    /// The rect is in client-area coordinates, so `title_inset` (the native
    /// frame's top margin) is taken off the top of every screen: a window
    /// placed at a screen's top edge then keeps its title bar on screen.
    pub fn placed_within(&self, screens: &[WindowGeometry], title_inset: u32) -> WindowGeometry {
        let screens: Vec<WindowGeometry> =
            screens.iter().map(|s| s.below_title(title_inset)).collect();
        let screens = &screens[..];
        let Some(primary) = screens.iter().find(|s| s.is_usable()) else {
            return self.clone();
        };
        if self.is_usable() {
            let best = screens
                .iter()
                .map(|s| (s.title_strip_width_of(self), s))
                .filter(|(width, _)| *width >= i64::from(self.width.min(MIN_GRABBABLE_WIDTH)))
                .max_by_key(|(width, _)| *width);
            if let Some((_, screen)) = best {
                // A window spanning monitors stays where the user put it.
                let spans_screens = self
                    .corners()
                    .iter()
                    .all(|(x, y)| screens.iter().any(|s| s.contains(*x, *y)));
                return if spans_screens {
                    self.clone()
                } else {
                    self.fitted_into(screen)
                };
            }
        }
        primary.default_window_centred()
    }

    fn below_title(&self, inset: u32) -> WindowGeometry {
        let inset = inset.min(self.height);
        WindowGeometry {
            x: self.x,
            y: self.y.saturating_add(inset as i32),
            width: self.width,
            height: self.height - inset,
        }
    }

    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// How much of `window`'s title bar lies on this screen: its overlap
    /// width, or 0 when the strip is not vertically inside the screen.
    fn title_strip_width_of(&self, window: &WindowGeometry) -> i64 {
        let visible_width = window.right().min(self.right()) - i64::from(window.x.max(self.x));
        let strip_top = i64::from(window.y);
        if strip_top >= i64::from(self.y) && strip_top + TITLE_STRIP_HEIGHT <= self.bottom() {
            visible_width.max(0)
        } else {
            0
        }
    }

    fn contains(&self, x: i64, y: i64) -> bool {
        (i64::from(self.x)..self.right()).contains(&x)
            && (i64::from(self.y)..self.bottom()).contains(&y)
    }

    /// The four outermost pixels of the rect.
    fn corners(&self) -> [(i64, i64); 4] {
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        let (right, bottom) = (self.right() - 1, self.bottom() - 1);
        [(left, top), (right, top), (left, bottom), (right, bottom)]
    }

    fn fitted_into(&self, screen: &WindowGeometry) -> WindowGeometry {
        let width = self.width.min(screen.width);
        let height = self.height.min(screen.height);
        let x = i64::from(self.x).clamp(i64::from(screen.x), screen.right() - i64::from(width));
        let y = i64::from(self.y).clamp(i64::from(screen.y), screen.bottom() - i64::from(height));
        WindowGeometry {
            x: x as i32,
            y: y as i32,
            width,
            height,
        }
    }

    fn default_window_centred(&self) -> WindowGeometry {
        let width = DEFAULT_WIDTH.min(self.width * 9 / 10);
        let height = DEFAULT_HEIGHT.min(self.height * 9 / 10);
        WindowGeometry {
            x: self.x + ((self.width - width) / 2) as i32,
            y: self.y + ((self.height - height) / 2) as i32,
            width,
            height,
        }
    }
}

const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 800;
/// Height of the band at the top of the window that must stay on a screen so
/// the user can still drag it.
const TITLE_STRIP_HEIGHT: i64 = 32;
const MIN_GRABBABLE_WIDTH: u32 = 100;

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

    fn rect(x: i32, y: i32, width: u32, height: u32) -> WindowGeometry {
        WindowGeometry {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_fresh_config_opens_centred_and_inside_a_small_screen() {
        let placed = WindowGeometry::default().placed_within(&[rect(0, 0, 1706, 912)], 0);
        assert_eq!(placed, rect(213, 56, 1280, 800));
        let tiny = WindowGeometry::default().placed_within(&[rect(0, 0, 1000, 600)], 0);
        assert_eq!(tiny, rect(50, 30, 900, 540));
    }

    #[test]
    fn a_stale_off_screen_rect_falls_back_to_the_default() {
        let saved = rect(-1288, 100, 1280, 800);
        let placed = saved.placed_within(&[rect(0, 0, 1706, 912)], 0);
        assert_eq!(placed, rect(213, 56, 1280, 800));
    }

    #[test]
    fn a_rect_from_a_disconnected_monitor_falls_back_to_the_default() {
        let saved = rect(2000, 50, 1000, 700);
        let placed = saved.placed_within(&[rect(0, 0, 1920, 1040)], 0);
        assert_eq!(placed, rect(320, 120, 1280, 800));
    }

    #[test]
    fn a_rect_larger_than_the_screen_is_shrunk_to_fit() {
        let saved = rect(-50, 0, 3000, 2000);
        let placed = saved.placed_within(&[rect(0, 0, 1920, 1040)], 0);
        assert_eq!(placed, rect(0, 0, 1920, 1040));
    }

    #[test]
    fn a_valid_rect_on_a_screen_left_of_the_primary_is_kept() {
        let saved = rect(-1800, 100, 1200, 700);
        let screens = [rect(0, 0, 1920, 1040), rect(-1920, 0, 1920, 1040)];
        assert_eq!(saved.placed_within(&screens, 0), saved);
    }

    #[test]
    fn a_window_spanning_two_monitors_stays_put() {
        let screens = [rect(0, 0, 1920, 1040), rect(1920, 0, 1920, 1040)];
        let saved = rect(1800, 100, 1200, 700);
        assert_eq!(saved.placed_within(&screens, 0), saved);
    }

    #[test]
    fn a_window_hanging_off_the_edge_of_two_monitors_fits_the_one_showing_most_of_it() {
        let screens = [rect(0, 0, 1920, 1040), rect(1920, 0, 1920, 1040)];
        let saved = rect(1800, 900, 1200, 700);
        assert_eq!(saved.placed_within(&screens, 0), rect(1920, 340, 1200, 700));
        let mostly_left = rect(1000, 900, 1200, 700);
        assert_eq!(
            mostly_left.placed_within(&screens, 0),
            rect(720, 340, 1200, 700)
        );
    }

    #[test]
    fn a_rect_hanging_off_the_edge_is_nudged_back_on_screen() {
        let saved = rect(1500, 900, 800, 600);
        let placed = saved.placed_within(&[rect(0, 0, 1920, 1040)], 0);
        assert_eq!(placed, rect(1120, 440, 800, 600));
    }

    #[test]
    fn a_title_bar_above_the_screen_is_not_reachable() {
        let saved = rect(100, -200, 800, 600);
        let placed = saved.placed_within(&[rect(0, 0, 1920, 1040)], 0);
        assert_eq!(placed, rect(320, 120, 1280, 800));
    }

    #[test]
    fn a_rect_whose_title_bar_hangs_above_the_screen_is_not_kept() {
        let screens = [rect(0, 0, 1920, 1040)];
        let flush = rect(100, 31, 800, 600);
        assert_eq!(flush.placed_within(&screens, 31), flush);
        let clipped = rect(100, 0, 800, 600);
        assert_eq!(
            clipped.placed_within(&screens, 31),
            rect(320, 135, 1280, 800)
        );
    }

    #[test]
    fn the_default_is_centred_within_the_area_below_the_title_bar() {
        let placed = WindowGeometry::default().placed_within(&[rect(0, 0, 1920, 1040)], 40);
        assert_eq!(placed, rect(320, 140, 1280, 800));
    }

    #[test]
    fn without_any_screen_the_saved_rect_is_returned_unchanged() {
        let saved = rect(5, 5, 600, 400);
        assert_eq!(saved.placed_within(&[], 0), saved);
    }
}
