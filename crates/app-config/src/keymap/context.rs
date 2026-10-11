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
/// owning widget through `scopeShortcut` (or `scopedShortcutKey` for a key
/// filter), never window-wide; a test below holds the two lists equal.
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

    /// The view half of [`SCOPED_ACTIONS`] (#423): a scoped action Qt
    /// registered window-wide would make two actions sharing its key
    /// ambiguous, so neither fires. `ui-shell/cpp` registers a scoped action
    /// only through `scopeShortcut` or `scopedShortcutKey`, which cannot
    /// register one window-wide; this holds the ids those calls name equal
    /// to this table, scanning source text as
    /// `every_registered_cpp_action_has_a_keymap_row` does.
    #[test]
    fn every_scoped_action_is_registered_widget_scoped() {
        let cpp_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui-shell/cpp");
        let mut registered = std::collections::BTreeSet::new();
        let mut unreadable = Vec::new();
        for entry in std::fs::read_dir(&cpp_dir).expect("ui-shell/cpp is readable") {
            let path = entry.expect("readable ui-shell/cpp entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("cpp") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("readable .cpp file");
            // The helpers' own definitions pass `id` through.
            let defines_helpers = path
                .file_name()
                .is_some_and(|name| name == "keymap_page.cpp");
            for call in ["scopeShortcut(", "scopedShortcutKey("] {
                for arguments in call_arguments(&text, call) {
                    match string_literal(arguments) {
                        Some(id) => {
                            registered.insert(id.to_string());
                        }
                        None if defines_helpers => {}
                        None => unreadable.push(format!("{}: {call}{arguments})", path.display())),
                    }
                }
            }
        }
        assert!(
            unreadable.is_empty(),
            "scoped shortcut registrations whose id is not a QStringLiteral: {unreadable:#?}"
        );
        let scoped: std::collections::BTreeSet<String> = SCOPED_ACTIONS
            .iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(
            registered, scoped,
            "ui-shell/cpp's scopeShortcut/scopedShortcutKey ids must equal SCOPED_ACTIONS"
        );
    }

    /// The argument text of every `call` (a name ending in `(`) in `text`,
    /// up to its matching `)`.
    fn call_arguments<'a>(text: &'a str, call: &str) -> Vec<&'a str> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find(call) {
            rest = &rest[at + call.len()..];
            let mut depth = 1;
            let end = rest.char_indices().find_map(|(index, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(index)
            });
            let Some(end) = end else {
                break;
            };
            found.push(&rest[..end]);
        }
        found
    }

    /// The text of the first `QStringLiteral("…")` in `arguments`.
    fn string_literal(arguments: &str) -> Option<&str> {
        const LITERAL: &str = "QStringLiteral(\"";
        let start = arguments.find(LITERAL)? + LITERAL.len();
        let length = arguments[start..].find('"')?;
        Some(&arguments[start..start + length])
    }

    #[test]
    fn call_arguments_stop_at_the_matching_parenthesis() {
        let text = r#"a(scopeShortcut(new QAction(tr("x")), QStringLiteral("t.c"), r)); scopeShortcut(id)"#;
        let calls = call_arguments(text, "scopeShortcut(");
        assert_eq!(calls.len(), 2);
        assert_eq!(string_literal(calls[0]), Some("t.c"));
        assert_eq!(string_literal(calls[1]), None);
    }
}
