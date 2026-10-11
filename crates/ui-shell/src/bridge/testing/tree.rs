//! The project tree's test entries (PHPUnit editor runs plan, E6): which
//! temporary `phpunit` configuration a right-clicked file, directory or the
//! project itself stands for.
//!
//! Translation only: the rules are `php_core::tree_run`'s; the run itself
//! is the ordinary Run / Debug / Run with Coverage of that configuration,
//! so before-launch tasks, the toolbar combo and edits stored for a
//! temporary configuration all behave as for an editor run.
//!
//! A right-click must not hang the menu on a slow disk (a `\\wsl.localhost`
//! share): the packages found are cached per directory until a
//! `composer.json` or PHPUnit configuration changes, the editor's text is
//! used before the file's, and what still needs the disk gets
//! [`PROBE_DEADLINE`] before the menu opens without the entries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cxx_qt_lib::QString;
use php_core::tree_run::{read_test_source, tree_run, TestPackage, TreeRow, TreeRunLabel};

use super::configuration::remember;
use crate::bridge::errors;
use crate::bridge::ffi;

/// How long a right-click waits for the disk before the menu opens without
/// test entries.
const PROBE_DEADLINE: Duration = Duration::from_millis(300);

/// The test packages right-clicked directories belong to, for one project.
#[derive(Default)]
pub(super) struct PackageCache {
    root: PathBuf,
    by_dir: HashMap<PathBuf, TestPackage>,
}

/// The run the open tree menu offers, kept for its click.
pub(super) struct TreeOffer {
    path: String,
    is_dir: bool,
    config: run_core::RunConfig,
}

fn to_ffi_tree_run(label: &TreeRunLabel) -> ffi::FfiTreeRun {
    let (kind, subject) = match label {
        TreeRunLabel::AllTests => (ffi::FfiTreeRunKind::AllTests, ""),
        TreeRunLabel::Directory(name) => (ffi::FfiTreeRunKind::Directory, name.as_str()),
        TreeRunLabel::Named(name) => (ffi::FfiTreeRunKind::Named, name.as_str()),
    };
    ffi::FfiTreeRun {
        runnable: true,
        kind,
        subject: QString::from(subject),
    }
}

fn nothing_to_run() -> ffi::FfiTreeRun {
    ffi::FfiTreeRun {
        runnable: false,
        kind: ffi::FfiTreeRunKind::AllTests,
        subject: QString::default(),
    }
}

/// `work` on a thread of its own, or `None` when it outlasts
/// [`PROBE_DEADLINE`]; the thread then finishes unobserved.
fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.recv_timeout(PROBE_DEADLINE).ok()
}

/// Whether `path` is excluded from the project scope (ADR-0064).
fn is_excluded(root: &Path, path: &Path, is_dir: bool) -> bool {
    let settings = crate::bridge::convert::load_resolved_settings_for(root);
    project_model::ProjectScope::new(root, &settings.excluded, &settings.ignored_names)
        .is_excluded(path, is_dir)
}

impl ffi::TestService {
    /// The package of `dir` found before, for the project at `root`.
    pub(super) fn cached_package(&self, root: &Path, dir: &Path) -> Option<TestPackage> {
        let mut cache = self.tree_packages.borrow_mut();
        if cache.root != root {
            *cache = PackageCache {
                root: root.to_path_buf(),
                by_dir: HashMap::new(),
            };
        }
        cache.by_dir.get(dir).cloned()
    }

    /// Remember the package a run's worker found for `dir`.
    pub(super) fn cache_package(&self, dir: PathBuf, package: TestPackage) {
        self.tree_packages.borrow_mut().by_dir.insert(dir, package);
    }

    /// What the tree's entries for `path` (a directory when `is_dir`) run
    /// and call it; not runnable when it holds nothing to run.
    /// `open_text` is the file's unsaved editor text, when it is open.
    pub fn tree_run(
        &self,
        path: &QString,
        is_dir: bool,
        open_text: &QString,
        has_open_text: bool,
    ) -> ffi::FfiTreeRun {
        *self.tree_offer.borrow_mut() = None;
        let Some(root) = super::current_project_root() else {
            return nothing_to_run();
        };
        let path_text = path.to_string();
        let row_path = PathBuf::from(&path_text);
        let dir = if is_dir {
            row_path.clone()
        } else {
            row_path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        let cached = self.cached_package(&root, &dir);
        let open = has_open_text.then(|| open_text.to_string());
        let probe_root = root.clone();
        let probe_dir = dir.clone();
        let probed = within_deadline(move || {
            let package =
                cached.unwrap_or_else(|| TestPackage::containing(&probe_root, &probe_dir));
            let row = TreeRow {
                path: &row_path,
                is_dir,
                excluded: is_excluded(&probe_root, &row_path, is_dir),
            };
            let run = tree_run(
                &probe_root,
                row,
                &package,
                |file| open.or_else(|| read_test_source(file)),
                cfg!(windows),
            );
            (package, run)
        });
        let Some((package, run)) = probed else {
            return nothing_to_run();
        };
        self.tree_packages.borrow_mut().by_dir.insert(dir, package);
        let Some(run) = run else {
            return nothing_to_run();
        };
        let answer = to_ffi_tree_run(&run.label);
        *self.tree_offer.borrow_mut() = Some(TreeOffer {
            path: path_text,
            is_dir,
            config: run.config,
        });
        answer
    }

    /// Store the temporary configuration the open tree menu offers for
    /// `path` (an existing one keeps its edits), tell the toolbar to select
    /// it and return its id; the view then runs it like any configuration.
    pub fn remember_tree_run(
        mut self: std::pin::Pin<&mut Self>,
        path: &QString,
        is_dir: bool,
    ) -> ffi::FfiConfigIdResult {
        let refused = |code: i32, message: &str| ffi::FfiConfigIdResult {
            result: errors::failure(code, message),
            config_id: QString::default(),
        };
        let Some(root) = super::current_project_root() else {
            return refused(errors::CODE_NO_PROJECT, "no project is open");
        };
        let path = path.to_string();
        let offer = self.tree_offer.borrow_mut().take();
        let Some(config) = offer
            .filter(|offer| offer.path == path && offer.is_dir == is_dir)
            .map(|offer| offer.config)
        else {
            return refused(
                errors::CODE_REFUSED,
                "the project tree's test entry is out of date; right-click the row again",
            );
        };
        if let Err(reason) = remember(&config, &root) {
            return refused(
                errors::CODE_SETTINGS_IO,
                &format!(
                    "could not save the run configuration \"{}\": {reason}",
                    config.name
                ),
            );
        }
        let id = QString::from(config.id.as_str());
        self.as_mut().configuration_remembered(id.clone());
        ffi::FfiConfigIdResult {
            result: ffi::FfiResult::default(),
            config_id: id,
        }
    }

    /// A watched project file changed: a `composer.json` or PHPUnit
    /// configuration can change which rows hold tests, and a test file what
    /// Go to Test lists as covering a class (#426).
    pub fn project_file_changed(&self, path: &QString) {
        let path = PathBuf::from(path.to_string());
        if php_core::tree_run::is_package_file(&path) {
            self.tree_packages.borrow_mut().by_dir.clear();
        }
        self.covers.file_changed(&path);
    }
}
