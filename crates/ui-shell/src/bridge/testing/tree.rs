//! The project tree's test entries (PHPUnit editor runs plan, E6): which
//! temporary `phpunit` configuration a right-clicked file, directory or the
//! project itself stands for.
//!
//! Translation only: the configuration is `php_core::phpunit_run::tree_config`'s;
//! the run itself is the ordinary Run / Debug / Run with Coverage of that
//! configuration, so before-launch tasks, the toolbar combo and edits stored
//! for a temporary configuration all behave as for an editor run.

use std::path::Path;
use std::pin::Pin;

use cxx_qt_lib::QString;
use php_core::phpunit_run::{has_test_framework, tree_config, TreeTarget};

use super::configuration::{remember, stored_or};
use crate::bridge::ffi;

/// A file larger than this is not parsed for tests on a right-click.
const MAX_PARSED_BYTES: u64 = 1 << 20;

fn is_php_file(path: &Path) -> bool {
    syntax_core::language_for_path(path).id() == "php"
}

/// The configuration `path` (a directory when `is_dir`) stands for in the
/// project at `root`; `None` when the project has no PHP test framework, the
/// path is outside it, or a file holds no tests.
fn config_for(path: &str, is_dir: bool, root: &Path) -> Option<run_core::RunConfig> {
    if !has_test_framework(root) {
        return None;
    }
    let relative =
        test_core::filter::relative_to_root(path, &root.to_string_lossy(), cfg!(windows))?;
    let setting = if is_dir {
        match relative.as_str() {
            "." => tree_config(TreeTarget::Project),
            dir => tree_config(TreeTarget::Directory(dir)),
        }
    } else {
        let file = Path::new(path);
        if !is_php_file(file) || std::fs::metadata(file).ok()?.len() > MAX_PARSED_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(file).ok()?;
        tree_config(TreeTarget::File {
            relative: &relative,
            text: &text,
        })
    }?;
    Some(stored_or(setting, root))
}

impl ffi::TestService {
    /// The name the tree's Run entries show for `path`, empty when it holds
    /// nothing runnable.
    pub fn tree_run_name(&self, path: &QString, is_dir: bool) -> QString {
        super::current_project_root()
            .and_then(|root| config_for(&path.to_string(), is_dir, &root))
            .map(|config| QString::from(config.name.as_str()))
            .unwrap_or_default()
    }

    /// Store the temporary configuration for `path` (an existing one keeps
    /// its edits), tell the toolbar to select it and return its id, empty
    /// when there is none; the view then runs it like any configuration.
    pub fn remember_tree_run(mut self: Pin<&mut Self>, path: &QString, is_dir: bool) -> QString {
        let Some(root) = super::current_project_root() else {
            return QString::default();
        };
        let Some(config) = config_for(&path.to_string(), is_dir, &root) else {
            return QString::default();
        };
        if config.temporary {
            remember(&config, &root);
        }
        let id = QString::from(config.id.as_str());
        self.as_mut().configuration_remembered(id.clone());
        id
    }
}
