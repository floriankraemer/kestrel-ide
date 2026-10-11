//! Shortcut contexts (ADR-0074): where an action's shortcut is live, and
//! which contexts can be active at once.

use super::ActionDef;

/// Where an action's shortcut is live (ADR-0074): everywhere in the main
/// window, or only while focus is inside one kind of widget. Two actions may
/// share a key only when their contexts never overlap, because Qt fires
/// neither of two shortcuts that are active at once ("ambiguous").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutContext {
    /// A window-wide shortcut: a menu action, or a widget's own window-scoped
    /// action.
    Global,
    /// The editor area: focus in a code editor.
    Editor,
    /// The terminal dock: focus in a terminal tab.
    Terminal,
    /// The Tests dock's result tree.
    TestsTree,
    /// A database result grid.
    DataGrid,
}

impl ShortcutContext {
    /// Whether a shortcut in `self` and one in `other` can be active at the
    /// same time. A global shortcut is active wherever focus is; the scoped
    /// contexts are disjoint widget trees.
    pub fn overlaps(self, other: Self) -> bool {
        self == other || self == Self::Global || other == Self::Global
    }
}

/// The actions whose shortcut is scoped to a widget; every other action is
/// [`ShortcutContext::Global`]. The view registers each of these on its
/// owning widget (`Qt::WidgetWithChildrenShortcut`, or a key filter), never
/// window-wide.
const SCOPED_ACTIONS: &[(&str, ShortcutContext)] = &[
    ("navigate.goToTest", ShortcutContext::Editor),
    ("terminal.copy", ShortcutContext::Terminal),
    ("terminal.paste", ShortcutContext::Terminal),
    ("terminal.newSession", ShortcutContext::Terminal),
    ("terminal.selectShell", ShortcutContext::Terminal),
    ("tests.jumpToSource", ShortcutContext::TestsTree),
    ("database.goToReferencedRow", ShortcutContext::DataGrid),
];

impl ActionDef {
    /// Where this action's shortcut is live.
    pub fn context(&self) -> ShortcutContext {
        SCOPED_ACTIONS
            .iter()
            .find(|(id, _)| *id == self.id)
            .map_or(ShortcutContext::Global, |(_, context)| *context)
    }

    /// Whether this action's shortcut and `other`'s can be active at once.
    pub(super) fn overlaps(&self, other: &ActionDef) -> bool {
        self.context().overlaps(other.context())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::{action, Keymap};
    use super::*;

    fn keymap() -> Keymap {
        Keymap::default()
    }

    #[test]
    fn every_scoped_action_exists() {
        for (id, _) in SCOPED_ACTIONS {
            assert!(action(id).is_some(), "{id} is not in ACTIONS");
        }
    }

    #[test]
    fn a_global_context_overlaps_every_context_and_scoped_ones_only_themselves() {
        use ShortcutContext::*;
        assert!(Global.overlaps(Terminal));
        assert!(DataGrid.overlaps(Global));
        assert!(Editor.overlaps(Editor));
        assert!(!Editor.overlaps(Terminal));
        assert!(!TestsTree.overlaps(DataGrid));
    }

    #[test]
    fn jetbrains_keys_are_shared_by_contexts_that_never_overlap() {
        let map = keymap();
        assert_eq!(map.shortcut_for("navigate.goToTest"), "Ctrl+Shift+T");
        assert_eq!(map.shortcut_for("terminal.newSession"), "Ctrl+Shift+T");
        assert_eq!(map.shortcut_for("tests.jumpToSource"), "F4");
        assert_eq!(map.shortcut_for("database.goToReferencedRow"), "F4");
        assert_eq!(
            action("navigate.goToTest").map(ActionDef::context),
            Some(ShortcutContext::Editor)
        );
    }

    #[test]
    fn a_scoped_rebind_conflicts_with_global_and_same_context_actions_only() {
        let map = keymap();
        // Ctrl+Shift+F is global: it is live in the terminal too.
        let ids = |conflicts: Vec<&ActionDef>| conflicts.iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(
            ids(map.conflicts("terminal.selectShell", "Ctrl+Shift+F")),
            ["edit.findInFiles"]
        );
        assert_eq!(
            ids(map.conflicts("terminal.selectShell", "Ctrl+Shift+T")),
            ["terminal.newSession"]
        );
        assert!(map
            .conflicts("tests.jumpToSource", "Ctrl+Shift+T")
            .is_empty());
        // A global rebind is live everywhere, so it clashes with both.
        assert_eq!(
            ids(map.conflicts("view.goToLine", "Ctrl+Shift+T")),
            ["navigate.goToTest", "terminal.newSession"]
        );
    }

    #[test]
    fn stealing_a_scoped_key_leaves_the_other_contexts_binding_alone() {
        let mut map = keymap();
        map.assign("terminal.selectShell", "Ctrl+Shift+T");
        assert_eq!(map.shortcut_for("terminal.newSession"), "");
        assert_eq!(map.shortcut_for("navigate.goToTest"), "Ctrl+Shift+T");

        // An override equal to a default hides it only in an overlapping context.
        let overridden = |id: &str| {
            Keymap::from_overrides(HashMap::from([(
                id.to_string(),
                "Ctrl+Shift+T".to_string(),
            )]))
        };
        let scoped = overridden("tests.jumpToSource");
        assert_eq!(scoped.shortcut_for("navigate.goToTest"), "Ctrl+Shift+T");
        assert_eq!(scoped.shortcut_for("terminal.newSession"), "Ctrl+Shift+T");
        let global = overridden("view.goToLine");
        assert_eq!(global.shortcut_for("navigate.goToTest"), "");
        assert_eq!(global.shortcut_for("terminal.newSession"), "");
    }

    #[test]
    fn an_unknown_id_conflicts_with_nothing() {
        assert!(keymap().conflicts("gone.action", "Ctrl+S").is_empty());
    }
}
