//! `pack.toml`: the tables an icon theme is, and the order they are consulted in.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::IconError;

/// The directory an icon pack's SVGs live in, relative to `pack.toml`.
///
/// Fixed rather than a field of the pack, because a pack-supplied directory
/// would be an untrusted path joined to a plugin directory, and
/// `plugin-api`'s path-safety rules stop at the manifest — they say nothing
/// about a path a file the manifest points to then names. A constant cannot
/// climb anywhere. It also gives P4's import script one less thing to get
/// right.
pub const ICONS_DIR: &str = "icons";

/// Which set of icons the current colour theme wants.
///
/// Not a colour and not a theme id: the only question a pack can answer is
/// whether the light substitutions apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Appearance {
    Dark,
    Light,
}

/// One icon theme: the mapping tables, and nothing about pixels.
///
/// Parsing and validation are one step and are not separable from outside,
/// so an `IconPack` that exists has non-empty defaults — the same guarantee
/// `plugin_api::PluginManifest` gives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconPack {
    /// Stable id. Also the first component of the render cache key, so two
    /// packs that name an icon `rust` never share a rasterisation.
    pub id: String,
    /// What the settings page shows.
    pub label: String,

    /// Icon for a file nothing else matched.
    pub default_file: String,
    /// Icon for a collapsed folder no `folder_names` entry matched.
    pub default_folder: String,
    /// Icon for an expanded folder no `folder_names_open` entry matched.
    pub default_folder_open: String,
    /// Icon for the project root row.
    pub default_root_folder: String,

    /// Exact file name -> icon id, e.g. `"cargo.toml" = "cargo"`.
    #[serde(default)]
    file_names: HashMap<String, String>,
    /// Extension -> icon id. A key may be multi-part (`"spec.ts"`); the
    /// longest suffix that matches wins.
    #[serde(default)]
    file_extensions: HashMap<String, String>,
    /// `syntax-core` language id -> icon id. Consulted only after both name
    /// tables miss, so a pack can give `.rs` an icon without knowing what
    /// the grammar registry calls Rust.
    #[serde(default)]
    language_ids: HashMap<String, String>,

    /// Folder name -> icon id, collapsed.
    #[serde(default)]
    folder_names: HashMap<String, String>,
    /// Folder name -> icon id, expanded.
    #[serde(default)]
    folder_names_open: HashMap<String, String>,

    /// Icon id -> its light-theme variant. A substitution applied *after*
    /// resolution, so an icon with no entry simply keeps its dark form and
    /// a pack needs light art only where the dark art is unreadable.
    #[serde(default)]
    light: HashMap<String, String>,
}

impl IconPack {
    /// Parse and validate `pack.toml`.
    pub fn from_toml_str(text: &str) -> Result<Self, IconError> {
        let pack: Self =
            toml::from_str(text).map_err(|e| IconError::MalformedPack(e.to_string()))?;
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<(), IconError> {
        for (field, value) in [
            ("id", &self.id),
            ("label", &self.label),
            ("default_file", &self.default_file),
            ("default_folder", &self.default_folder),
            ("default_folder_open", &self.default_folder_open),
            ("default_root_folder", &self.default_root_folder),
        ] {
            if value.trim().is_empty() {
                return Err(IconError::EmptyField(field));
            }
        }
        Ok(())
    }

    /// The icon for a file row.
    ///
    /// `file_name` is the last path component, not a path. `language_id` is
    /// whatever `syntax-core`'s registry resolved for the file, or `None`
    /// when it recognised nothing — this crate never detects a language
    /// itself (ADR-0018).
    ///
    /// Order: exact name, then longest multi-part extension, then single
    /// extension, then language id, then the pack default.
    pub fn file_icon(
        &self,
        file_name: &str,
        language_id: Option<&str>,
        appearance: Appearance,
    ) -> &str {
        // Lowercased once for both name tables — see `extension_icon`
        // for why the case rule is upstream's and not ours to pick.
        let lower = file_name.to_ascii_lowercase();
        let resolved = self
            .file_names
            .get(&lower)
            .or_else(|| self.extension_icon(&lower))
            .or_else(|| language_id.and_then(|id| self.language_ids.get(id)))
            .unwrap_or(&self.default_file);
        self.for_appearance(resolved, appearance)
    }

    /// The icon for a folder row, by name and expanded state.
    pub fn folder_icon(&self, folder_name: &str, expanded: bool, appearance: Appearance) -> &str {
        let (table, default) = if expanded {
            (&self.folder_names_open, &self.default_folder_open)
        } else {
            (&self.folder_names, &self.default_folder)
        };
        // A folder with an entry in `folder_names` but none in
        // `folder_names_open` falls back to the *default open* icon rather
        // than to its own closed art: mixing the two states in one tree is
        // the misreading this avoids.
        let resolved = table
            .get(&folder_name.to_ascii_lowercase())
            .unwrap_or(default);
        self.for_appearance(resolved, appearance)
    }

    /// The icon for the project root row.
    pub fn root_folder_icon(&self, appearance: Appearance) -> &str {
        self.for_appearance(&self.default_root_folder, appearance)
    }

    /// The pack default for a file, used as the fallback when an icon id
    /// has no asset behind it.
    pub fn default_file_icon(&self, appearance: Appearance) -> &str {
        self.for_appearance(&self.default_file, appearance)
    }

    /// One icon by its id, with the light-variant substitution applied — for
    /// art no file or folder name reaches. An id the pack has no asset for
    /// is the renderer's to fall back from (to the default file icon).
    pub fn icon_by_id<'a>(&'a self, icon_id: &'a str, appearance: Appearance) -> &'a str {
        self.for_appearance(icon_id, appearance)
    }

    /// Where an icon's SVG lives, relative to `pack.toml`.
    pub fn asset_path(&self, icon_id: &str) -> PathBuf {
        PathBuf::from(ICONS_DIR).join(format!("{icon_id}.svg"))
    }

    /// Longest matching suffix after a dot. `lower` is the file name,
    /// already lowercased by [`IconPack::file_icon`].
    ///
    /// Every name table matches case-insensitively, which is upstream's
    /// rule and not a convenience: VS Code lowercases a row's name before
    /// consulting `fileNames`, `folderNames` and `fileExtensions`, so
    /// Material ships `dockerfile` and `makefile` in lower case and means
    /// them to match `Dockerfile` and `Makefile`. Matching by exact case
    /// leaves those rows — and `LICENSE`, and every `META-INF` — with no
    /// icon at all. The keys are lowercased at import time to match (see
    /// `scripts/import-material-icons.py`), which also revives the handful
    /// of mixed-case keys that are dead in VS Code itself.
    fn extension_icon(&self, lower: &str) -> Option<&String> {
        // Dots left to right, so the longest suffix is tried first:
        // "a.spec.ts" yields "spec.ts" before "ts". A leading dot falls out
        // of this for free — ".gitignore" yields "gitignore".
        lower
            .char_indices()
            .filter(|&(_, c)| c == '.')
            .find_map(|(i, _)| self.file_extensions.get(&lower[i + 1..]))
    }

    fn for_appearance<'a>(&'a self, icon_id: &'a str, appearance: Appearance) -> &'a str {
        match appearance {
            Appearance::Dark => icon_id,
            Appearance::Light => self.light.get(icon_id).map_or(icon_id, String::as_str),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK: &str = r#"
id = "fixture"
label = "Fixture"
default_file = "file"
default_folder = "folder"
default_folder_open = "folder-open"
default_root_folder = "folder-root"

[file_names]
"cargo.toml" = "cargo"
"makefile" = "make"

[file_extensions]
rs = "rust"
ts = "typescript"
"spec.ts" = "test-ts"
toml = "toml"

[language_ids]
rust = "rust-by-language"
python = "python"

[folder_names]
src = "folder-src"
docs = "folder-docs"

[folder_names_open]
src = "folder-src-open"

[light]
rust = "rust_light"
folder-src = "folder-src_light"
"#;

    fn pack() -> IconPack {
        IconPack::from_toml_str(PACK).expect("fixture pack parses")
    }

    #[test]
    fn an_exact_filename_beats_its_extension() {
        // `cargo.toml` would otherwise match the `toml` extension.
        assert_eq!(
            pack().file_icon("cargo.toml", None, Appearance::Dark),
            "cargo"
        );
    }

    #[test]
    fn a_longer_extension_beats_a_shorter_one() {
        let pack = pack();
        assert_eq!(
            pack.file_icon("widget.spec.ts", None, Appearance::Dark),
            "test-ts"
        );
        assert_eq!(
            pack.file_icon("widget.ts", None, Appearance::Dark),
            "typescript"
        );
    }

    #[test]
    fn an_extension_beats_the_language_id() {
        // Both tables know Rust; the extension table is consulted first, so
        // a pack can give `.rs` art without agreeing with the grammar
        // registry's vocabulary.
        assert_eq!(
            pack().file_icon("main.rs", Some("rust"), Appearance::Dark),
            "rust"
        );
    }

    #[test]
    fn a_language_id_beats_the_default_when_no_name_or_extension_matches() {
        assert_eq!(
            pack().file_icon("script", Some("python"), Appearance::Dark),
            "python"
        );
    }

    #[test]
    fn an_unmatched_file_falls_back_to_the_pack_default() {
        let pack = pack();
        assert_eq!(pack.file_icon("notes.xyz", None, Appearance::Dark), "file");
        assert_eq!(
            pack.file_icon("notes.xyz", Some("brainfuck"), Appearance::Dark),
            "file"
        );
    }

    #[test]
    fn a_leading_dot_is_treated_as_the_start_of_an_extension() {
        // `.rs` is not a Rust file by name, but the suffix scan must not
        // skip a dot at index 0 either, or `.gitignore` could never match a
        // `gitignore` extension entry.
        assert_eq!(pack().file_icon(".rs", None, Appearance::Dark), "rust");
    }

    #[test]
    fn names_and_extensions_both_match_case_insensitively() {
        // Upstream Material's rule, matched deliberately — see
        // `extension_icon`.
        let pack = pack();
        assert_eq!(pack.file_icon("MAIN.RS", None, Appearance::Dark), "rust");
        assert_eq!(pack.file_icon("Makefile", None, Appearance::Dark), "make");
        assert_eq!(pack.file_icon("makefile", None, Appearance::Dark), "make");
        assert_eq!(
            pack.folder_icon("SRC", false, Appearance::Dark),
            "folder-src"
        );
    }

    #[test]
    fn a_folder_uses_the_open_table_when_expanded() {
        let pack = pack();
        assert_eq!(
            pack.folder_icon("src", false, Appearance::Dark),
            "folder-src"
        );
        assert_eq!(
            pack.folder_icon("src", true, Appearance::Dark),
            "folder-src-open"
        );
    }

    #[test]
    fn a_folder_without_an_open_variant_falls_back_to_the_default_open_icon() {
        // Not to its own closed art: a half-open tree reads as a bug.
        let pack = pack();
        assert_eq!(
            pack.folder_icon("docs", false, Appearance::Dark),
            "folder-docs"
        );
        assert_eq!(
            pack.folder_icon("docs", true, Appearance::Dark),
            "folder-open"
        );
    }

    #[test]
    fn an_unknown_folder_falls_back_to_the_defaults_for_its_state() {
        let pack = pack();
        assert_eq!(
            pack.folder_icon("vendor", false, Appearance::Dark),
            "folder"
        );
        assert_eq!(
            pack.folder_icon("vendor", true, Appearance::Dark),
            "folder-open"
        );
        assert_eq!(pack.root_folder_icon(Appearance::Dark), "folder-root");
    }

    #[test]
    fn the_light_table_substitutes_after_resolution() {
        let pack = pack();
        assert_eq!(
            pack.file_icon("main.rs", None, Appearance::Light),
            "rust_light"
        );
        assert_eq!(
            pack.folder_icon("src", false, Appearance::Light),
            "folder-src_light"
        );
    }

    #[test]
    fn an_icon_without_a_light_variant_keeps_its_dark_one() {
        let pack = pack();
        assert_eq!(
            pack.file_icon("widget.ts", None, Appearance::Light),
            "typescript"
        );
        assert_eq!(pack.file_icon("notes.xyz", None, Appearance::Light), "file");
    }

    #[test]
    fn an_asset_path_is_the_icon_id_under_the_icons_directory() {
        assert_eq!(
            pack().asset_path("rust"),
            PathBuf::from("icons").join("rust.svg")
        );
    }

    #[test]
    fn a_malformed_pack_is_a_typed_error_rather_than_a_panic() {
        assert!(matches!(
            IconPack::from_toml_str("id = ["),
            Err(IconError::MalformedPack(_))
        ));
        // An unknown key is a parse error, not a silent drop: a pack whose
        // table name we quietly ignored would render defaults everywhere
        // with nothing to point at.
        assert!(matches!(
            IconPack::from_toml_str(&format!("{PACK}\n[file_extensiosn]\nrs = \"rust\"\n")),
            Err(IconError::MalformedPack(_))
        ));
        assert!(matches!(
            IconPack::from_toml_str(
                &PACK.replace(r#"default_file = "file""#, r#"default_file = """#)
            ),
            Err(IconError::EmptyField("default_file"))
        ));
    }
}
