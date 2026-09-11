//! Read-only [`TabContent::Text`] tabs backed by a
//! [`editor_core::DocumentSource::Virtual`] document (C12): decompiled or
//! generated source with no backing file, fetched by `lsp_core`'s
//! `csharp/metadata` (`navigation.rs`'s `DefinitionOutcome::NeedsMetadataFetch`)
//! and handed here as already-resolved text — the actual LSP round trip is
//! `lsp-core`/`ui-shell`'s job, not this one's.
//!
//! No new `TabContent`/`TabKind` variant: a virtual document is still
//! fundamentally a text document (content, dirty tracking, language
//! detection via `tab_file_name` all already work), and
//! `editor_core::Document::is_read_only` is the one real difference from a
//! plain file tab — see `AppSession::tab_is_read_only`.
//!
//! Split out once `lib.rs` hit its ratcheted file-size ceiling, the same
//! reason `diff_tab.rs`/`file_ops.rs`/`tree_sort.rs`/`preview.rs` exist as
//! siblings rather than growing it further.

use editor_core::{Document, DocumentSource};

use crate::{AppSession, OpenedTab, TabContent, TabEntry, TabId, TabKind};

impl AppSession {
    /// Open a read-only virtual document (C12) — no disk read, `text`
    /// arrived over some other channel. Reopening the same `(scheme, key)`
    /// focuses the existing tab rather than duplicating it, mirroring
    /// [`AppSession::open_file`]'s focus-don't-duplicate rule (US-3) so
    /// re-navigating to the same decompiled symbol lands on the same tab.
    pub fn open_virtual_document(&mut self, scheme: &str, key: &str, text: &str) -> OpenedTab {
        if let Some(id) = self.find_tab_by_virtual(scheme, key) {
            self.active = Some(id);
            let title = self.tab_title(id).expect("tab found by key exists");
            return OpenedTab {
                id,
                title,
                newly_opened: false,
                kind: TabKind::Text,
            };
        }
        let doc = Document::open_virtual(scheme, key, text);
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let title = doc.title();
        self.docs.push(TabEntry {
            id,
            content: TabContent::Text(doc),
        });
        self.active = Some(id);
        OpenedTab {
            id,
            title,
            newly_opened: true,
            kind: TabKind::Text,
        }
    }

    /// Whether the tab is read-only — a
    /// [`editor_core::DocumentSource::Virtual`] text tab (C12), or a binary
    /// or diff tab. `None` for an unknown tab. The view uses this to
    /// disable editing/Save on a tab it would otherwise assume is a plain,
    /// writable text tab from `kind` alone.
    pub fn tab_is_read_only(&self, id: TabId) -> Option<bool> {
        match self.entry(id).map(|e| &e.content) {
            Some(TabContent::Text(doc)) => Some(doc.is_read_only()),
            Some(TabContent::Binary(_)) => Some(true),
            Some(TabContent::Diff(_)) => Some(true),
            Some(TabContent::Image(_)) => Some(true),
            None => None,
        }
    }

    /// The open tab for `(scheme, key)`, if any — the dedup/re-open key for
    /// a [`editor_core::DocumentSource::Virtual`] tab, the same role
    /// [`AppSession::find_tab_by_path`] plays for a file.
    fn find_tab_by_virtual(&self, scheme: &str, key: &str) -> Option<TabId> {
        self.docs
            .iter()
            .find(|e| match &e.content {
                TabContent::Text(doc) => {
                    doc.source()
                        == &DocumentSource::Virtual {
                            scheme: scheme.to_string(),
                            key: key.to_string(),
                        }
                }
                _ => false,
            })
            .map(|e| e.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppError;
    use std::fs;

    fn session_with_project() -> (tempfile::TempDir, tempfile::TempDir, AppSession) {
        let project_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        fs::write(project_dir.path().join("a.txt"), "alpha").unwrap();
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();
        (project_dir, config_dir, session)
    }

    #[test]
    fn open_virtual_document_opens_a_read_only_text_tab_with_no_path() {
        let mut session = AppSession::new();

        let opened =
            session.open_virtual_document("csharp", "metadata/Projects/x/Console.cs", "// stub");

        assert!(opened.newly_opened);
        assert_eq!(opened.kind, TabKind::Text);
        assert_eq!(opened.title, "Console.cs");
        assert_eq!(session.tab_kind(opened.id), Some(TabKind::Text));
        assert_eq!(session.tab_is_read_only(opened.id), Some(true));
        assert_eq!(session.tab_path(opened.id), None);
        assert_eq!(session.tab_content(opened.id).as_deref(), Some("// stub"));
        assert_eq!(
            session.tab_file_name(opened.id).as_deref(),
            Some("Console.cs")
        );
    }

    #[test]
    fn reopening_the_same_virtual_key_focuses_the_existing_tab_instead_of_duplicating() {
        let mut session = AppSession::new();
        let first = session.open_virtual_document("csharp", "metadata/Console.cs", "// v1");

        session.open_virtual_document("csharp", "metadata/Other.cs", "// other");
        assert_ne!(session.active_tab(), Some(first.id));

        // Re-navigating to the same decompiled symbol must land back on the
        // same tab, not fetch-and-open a duplicate — the LSP round trip
        // that produced `text` here is `navigation`'s concern, not this
        // one's, so it is deliberately re-supplied and deliberately ignored.
        let second = session.open_virtual_document("csharp", "metadata/Console.cs", "// stale");

        assert!(!second.newly_opened);
        assert_eq!(second.id, first.id);
        assert_eq!(session.active_tab(), Some(first.id));
        // The original fetch's text wins; a duplicate open is a focus, not
        // a silent re-fetch-and-replace.
        assert_eq!(session.tab_content(first.id).as_deref(), Some("// v1"));
    }

    #[test]
    fn a_different_scheme_with_the_same_key_is_a_distinct_tab() {
        let mut session = AppSession::new();
        let a = session.open_virtual_document("csharp", "same-key", "a");
        let b = session.open_virtual_document("other-scheme", "same-key", "b");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn a_virtual_tab_refuses_save_with_a_clear_error_and_stays_open() {
        let mut session = AppSession::new();
        let opened = session.open_virtual_document("csharp", "metadata/Console.cs", "// stub");

        // `save_tab` mirrors the widget content in before attempting to
        // write (same as any other tab, per ADR-0003), so the refusal
        // happens at the disk-write boundary, not the in-memory one — the
        // buffer legitimately holds "edited" afterward, same as a normal
        // save failure (`save_failure_leaves_dirty_flag_set` in
        // `editor_core`).
        let err = session.save_tab(opened.id, "edited").unwrap_err();
        assert_eq!(err.code(), AppError::CODE_SAVE);
        assert!(err.to_string().contains("read-only"));
        assert_eq!(session.tab_content(opened.id).as_deref(), Some("edited"));

        let err = session.save_buffer(opened.id).unwrap_err();
        assert_eq!(err.code(), AppError::CODE_SAVE);

        let err = session.reload_tab(opened.id).unwrap_err();
        assert_eq!(err.code(), AppError::CODE_RELOAD);

        // The tab must still exist — a refused save/reload is not a broken
        // or vanished tab.
        assert_eq!(session.tab_content(opened.id).as_deref(), Some("edited"));
    }

    #[test]
    fn a_binary_and_a_diff_tab_are_also_read_only() {
        let (project_dir, _config, mut session) = session_with_project();
        let binary = project_dir.path().join("blob.bin");
        fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let bin_tab = session.open_file(&binary).unwrap().id;
        assert_eq!(session.tab_is_read_only(bin_tab), Some(true));

        let diff_tab = session.open_diff_tab(
            project_dir.path().join("a.txt"),
            "left".into(),
            "right".into(),
            "one".into(),
            "two".into(),
        );
        assert_eq!(session.tab_is_read_only(diff_tab), Some(true));
    }

    #[test]
    fn a_plain_text_tab_is_not_read_only() {
        let (project_dir, _config, mut session) = session_with_project();
        let id = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap()
            .id;
        assert_eq!(session.tab_is_read_only(id), Some(false));
        assert_eq!(session.tab_is_read_only(TabId::from_raw(999)), None);
    }
}
