//! Console file paths (database-tools.md §8): `<config_dir>/consoles/
//! <source-id>/console*.sql`, and `source_of`, the reverse lookup a
//! console tab uses to find which data source it defaults to.

use std::path::{Path, PathBuf};

/// The directory a source's console files live under.
pub fn console_dir(config_dir: &Path, source_id: &str) -> PathBuf {
    config_dir.join("consoles").join(source_id)
}

/// A fresh console file's path within its source's directory — callers
/// pick `index` (the next unused number) themselves; this only spells the
/// name.
pub fn console_file(config_dir: &Path, source_id: &str, index: u32) -> PathBuf {
    console_dir(config_dir, source_id).join(format!("console{index}.sql"))
}

/// Which source id a console file belongs to, if `path` sits under
/// `config_dir/consoles/<id>/…` — `None` for a plain project file (a
/// `.sql` file opened directly, or one covered instead by the project's
/// own `file_sources` map, which this function knows nothing about; that
/// join is `settings_model::database`'s, not this one's).
pub fn source_of(path: &Path, config_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(config_dir).ok()?;
    let mut components = relative.components();
    if components.next()?.as_os_str() != "consoles" {
        return None;
    }
    let id = components.next()?.as_os_str().to_str()?.to_string();
    // Must actually name a file beneath the id directory, not just the
    // directory itself.
    components.next()?;
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_dir_nests_under_the_source_id() {
        let dir = console_dir(Path::new("/cfg"), "abc123");
        assert_eq!(dir, Path::new("/cfg/consoles/abc123"));
    }

    #[test]
    fn console_file_names_by_index() {
        let file = console_file(Path::new("/cfg"), "abc123", 2);
        assert_eq!(file, Path::new("/cfg/consoles/abc123/console2.sql"));
    }

    #[test]
    fn source_of_reads_the_id_back_out_of_a_console_path() {
        let path = Path::new("/cfg/consoles/abc123/console1.sql");
        assert_eq!(
            source_of(path, Path::new("/cfg")),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn source_of_is_none_for_a_plain_project_file() {
        let path = Path::new("/project/sql/reports.sql");
        assert_eq!(source_of(path, Path::new("/cfg")), None);
    }

    #[test]
    fn source_of_is_none_for_the_consoles_directory_itself() {
        assert_eq!(
            source_of(Path::new("/cfg/consoles"), Path::new("/cfg")),
            None
        );
        assert_eq!(
            source_of(Path::new("/cfg/consoles/abc123"), Path::new("/cfg")),
            None
        );
    }

    #[test]
    fn source_of_is_none_outside_the_config_dir() {
        assert_eq!(
            source_of(
                Path::new("/other/consoles/abc123/console1.sql"),
                Path::new("/cfg")
            ),
            None
        );
    }
}
