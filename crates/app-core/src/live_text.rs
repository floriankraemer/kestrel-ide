//! The rope catching up with the view's unsaved typing (ADR-0003 keeps
//! keystrokes out of the rope): readers such as MCP `read_buffer` and the
//! agent tools pull the widget's text on demand, only for dirty tabs.

use crate::{AppSession, TabId};

impl AppSession {
    /// Whether a read of `id` must first pull the widget's text: only a
    /// dirty tab has typing the rope has not seen, so clean tabs (and
    /// unknown ones) cost nothing.
    pub fn tab_needs_live_text(&self, id: TabId) -> bool {
        self.tab_is_dirty(id) == Some(true)
    }

    /// Store the widget's live text in the rope without touching the dirty
    /// flag: the view already reported that (`set_tab_dirty`). Returns
    /// whether the tab is a text tab.
    pub fn sync_tab_text(&mut self, id: TabId, text: &str) -> bool {
        match self.text_doc_mut(id) {
            Ok(doc) => {
                let dirty = doc.is_dirty();
                doc.replace_content(text);
                doc.set_dirty(dirty);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn sync_tab_text_catches_the_rope_up_without_changing_dirty_state() {
        let project_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = project_dir.path().join("a.txt");
        fs::write(&path, "alpha").unwrap();
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();
        let tab = session.open_file(&path).unwrap();

        assert!(!session.tab_needs_live_text(tab.id));
        session.set_tab_dirty(tab.id, true);
        assert!(session.tab_needs_live_text(tab.id));

        assert!(session.sync_tab_text(tab.id, "typed, never saved"));
        assert_eq!(
            session.content_for_path(&path).as_deref(),
            Some("typed, never saved")
        );
        assert_eq!(session.tab_is_dirty(tab.id), Some(true));
        assert_eq!(fs::read_to_string(&path).unwrap(), "alpha");

        session.set_tab_dirty(tab.id, false);
        assert!(session.sync_tab_text(tab.id, "clean sync"));
        assert_eq!(session.tab_is_dirty(tab.id), Some(false));
        assert!(!session.sync_tab_text(TabId::from_raw(999), "x"));
    }
}
