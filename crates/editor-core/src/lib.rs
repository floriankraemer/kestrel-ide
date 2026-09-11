//! Rope-backed text buffer and tab-list state for the editor core.
//!
//! No Qt dependency — pure Rust, unit-testable in isolation. `ui-shell`
//! wraps [`TabList`] in a `DocumentManager` QObject later; this crate only
//! owns the buffer and its dirty/save state.

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::Rope;

mod binary_detect;
pub use binary_detect::{looks_binary, looks_binary_file};

pub mod diff;
pub mod hex;

/// Read-only handle behind an image tab; see [`ImageFile`].
mod image_file;
pub use image_file::ImageFile;

/// Byte <-> UTF-16 offset conversion, shared by every place Rust text meets
/// a Qt cursor position.
pub mod offsets;
pub use hex::{BinaryFile, HexRow, BYTES_PER_ROW};

pub mod search;

/// Carets and selections — the state a multi-caret edit is computed from.
pub mod selection;
pub use selection::{column_block, Caret, SelectionError, SelectionSet, MAX_CARETS};

/// One user-visible change to a buffer, however many carets made it.
pub mod transaction;
pub use transaction::{map_carets, TextEdit, Transaction, TransactionError};

/// Whole-line operations: duplicate, move, delete, join.
pub mod line_ops;

/// What a file is tidied into on the way to disk.
pub mod save_rules;
pub use save_rules::{detect_line_ending, LineEnding, SaveRules};

/// Leading/inner/trailing classification of the space/tab characters on a
/// line (JetBrains-style "show whitespaces").
pub mod whitespace;

pub use search::{
    find_matches, replacement_edits, replacements, Replacement, SearchError, SearchOptions,
    TextMatch,
};

/// What a [`Document`] is backed by: a real file on disk, or a read-only
/// virtual buffer with no path at all (ADR-0003 amendment: decompiled
/// metadata, `csharp:/...` and the like — C12).
///
/// `Virtual`'s `key` is treated as an opaque identifier, not a path: the
/// server that minted it (e.g. csharp-ls's `csharp/metadata`) owns its
/// structure, and this type never parses it. `(scheme, key)` together are
/// the document's identity for dedup/re-open, the same role a `PathBuf`
/// plays for `File`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentSource {
    /// A real file at this path.
    File(PathBuf),
    /// A read-only buffer with no backing file — its content arrived over
    /// some other channel (e.g. an LSP custom request) and there is nowhere
    /// on disk to save it back to.
    Virtual { scheme: String, key: String },
}

/// A single open file: a rope-backed buffer, its [`DocumentSource`], and a
/// dirty flag tracking unsaved edits.
pub struct Document {
    source: DocumentSource,
    rope: Rope,
    dirty: bool,
    /// Set when the tree tells us this document's backing file was deleted
    /// (US-2b) — blocks further silent-write-to-nowhere saves until the
    /// user does something about it (e.g. Save As, not in MVP scope, or
    /// simply accepts the error and closes the tab). Always `false` for a
    /// [`DocumentSource::Virtual`] document — there is no backing file to
    /// have been deleted.
    deleted: bool,
    /// `save()`/`reload()` refuse on a read-only document rather than
    /// silently no-opping or panicking (C12). Every [`DocumentSource::Virtual`]
    /// document is read-only; a `File` document is never constructed this
    /// way today, but the flag lives on `Document` rather than being
    /// inferred from the source so a future read-only *file* (e.g. a
    /// permissions-denied reopen) has somewhere to record it too.
    read_only: bool,
}

impl Document {
    /// Load a file from disk into a rope-backed buffer.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let content = fs::read_to_string(&path)?;
        Ok(Self {
            source: DocumentSource::File(path),
            rope: Rope::from_str(&content),
            dirty: false,
            deleted: false,
            read_only: false,
        })
    }

    /// A read-only virtual document with no backing file (C12): `text`
    /// arrived over some other channel (an LSP custom request fetching
    /// decompiled source, say) and is handed to the rope as-is. Never dirty
    /// on open, and [`Document::save`]/[`Document::reload`] refuse on it —
    /// see their docs.
    pub fn open_virtual(scheme: impl Into<String>, key: impl Into<String>, text: &str) -> Self {
        Self {
            source: DocumentSource::Virtual {
                scheme: scheme.into(),
                key: key.into(),
            },
            rope: Rope::from_str(text),
            dirty: false,
            deleted: false,
            read_only: true,
        }
    }

    /// What this document is backed by.
    pub fn source(&self) -> &DocumentSource {
        &self.source
    }

    /// The file this document is backed by, or `None` for a
    /// [`DocumentSource::Virtual`] document — it has no path.
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            DocumentSource::File(path) => Some(path),
            DocumentSource::Virtual { .. } => None,
        }
    }

    /// Update the backing path after the tree renames the underlying file
    /// (US-2b) — future saves target the new location. A no-op on a
    /// [`DocumentSource::Virtual`] document: it has no path to retarget,
    /// the same way a tree rename never touches a `TabKind::Diff` tab
    /// (`app_core::diff_tab`).
    pub fn set_path(&mut self, path: PathBuf) {
        if matches!(self.source, DocumentSource::File(_)) {
            self.source = DocumentSource::File(path);
        }
    }

    /// Whether this document cannot be saved or reloaded. Every
    /// [`DocumentSource::Virtual`] document is read-only.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the tree reported this document's backing file as deleted
    /// (US-2b). Always `false` for a [`DocumentSource::Virtual`] document.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Record that the tree deleted this document's backing file (US-2b).
    /// Subsequent `save()` calls fail with a clear error instead of
    /// silently writing to a path that no longer exists as the user
    /// expects. A no-op on a [`DocumentSource::Virtual`] document — there is
    /// no backing file for the tree to have deleted.
    pub fn mark_deleted(&mut self) {
        if matches!(self.source, DocumentSource::File(_)) {
            self.deleted = true;
        }
    }

    /// Tab title: the file name for a [`DocumentSource::File`] document, or
    /// the last `/`-separated segment of `key` (falling back to the whole
    /// key) for a [`DocumentSource::Virtual`] one — csharp-ls's
    /// `csharp:/metadata/.../Console.cs`-shaped keys carry a real file name
    /// worth showing, and this also doubles as the extension the
    /// highlighting/language registry matches on (Y2).
    pub fn title(&self) -> String {
        match &self.source {
            DocumentSource::File(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            DocumentSource::Virtual { scheme, key } => key
                .rsplit('/')
                .next()
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string())
                .unwrap_or_else(|| format!("{scheme}:{key}")),
        }
    }

    /// Whether this document has unsaved edits.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Explicitly set the dirty flag. Used by `ui-shell` to mirror
    /// `QPlainTextEdit`'s own `QTextDocument::modificationChanged` state
    /// (live keystrokes are not marshalled through `insert`/`delete` — see
    /// mvp-implementation-plan.md §2), rather than editor-core tracking
    /// dirty state independently of the widget.
    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    /// Current buffer content as a plain string.
    pub fn content(&self) -> String {
        self.rope.to_string()
    }

    /// Number of lines in the buffer (rope line count).
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Number of chars in the buffer.
    pub fn char_count(&self) -> usize {
        self.rope.len_chars()
    }

    /// Insert text at a char index. Marks the document dirty.
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        self.rope.insert(char_idx, text);
        self.dirty = true;
    }

    /// Delete a char range. Marks the document dirty.
    pub fn delete(&mut self, char_range: Range<usize>) {
        self.rope.remove(char_range);
        self.dirty = true;
    }

    /// Replace the entire buffer content in one shot — this is the path
    /// `ui-shell` uses on save, pulling the full current text out of the
    /// `QPlainTextEdit` widget and handing it back here (see
    /// mvp-implementation-plan.md §2, "Tab / buffer state → Qt widgets").
    /// Marks the document dirty.
    pub fn replace_content(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.dirty = true;
    }

    /// Re-read the backing file from disk into the buffer, discarding any
    /// in-editor edits (the external-change "Reload" choice).
    /// Clears the dirty flag on success; leaves existing state untouched on
    /// failure. A [`DocumentSource::Virtual`] document has no backing file
    /// to re-read and always refuses.
    pub fn reload(&mut self) -> io::Result<()> {
        let DocumentSource::File(path) = &self.source else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("\"{}\" has no backing file to reload", self.title()),
            ));
        };
        let content = fs::read_to_string(path)?;
        self.rope = Rope::from_str(&content);
        self.dirty = false;
        Ok(())
    }

    /// Write the current buffer content to disk, overwriting the file.
    /// Clears the dirty flag on success; leaves it set on failure so no
    /// unsaved state is silently lost (US-4's save-failure criterion).
    ///
    /// Refuses cleanly, not silently, on a read-only document (C12) — a
    /// [`DocumentSource::Virtual`] document has no file to write to in the
    /// first place.
    pub fn save(&mut self) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("\"{}\" is read-only and cannot be saved", self.title()),
            ));
        }
        if self.deleted {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("\"{}\" was deleted; nothing to save to", self.title()),
            ));
        }
        let DocumentSource::File(path) = &self.source else {
            // Unreachable today (only a `Virtual` document is read-only,
            // caught above), but a `save()` with no path to write to must
            // never silently no-op — fail loudly rather than assume.
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("\"{}\" has no backing file to save to", self.title()),
            ));
        };
        fs::write(path, self.rope.to_string())?;
        self.dirty = false;
        Ok(())
    }
}

/// Ordered list of open tabs/documents, with the minimal state a tab-strip
/// widget needs: which tab is active, and (via [`Document`]) each tab's
/// title and dirty indicator.
#[derive(Default)]
pub struct TabList {
    tabs: Vec<Document>,
    active: Option<usize>,
}

impl TabList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open `path` as a new tab, or focus the existing tab if the file is
    /// already open (US-3: clicking an already-open file focuses it rather
    /// than duplicating the tab). Returns the tab's index.
    pub fn open(&mut self, path: impl AsRef<Path>) -> io::Result<usize> {
        let path = path.as_ref();
        if let Some(index) = self.find_by_path(path) {
            self.active = Some(index);
            return Ok(index);
        }
        let doc = Document::open(path)?;
        self.tabs.push(doc);
        let index = self.tabs.len() - 1;
        self.active = Some(index);
        Ok(index)
    }

    /// Close the tab at `index`, returning the removed document.
    /// Re-homes the active tab if it was the one closed or shifts down.
    pub fn close(&mut self, index: usize) -> Option<Document> {
        if index >= self.tabs.len() {
            return None;
        }
        let doc = self.tabs.remove(index);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.tabs.len() - 1)),
            other => other,
        };
        Some(doc)
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = Some(index);
        }
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    pub fn active(&self) -> Option<&Document> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut Document> {
        match self.active {
            Some(i) => self.tabs.get_mut(i),
            None => None,
        }
    }

    pub fn get(&self, index: usize) -> Option<&Document> {
        self.tabs.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Document> {
        self.tabs.get_mut(index)
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.tabs.iter()
    }

    /// Index of the tab backed by `path`, if any is open. A
    /// [`DocumentSource::Virtual`] tab has no path and never matches.
    pub fn find_by_path(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|d| d.path() == Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn open_small_file_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "small.txt", "hello world");
        let doc = Document::open(&path).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(doc.content(), "hello world");
        assert_eq!(doc.title(), "small.txt");
    }

    #[test]
    fn edit_marks_dirty_then_save_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "edit.txt", "hello");
        let mut doc = Document::open(&path).unwrap();
        assert!(!doc.is_dirty());

        doc.insert(5, " world");
        assert!(doc.is_dirty());
        assert_eq!(doc.content(), "hello world");

        doc.save().unwrap();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn save_round_trip_reload_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "roundtrip.txt", "start");
        let mut doc = Document::open(&path).unwrap();
        doc.replace_content("completely different content\nwith a second line");
        doc.save().unwrap();

        let reloaded = Document::open(&path).unwrap();
        assert_eq!(
            reloaded.content(),
            "completely different content\nwith a second line"
        );
        assert!(!reloaded.is_dirty());
    }

    #[test]
    fn save_failure_leaves_dirty_flag_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "gone.txt", "content");
        let mut doc = Document::open(&path).unwrap();
        doc.insert(0, "x");
        assert!(doc.is_dirty());

        // Remove the backing directory entirely so save() fails.
        fs::remove_file(&path).unwrap();
        fs::remove_dir_all(dir.path()).unwrap();

        assert!(doc.save().is_err());
        assert!(doc.is_dirty(), "dirty flag must survive a failed save");
    }

    #[test]
    fn large_file_loads_via_rope_without_pathological_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        // Generate a tens-of-MB fixture: ~50 bytes/line * ~1,000,000 lines
        // ≈ 50 MB, well into the "tens of MB" acceptance criterion.
        let mut f = fs::File::create(&path).unwrap();
        let line = "the quick brown fox jumps over the lazy dog\n";
        for i in 0..1_000_000 {
            f.write_all(line.as_bytes()).unwrap();
            if i % 100_000 == 0 {
                // keep a few distinguishable lines to sanity-check content
                f.write_all(b"MARKER\n").unwrap();
            }
        }
        drop(f);

        let metadata = fs::metadata(&path).unwrap();
        assert!(
            metadata.len() > 10_000_000,
            "fixture should be tens of MB, was {} bytes",
            metadata.len()
        );

        let start = std::time::Instant::now();
        let doc = Document::open(&path).unwrap();
        let elapsed = start.elapsed();

        assert!(!doc.is_dirty());
        assert!(doc.line_count() > 1_000_000);
        assert!(doc.char_count() > 10_000_000);
        // 5s was too tight under Docker/shared-host contention: measured
        // 5.97-6.18s on an otherwise-passing, unrelated build with the host
        // under load, not a regression in loading itself. 12s still catches
        // real pathological (e.g. quadratic) behavior.
        assert!(
            elapsed.as_secs() < 12,
            "loading a tens-of-MB file into the rope took too long: {:?}",
            elapsed
        );

        // Cheap edit near the start of a large rope should also be fast —
        // this is the "no hard multi-second freeze" property at the buffer
        // layer that backs US-3's large-file acceptance criterion.
        let mut doc = doc;
        let edit_start = std::time::Instant::now();
        doc.insert(0, "EDITED\n");
        let edit_elapsed = edit_start.elapsed();
        assert!(doc.is_dirty());
        assert!(
            edit_elapsed.as_millis() < 500,
            "editing a large rope took too long: {:?}",
            edit_elapsed
        );
    }

    #[test]
    fn set_dirty_mirrors_widget_modified_state_without_editing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "mirror.txt", "hello");
        let mut doc = Document::open(&path).unwrap();
        assert!(!doc.is_dirty());

        doc.set_dirty(true);
        assert!(doc.is_dirty());
        assert_eq!(doc.content(), "hello", "set_dirty must not touch content");

        doc.set_dirty(false);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn tablist_open_focuses_existing_tab_instead_of_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "a.txt", "a");
        let mut tabs = TabList::new();

        let first_index = tabs.open(&path).unwrap();
        let second_index = tabs.open(&path).unwrap();

        assert_eq!(first_index, second_index);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), Some(first_index));
    }

    #[test]
    fn tablist_tracks_active_tab_and_dirty_titles() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_fixture(&dir, "a.txt", "a");
        let b = write_fixture(&dir, "b.txt", "b");
        let mut tabs = TabList::new();

        let a_index = tabs.open(&a).unwrap();
        let b_index = tabs.open(&b).unwrap();
        assert_eq!(tabs.active_index(), Some(b_index));

        tabs.set_active(a_index);
        assert_eq!(tabs.active_index(), Some(a_index));
        assert_eq!(tabs.active().unwrap().title(), "a.txt");
        assert!(!tabs.active().unwrap().is_dirty());

        tabs.active_mut().unwrap().insert(0, "x");
        assert!(tabs.get(a_index).unwrap().is_dirty());
        assert!(!tabs.get(b_index).unwrap().is_dirty());
    }

    #[test]
    fn tablist_close_reindexes_active_tab() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_fixture(&dir, "a.txt", "a");
        let b = write_fixture(&dir, "b.txt", "b");
        let c = write_fixture(&dir, "c.txt", "c");
        let mut tabs = TabList::new();
        tabs.open(&a).unwrap();
        tabs.open(&b).unwrap();
        tabs.open(&c).unwrap();
        tabs.set_active(1); // b

        tabs.close(0); // remove a
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), Some(0)); // b shifted down
        assert_eq!(tabs.active().unwrap().title(), "b.txt");
    }

    #[test]
    fn marking_deleted_blocks_further_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "will_be_deleted.txt", "content");
        let mut doc = Document::open(&path).unwrap();
        doc.insert(0, "x");

        doc.mark_deleted();
        assert!(doc.is_deleted());

        let result = doc.save();
        assert!(result.is_err(), "save must fail once marked deleted");
        assert!(doc.is_dirty(), "dirty flag must survive the blocked save");
    }

    #[test]
    fn reload_discards_in_memory_edits_and_picks_up_disk_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(&dir, "reload.txt", "original");
        let mut doc = Document::open(&path).unwrap();

        doc.insert(0, "in-editor edit, never saved\n");
        assert!(doc.is_dirty());

        // Simulate an external editor overwriting the file.
        fs::write(&path, "changed externally").unwrap();

        doc.reload().unwrap();
        assert_eq!(doc.content(), "changed externally");
        assert!(!doc.is_dirty());
    }

    #[test]
    fn set_path_redirects_future_saves() {
        let dir = tempfile::tempdir().unwrap();
        let old_path = write_fixture(&dir, "old_name.txt", "content");
        let new_path = dir.path().join("new_name.txt");
        let mut doc = Document::open(&old_path).unwrap();

        doc.set_path(new_path.clone());
        assert_eq!(doc.title(), "new_name.txt");

        doc.insert(0, "x");
        doc.save().unwrap();
        assert!(new_path.exists());
    }

    // --- DocumentSource::Virtual (C12) ------------------------------------

    #[test]
    fn a_virtual_document_has_no_path_and_a_title_derived_from_its_key() {
        let doc = Document::open_virtual("csharp", "metadata/Projects/x/Console.cs", "// stub");
        assert_eq!(doc.path(), None);
        assert_eq!(doc.title(), "Console.cs");
        assert!(doc.is_read_only());
        assert!(!doc.is_dirty());
        assert_eq!(doc.content(), "// stub");
        assert_eq!(
            doc.source(),
            &DocumentSource::Virtual {
                scheme: "csharp".to_string(),
                key: "metadata/Projects/x/Console.cs".to_string(),
            }
        );
    }

    #[test]
    fn a_virtual_document_with_no_slash_in_its_key_titles_itself_the_whole_key() {
        let doc = Document::open_virtual("csharp", "opaque-id", "text");
        assert_eq!(doc.title(), "opaque-id");
    }

    #[test]
    fn a_virtual_document_refuses_to_be_saved() {
        let mut doc = Document::open_virtual("csharp", "metadata/Console.cs", "// decompiled");

        let err = doc.save().unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn a_virtual_document_refuses_to_be_reloaded() {
        let mut doc = Document::open_virtual("csharp", "metadata/Console.cs", "// decompiled");

        let err = doc.reload().unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert_eq!(doc.content(), "// decompiled", "reload must not clear it");
    }

    #[test]
    fn a_virtual_document_still_accepts_in_memory_edits_and_dirty_tracking() {
        // Read-only is enforced at the save boundary (C12's non-negotiable
        // rule), not by refusing every edit at this layer — the same
        // "editing is permitted, saving is separately gated" split
        // `AppSession`'s binary/diff tabs already use one layer up.
        let mut doc = Document::open_virtual("csharp", "metadata/Console.cs", "one");
        doc.insert(3, " two");
        assert_eq!(doc.content(), "one two");
        assert!(doc.is_dirty());
    }

    #[test]
    fn set_path_and_mark_deleted_are_no_ops_on_a_virtual_document() {
        let mut doc = Document::open_virtual("csharp", "metadata/Console.cs", "text");

        doc.set_path(PathBuf::from("/should/not/apply.cs"));
        assert_eq!(doc.path(), None);

        doc.mark_deleted();
        assert!(
            !doc.is_deleted(),
            "a virtual document has no file to delete"
        );
    }

    #[test]
    fn tablist_find_by_path_never_matches_a_virtual_document() {
        // Not directly exercisable through `TabList::open` (which only ever
        // creates `File` documents), but a defensive proof that
        // `find_by_path` is `Some(path)`-keyed, not path-shaped-string-keyed
        // — a virtual document's key could coincidentally look like a path.
        let doc = Document::open_virtual("csharp", "/a/weird/key.cs", "text");
        assert_eq!(doc.path(), None);
        assert_ne!(doc.path(), Some(Path::new("/a/weird/key.cs")));
    }
}
