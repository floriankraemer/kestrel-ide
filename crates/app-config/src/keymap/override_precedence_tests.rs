//! A persisted override keeps its key over a default shipped later.

use std::collections::HashMap;

use super::*;

#[test]
fn a_user_override_wins_over_a_default_added_later_with_the_same_key() {
    for shortcut in [
        "Ctrl+N",
        "Ctrl+J",
        "Ctrl+Shift+B",
        "Alt+1",
        "Ctrl+Alt+Insert",
        "Alt+Insert",
        "Ctrl+Alt+T",
    ] {
        let owner = ACTIONS
            .iter()
            .find(|a| a.default_shortcut == shortcut)
            .unwrap_or_else(|| panic!("{shortcut} is a shipped default"))
            .id;
        // The user bound the key to an action that ships unbound.
        let theirs = ACTIONS
            .iter()
            .find(|a| a.default_shortcut.is_empty())
            .expect("an unbound action to hold the override")
            .id;
        let map =
            Keymap::from_overrides(HashMap::from([(theirs.to_string(), shortcut.to_string())]));
        assert_eq!(map.shortcut_for(theirs), shortcut);
        assert_eq!(map.shortcut_for(owner), "", "{shortcut}: default unbound");
        assert_eq!(
            map.bindings()
                .iter()
                .filter(|b| b.shortcut == shortcut)
                .count(),
            1,
            "{shortcut} triggers exactly one action"
        );
    }
}

#[test]
fn an_unrelated_override_leaves_defaults_alone() {
    let map = Keymap::from_overrides(HashMap::from([(
        "file.save".to_string(),
        "Ctrl+Alt+S".to_string(),
    )]));
    assert_eq!(map.shortcut_for("view.goToClass"), "Ctrl+N");
}
