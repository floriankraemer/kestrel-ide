//! Rust side of the `IconProvider` QObject: the seam icons cross.
//!
//! Two slots and no state of its own beyond the shared handles. Which icon a
//! row gets, and what its pixels are, is `app_core::icons`' answer; this
//! translates a path to a `QString` and pixels to a `QByteArray`, and
//! decides nothing.
//!
//! Deliberately usable without a model: the project tree reaches icons
//! through a role and a proxy model, but editor tabs and the search result
//! lists (P6) have no model to hang a role on and call these two slots
//! directly.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use app_core::AppSession;
use cxx_qt_lib::{QByteArray, QString};

use crate::bridge::ffi;
use crate::bridge::registry::{shared_color_themes, shared_icons, shared_session, SharedIcons};

/// Handles on the process-wide icon theme and session, nothing more.
pub struct IconProviderRust {
    icons: Rc<SharedIcons>,
    /// Only ever read for the open project's root path — see
    /// [`ffi::IconProvider::icon_key_for_path`].
    session: Rc<RefCell<AppSession>>,
}

impl Default for IconProviderRust {
    fn default() -> Self {
        Self {
            icons: shared_icons(),
            session: shared_session(),
        }
    }
}

impl ffi::IconProvider {
    /// The icon key for one row, or an empty string when no icon theme is
    /// active.
    ///
    /// Empty rather than a sentinel with a meaning: the only thing the view
    /// does with a key is look it up, and an empty one means "no
    /// decoration" — which is what makes a row with no icon reserve no icon
    /// width.
    pub fn icon_key_for_path(&self, path: &QString, is_dir: bool, expanded: bool) -> QString {
        let path = path.to_string();
        let path = Path::new(&path);
        // Whether a row *is* the project root is a fact about the open
        // session rather than a rule — what a root then looks like is the
        // pack's answer, made in `app-core`.
        let is_root = is_dir && self.session.borrow().root_path() == Some(path);
        match self.icons.service.borrow().icon_key(
            path,
            is_dir,
            expanded,
            is_root,
            self.icons.appearance.get(),
        ) {
            Some(key) => QString::from(key.as_str()),
            None => QString::default(),
        }
    }

    /// The icon key for a Build Tools row: the rule is
    /// `app_core::build_tools_icons::row_icon`, this only translates the
    /// enums and asks the shared service.
    pub fn icon_key_for_build_tool_row(
        &self,
        kind: ffi::FfiBuildToolNodeKind,
        tool: &QString,
        source_role: ffi::FfiBuildToolSourceRole,
        group_kind: ffi::FfiBuildToolGroupKind,
    ) -> QString {
        use app_core::build_tools_tree::FolderRole;
        use jvm_build_core::view::NodeKind;
        let kind = match kind {
            ffi::FfiBuildToolNodeKind::ToolRoot => NodeKind::ToolRoot,
            ffi::FfiBuildToolNodeKind::Group => NodeKind::Group,
            ffi::FfiBuildToolNodeKind::Task => NodeKind::Task,
            ffi::FfiBuildToolNodeKind::Module => NodeKind::Module,
            ffi::FfiBuildToolNodeKind::SourceRoot => NodeKind::SourceRoot,
            ffi::FfiBuildToolNodeKind::Dependency => NodeKind::Dependency,
            ffi::FfiBuildToolNodeKind::Profile => NodeKind::Profile,
            ffi::FfiBuildToolNodeKind::Plugin => NodeKind::Plugin,
            ffi::FfiBuildToolNodeKind::Goal => NodeKind::Goal,
            _ => return QString::default(),
        };
        let role = match source_role {
            ffi::FfiBuildToolSourceRole::Main => Some(FolderRole::Main),
            ffi::FfiBuildToolSourceRole::Test => Some(FolderRole::Test),
            ffi::FfiBuildToolSourceRole::Resource => Some(FolderRole::Resource),
            _ => None,
        };
        let group = {
            use ffi::FfiBuildToolGroupKind as F;
            use jvm_build_core::view::GroupKind as G;
            match group_kind {
                F::Tasks => Some(G::Tasks),
                F::TaskGroup => Some(G::TaskGroup),
                F::Lifecycle => Some(G::Lifecycle),
                F::Plugins => Some(G::Plugins),
                F::Modules => Some(G::Modules),
                F::Dependencies => Some(G::Dependencies),
                F::DependencyModule => Some(G::DependencyModule),
                F::Configuration => Some(G::Configuration),
                F::Profiles => Some(G::Profiles),
                _ => None,
            }
        };
        let Some(tool) = jvm_build_core::model::Tool::from_toolchain_id(&tool.to_string()) else {
            return QString::default();
        };
        self.pack_key(app_core::build_tools_icons::row_icon(
            kind, tool, role, group,
        ))
    }

    /// The icon key for a pack-drawn Build Tools toolbar button.
    pub fn icon_key_for_build_toolbar(&self, button: ffi::FfiBuildToolbarIcon) -> QString {
        use app_core::build_tools_icons::ToolbarIcon;
        let button = match button {
            ffi::FfiBuildToolbarIcon::Settings => ToolbarIcon::Settings,
            ffi::FfiBuildToolbarIcon::SkipTests => ToolbarIcon::SkipTests,
            _ => return QString::default(),
        };
        self.pack_key(Some(app_core::build_tools_icons::toolbar_icon(button)))
    }

    fn pack_key(&self, icon: Option<app_core::build_tools_icons::PackIcon>) -> QString {
        icon.and_then(|icon| {
            self.icons
                .service
                .borrow()
                .pack_icon_key(&icon, self.icons.appearance.get())
        })
        .map(|key| QString::from(key.as_str()))
        .unwrap_or_default()
    }

    /// Every icon theme the loaded plugins offer, for the Appearance page's
    /// combo.
    pub fn icon_themes(&self) -> Vec<ffi::FfiIconTheme> {
        app_core::icons::icon_themes(&plugin_host::registry())
            .into_iter()
            .map(|choice| ffi::FfiIconTheme {
                id: QString::from(choice.id.as_str()),
                label: QString::from(choice.label.as_str()),
            })
            .collect()
    }

    /// Switch the icon theme without persisting anything — the Appearance
    /// page's live preview, and its Cancel path.
    ///
    /// Rebuilt over the registry that is already loaded rather than through
    /// a rescan: nothing about the plugins on disk has changed, only which
    /// of their contributions is being drawn.
    pub fn apply_icon_theme(&self, id: &QString) {
        *self.icons.service.borrow_mut() =
            app_core::icons::IconService::from_registry(plugin_host::registry(), &id.to_string());
    }

    /// Re-read which art the colour theme wants, so a light theme switched
    /// on in the same dialog gets the pack's light variants.
    ///
    /// The theme name itself is not read here (T7): every call site
    /// (`appearance_page.cpp`'s `applyThemeLive`) calls `applyTheme(name)`
    /// first, which resolves `name` through `ThemeProvider` before this
    /// slot ever runs — so the shared colour-theme service's active
    /// appearance is already the answer for `name`, mapped through
    /// `app_core::icons::icon_appearance` (the one allowed conversion point
    /// between `color_theme::Appearance` and `icon_theme::Appearance`).
    pub fn apply_color_theme(&self, _theme_name: &QString) {
        let appearance = shared_color_themes()
            .borrow()
            .active()
            .map(|theme| app_core::icons::icon_appearance(theme.appearance))
            .unwrap_or(app_core::icons::Appearance::Dark);
        self.icons.appearance.set(appearance);
    }

    /// Premultiplied RGBA8 for `key` at `px` by `px`, or an empty
    /// `QByteArray` when there is nothing to draw.
    ///
    /// The format is not negotiable: these bytes are wrapped in a
    /// `QImage::Format_RGBA8888_Premultiplied` on the other side, which is
    /// tiny-skia's byte order exactly. `Format_ARGB32_Premultiplied` is
    /// BGRA on little-endian and would turn every icon's red and blue
    /// around.
    pub fn icon_pixels(&self, key: &QString, px: u32) -> QByteArray {
        match self
            .icons
            .service
            .borrow_mut()
            .icon_pixels(&key.to_string(), px)
        {
            Some(pixels) => QByteArray::from(pixels.as_slice()),
            None => QByteArray::default(),
        }
    }
}
