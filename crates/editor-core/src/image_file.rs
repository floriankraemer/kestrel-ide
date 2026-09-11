//! Read-only handle behind an image tab.
//!
//! Mirrors the parts of [`crate::BinaryFile`]'s surface a tab needs
//! regardless of kind — path, title, rename retargeting, delete flagging —
//! so an image tab behaves like any other tab everywhere except editing.
//! Unlike `BinaryFile`, nothing here reads the file's bytes: decoding is the
//! view's job (`QImageReader` for raster formats, the vendored `resvg`
//! pipeline for SVG — see `ui-shell/cpp/image_viewer.cpp`), so this type only
//! has to prove the file exists and is readable when the tab is opened.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A file opened as an image tab. Existence is checked once, on open; the
/// pixels themselves are decoded by the view, on demand.
pub struct ImageFile {
    path: PathBuf,
    deleted: bool,
}

impl ImageFile {
    /// Open `path` as an image tab. Fails the same way [`crate::BinaryFile::open`]
    /// does when the file cannot be read at all.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        fs::metadata(&path)?;
        Ok(Self {
            path,
            deleted: false,
        })
    }

    /// The file this tab shows.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Update the backing path after a tree-driven rename.
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

    /// Whether the tree reported this file as deleted.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Record that the tree deleted the backing file.
    pub fn mark_deleted(&mut self) {
        self.deleted = true;
    }

    /// Tab title derived from the file name.
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_reads_metadata_and_titles_from_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        fs::write(&path, b"not really a png").unwrap();

        let image = ImageFile::open(&path).unwrap();
        assert_eq!(image.path(), path);
        assert_eq!(image.title(), "photo.png");
        assert!(!image.is_deleted());
    }

    #[test]
    fn open_reports_a_missing_file_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.png");
        assert!(ImageFile::open(&missing).is_err());
    }

    #[test]
    fn set_path_and_mark_deleted_update_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        fs::write(&path, b"x").unwrap();
        let mut image = ImageFile::open(&path).unwrap();

        let new_path = dir.path().join("b.png");
        image.set_path(new_path.clone());
        assert_eq!(image.path(), new_path);
        assert_eq!(image.title(), "b.png");

        image.mark_deleted();
        assert!(image.is_deleted());
    }
}
