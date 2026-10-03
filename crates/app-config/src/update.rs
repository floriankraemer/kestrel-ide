//! The load-edit-save primitive every "change one setting" path shares.

use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::{load_toml, save_toml, ConfigError};

/// Load, edit, save.
///
/// A load failure aborts the update instead of editing a `T::default()` and
/// saving that: the file on disk holds everything the user configured, so
/// writing defaults over it because it could not be read (or was momentarily
/// unreadable) is data loss, not a fresh start.
///
/// An edit that changes nothing writes nothing: no new file where there was
/// none, no reformatting of the user's own. Settings files live in config
/// directories and, for the project layers, in the user's repository.
pub(crate) fn update_toml<T>(
    path: &Path,
    temp_path: &Path,
    edit: impl FnOnce(&mut T),
) -> Result<(), ConfigError>
where
    T: DeserializeOwned + Serialize + Default + PartialEq + Clone,
{
    let before: T = load_toml(path)?;
    let mut value = before.clone();
    edit(&mut value);
    if value == before {
        return Ok(());
    }
    save_toml(path, temp_path, &value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[derive(Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        value: u32,
    }

    #[test]
    fn an_edit_that_changes_nothing_creates_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.toml");
        update_toml::<Probe>(&path, &dir.path().join("probe.tmp"), |_| {}).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn an_edit_that_changes_nothing_leaves_the_file_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.toml");
        let body = "# mine\nvalue = 3\n";
        fs::write(&path, body).unwrap();
        update_toml::<Probe>(&path, &dir.path().join("probe.tmp"), |p| p.value = 3).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn a_real_edit_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.toml");
        update_toml::<Probe>(&path, &dir.path().join("probe.tmp"), |p| p.value = 7).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "value = 7\n");
    }
}
