//! Shortcut contexts (ADR-0074): where an action's shortcut is live, and
//! which contexts can be active at once.

use std::collections::HashMap;

use super::{action, ActionDef};

/// The keymap rules a saved `Settings::keymap` was last migrated to; see
/// [`migrate_overrides`].
pub const KEYMAP_VERSION: u32 = 1;

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

/// Bring keymap `overrides` saved under rules older than `from` up to
/// [`KEYMAP_VERSION`].
///
/// Version 1 is this module's contexts. Before them every shortcut was
/// window-wide, so binding a key unbound (an `""` override) every other
/// action holding it, now also those in a context that never overlaps. Such
/// an unbinding gets its default back when that key's holder is in a
/// disjoint context and no overlapping action holds it; one with any other
/// cause, or none, stays the user's choice.
pub fn migrate_overrides(overrides: &mut HashMap<String, String>, from: u32) {
    if from >= 1 {
        return;
    }
    let restorable: Vec<String> = overrides
        .iter()
        .filter(|(id, key)| key.is_empty() && only_disjointly_taken(overrides, id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in restorable {
        overrides.remove(&id);
    }
}

/// Whether `id`'s default key is held, by override, only by actions in
/// contexts disjoint from `id`'s.
fn only_disjointly_taken(overrides: &HashMap<String, String>, id: &str) -> bool {
    let Some(own) = action(id).filter(|own| !own.default_shortcut.is_empty()) else {
        return false;
    };
    let holders: Vec<&ActionDef> = overrides
        .iter()
        .filter(|(other, key)| *other != id && key.as_str() == own.default_shortcut)
        .filter_map(|(other, _)| action(other))
        .collect();
    !holders.is_empty() && holders.iter().all(|holder| !own.overlaps(holder))
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

    #[test]
    fn an_unbinding_caused_only_by_a_now_disjoint_context_is_undone() {
        let overrides = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(id, key)| (id.to_string(), key.to_string()))
                .collect()
        };
        // Binding Jump to Source to F4 under the global rule stole the grid's.
        let mut stolen = overrides(&[
            ("tests.jumpToSource", "F4"),
            ("database.goToReferencedRow", ""),
        ]);
        migrate_overrides(&mut stolen, 0);
        assert_eq!(stolen, overrides(&[("tests.jumpToSource", "F4")]));
        let map = Keymap::from_overrides(stolen);
        assert_eq!(map.shortcut_for("database.goToReferencedRow"), "F4");

        // Taken by a global action: the unbinding is still needed.
        let global = overrides(&[("view.goToLine", "F4"), ("database.goToReferencedRow", "")]);
        let mut migrated = global.clone();
        migrate_overrides(&mut migrated, 0);
        assert_eq!(migrated, global);

        // Cleared with nothing taking the key: the user's choice.
        let cleared = overrides(&[("database.goToReferencedRow", "")]);
        let mut migrated = cleared.clone();
        migrate_overrides(&mut migrated, 0);
        assert_eq!(migrated, cleared);

        // Already migrated: left alone, whatever it holds.
        let mut current = overrides(&[
            ("tests.jumpToSource", "F4"),
            ("database.goToReferencedRow", ""),
        ]);
        migrate_overrides(&mut current, KEYMAP_VERSION);
        assert_eq!(current.len(), 2);
    }

    #[test]
    fn load_migrates_a_keymap_saved_before_shortcut_contexts() {
        let dir = tempfile::tempdir().unwrap();
        let old =
            "[keymap]\n\"tests.jumpToSource\" = \"F4\"\n\"database.goToReferencedRow\" = \"\"\n";
        std::fs::write(dir.path().join(crate::SETTINGS_FILE), old).unwrap();

        let loaded = crate::load(dir.path()).unwrap();

        assert!(!loaded.keymap.contains_key("database.goToReferencedRow"));

        // A keymap edited since keeps a deliberate unbinding.
        let mut edited = loaded.clone();
        let mut map = edited.keymap();
        map.assign("database.goToReferencedRow", "");
        edited.set_keymap(map);
        crate::save(dir.path(), &edited).unwrap();
        let reloaded = crate::load(dir.path()).unwrap();
        assert_eq!(reloaded.keymap["database.goToReferencedRow"], "");
    }
}
