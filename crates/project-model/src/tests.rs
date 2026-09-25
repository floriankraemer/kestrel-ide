//! Unit tests for the lazy `DirectoryTree` arena, `ProjectSession`,
//! the create/rename/delete free functions, and `walk_all_entries` —
//! split out of `lib.rs` once that file hit its size-gate ceiling, a
//! mechanical move with no behavior change.
use super::*;

fn make_fixture_tree(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("empty_dir")).unwrap();
    fs::write(root.join("README.md"), "hello").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
}

fn names(tree: &DirectoryTree, id: usize) -> Vec<String> {
    tree.children(id)
        .iter()
        .map(|&child| tree.node(child).name.clone())
        .collect()
}

#[test]
fn open_root_loads_only_the_root_level() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());

    let project = open_folder(dir.path()).unwrap();
    let tree = &project.tree;

    assert_eq!(
        names(tree, tree.root_id()),
        vec!["empty_dir", "src", "README.md"]
    );
    assert_eq!(tree.load_state(tree.root_id()), LoadState::Loaded);

    let src_id = tree
        .children(tree.root_id())
        .iter()
        .find(|&&id| tree.node(id).name == "src")
        .copied()
        .unwrap();
    assert_eq!(
        tree.load_state(src_id),
        LoadState::Unloaded,
        "a directory's own children are not read until asked for"
    );
    assert!(tree.children(src_id).is_empty());
}

#[test]
fn attach_children_loads_one_level_and_marks_it_loaded() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let mut project = open_folder(dir.path()).unwrap();
    let tree = &mut project.tree;
    let src_id = tree
        .children(tree.root_id())
        .iter()
        .find(|&&id| tree.node(id).name == "src")
        .copied()
        .unwrap();

    let entries = list_dir(&dir.path().join("src"), SortOrder::Ascending).unwrap();
    let inserted = tree.attach_children(src_id, entries);

    assert_eq!(inserted.len(), 2);
    assert_eq!(tree.load_state(src_id), LoadState::Loaded);
    assert_eq!(names(tree, src_id), vec!["lib.rs", "main.rs"]);
    for (row, &id) in tree.children(src_id).iter().enumerate() {
        assert_eq!(tree.index_in_parent(id), row);
    }
}

#[test]
fn list_dir_sorts_folders_first_then_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("zebra_dir")).unwrap();
    fs::write(dir.path().join("apple.txt"), "").unwrap();
    fs::write(dir.path().join("Banana.txt"), "").unwrap();
    fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["zebra_dir", "apple.txt", "Banana.txt", "Cargo.toml"]
    );
}

#[test]
fn list_dir_descending_reverses_names_but_keeps_folders_on_top() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());

    let entries = list_dir(dir.path(), SortOrder::Descending).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["src", "empty_dir", "README.md"]);
}

#[test]
fn list_dir_skips_the_search_index_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".ide-index")).unwrap();
    fs::write(dir.path().join(".ide-index/meta.json"), "{}").unwrap();
    fs::write(dir.path().join("visible.txt"), "x").unwrap();

    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["visible.txt"]);
}

#[test]
fn refresh_dir_adds_a_new_entry_as_a_single_insert() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let mut project = open_folder(dir.path()).unwrap();
    let root_id = project.tree.root_id();
    let old_ids = project.tree.children(root_id).to_vec();

    fs::write(dir.path().join("zzz.txt"), "").unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    let diff = project.tree.refresh_dir(root_id, entries);

    assert_eq!(
        diff.ops,
        vec![DirDiffOp::Insert { first: 3, last: 3 }],
        "the new entry sorts after the three existing ones and adds exactly one row"
    );
    assert_eq!(
        names(&project.tree, root_id),
        vec!["empty_dir", "src", "README.md", "zzz.txt"]
    );
    // The three survivors keep their ids — a persistent QModelIndex on
    // any of them would still be valid.
    for &id in &old_ids {
        assert!(project.tree.children(root_id).contains(&id));
    }
}

#[test]
fn refresh_dir_removes_a_deleted_entry_as_a_single_remove() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let mut project = open_folder(dir.path()).unwrap();
    let root_id = project.tree.root_id();

    fs::remove_dir_all(dir.path().join("empty_dir")).unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    let diff = project.tree.refresh_dir(root_id, entries);

    assert_eq!(diff.ops, vec![DirDiffOp::Remove { first: 0, last: 0 }]);
    assert_eq!(names(&project.tree, root_id), vec!["src", "README.md"]);
}

#[test]
fn refresh_dir_preserves_the_loaded_subtree_of_a_survivor() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let mut project = open_folder(dir.path()).unwrap();
    let root_id = project.tree.root_id();
    let src_id = project
        .tree
        .children(root_id)
        .iter()
        .find(|&&id| project.tree.node(id).name == "src")
        .copied()
        .unwrap();
    let src_entries = list_dir(&dir.path().join("src"), SortOrder::Ascending).unwrap();
    project.tree.attach_children(src_id, src_entries);
    assert_eq!(project.tree.load_state(src_id), LoadState::Loaded);

    // An unrelated change at the root must not disturb `src`'s id or
    // its already-loaded children.
    fs::write(dir.path().join("new_at_root.txt"), "").unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    project.tree.refresh_dir(root_id, entries);

    assert!(project.tree.children(root_id).contains(&src_id));
    assert_eq!(project.tree.load_state(src_id), LoadState::Loaded);
    assert_eq!(names(&project.tree, src_id), vec!["lib.rs", "main.rs"]);
}

#[test]
fn refresh_dir_treats_a_rename_as_remove_then_insert() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("old.txt"), "").unwrap();
    let mut project = open_folder(dir.path()).unwrap();
    let root_id = project.tree.root_id();
    let old_id = project.tree.children(root_id)[0];

    fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    let diff = project.tree.refresh_dir(root_id, entries);

    assert_eq!(
        diff.ops,
        vec![
            DirDiffOp::Remove { first: 0, last: 0 },
            DirDiffOp::Insert { first: 0, last: 0 },
        ]
    );
    let new_id = project.tree.children(root_id)[0];
    assert_ne!(old_id, new_id, "a rename is a fresh id, not a reused one");
    assert_eq!(project.tree.node(new_id).name, "new.txt");
}

#[test]
fn tombstoned_ids_are_reused_only_for_a_new_insert_not_a_surviving_sibling() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "").unwrap();
    fs::write(dir.path().join("b.txt"), "").unwrap();
    let mut project = open_folder(dir.path()).unwrap();
    let root_id = project.tree.root_id();
    let b_id = project
        .tree
        .children(root_id)
        .iter()
        .find(|&&id| project.tree.node(id).name == "b.txt")
        .copied()
        .unwrap();

    fs::remove_file(dir.path().join("a.txt")).unwrap();
    fs::write(dir.path().join("c.txt"), "").unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    project.tree.refresh_dir(root_id, entries);

    // `b.txt` survived untouched; its id must be exactly what it was.
    assert!(project.tree.children(root_id).contains(&b_id));
    assert_eq!(project.tree.node(b_id).name, "b.txt");
    let c_id = project
        .tree
        .children(root_id)
        .iter()
        .find(|&&id| project.tree.node(id).name == "c.txt")
        .copied()
        .unwrap();
    assert_ne!(
        c_id, b_id,
        "the reused slot must not collide with a survivor"
    );
}

#[test]
fn unreadable_root_does_not_fail_the_open() {
    use std::os::unix::fs::PermissionsExt;
    let is_root = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false);
    if is_root {
        eprintln!("skipping unreadable_root_does_not_fail_the_open: running as root");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let unreadable = dir.path().join("locked");
    fs::create_dir(&unreadable).unwrap();
    let mut perms = fs::metadata(&unreadable).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&unreadable, perms.clone()).unwrap();

    // `open_folder_sorted`'s own pre-check (`fs::read_dir`) already
    // catches an unreadable *root*; this pins down that
    // `DirectoryTree::open_root` itself would treat that failure as
    // Loaded-empty rather than propagating it, if it were ever called
    // directly on such a path (defence in depth, not the primary path).
    let tree = DirectoryTree::open_root(&unreadable, SortOrder::Ascending);
    assert_eq!(tree.load_state(tree.root_id()), LoadState::Loaded);
    assert!(tree.children(tree.root_id()).is_empty());

    perms.set_mode(0o755);
    fs::set_permissions(&unreadable, perms).unwrap();
}

#[test]
fn set_sort_order_resorts_loaded_directories_without_touching_disk() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();
    assert_eq!(session.sort_order(), SortOrder::Ascending);

    session.set_sort_order(SortOrder::Descending);
    assert_eq!(session.sort_order(), SortOrder::Descending);
    let tree = &session.current().unwrap().tree;
    assert_eq!(
        names(tree, tree.root_id()),
        vec!["src", "empty_dir", "README.md"]
    );
}

#[test]
fn first_unloaded_ancestor_walks_down_one_level_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("a/b")).unwrap();
    fs::write(dir.path().join("a/b/file.txt"), "").unwrap();
    let project = open_folder(dir.path()).unwrap();
    let target = dir.path().join("a/b/file.txt");

    // Root is loaded, `a` is not yet — `a` itself is the first unloaded
    // ancestor.
    let a_path = dir.path().join("a");
    assert_eq!(
        project.tree.first_unloaded_ancestor(&target),
        Some(a_path.clone())
    );
}

#[test]
fn first_unloaded_ancestor_is_none_once_the_immediate_parent_is_loaded() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("file.txt"), "").unwrap();
    let project = open_folder(dir.path()).unwrap();
    let target = dir.path().join("file.txt");

    assert_eq!(project.tree.first_unloaded_ancestor(&target), None);
}

#[test]
fn node_by_path_is_updated_on_insert_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "").unwrap();
    let mut project = open_folder(dir.path()).unwrap();
    let a_path = dir.path().join("a.txt");
    assert!(project.tree.node_by_path(&a_path).is_some());

    fs::remove_file(&a_path).unwrap();
    let entries = list_dir(dir.path(), SortOrder::Ascending).unwrap();
    project.tree.refresh_dir(project.tree.root_id(), entries);
    assert!(project.tree.node_by_path(&a_path).is_none());
}

#[test]
fn open_folder_sorted_applies_the_requested_order() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());

    let project = open_folder_sorted(dir.path(), SortOrder::Descending).unwrap();
    let tree = &project.tree;
    // Folders still lead either way; only the name order within each
    // group flips.
    assert_eq!(
        names(tree, tree.root_id()),
        vec!["src", "empty_dir", "README.md"]
    );
}

/// `install_project`/`install_watcher` are the swap-in half of an
/// off-thread open/watcher-registration (ADR-0037).
#[test]
fn install_project_replaces_the_current_project() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let project = open_folder_sorted(dir.path(), SortOrder::Ascending).unwrap();

    let mut session = ProjectSession::new();
    assert!(session.current().is_none());
    let (replaced, replaced_watcher) = session.install_project(project);
    assert!(replaced.is_none());
    assert!(replaced_watcher.is_none());
    assert_eq!(session.current().unwrap().root.path(), dir.path());
}

#[test]
fn install_project_hands_back_the_project_it_replaced() {
    let first_dir = tempfile::tempdir().unwrap();
    make_fixture_tree(first_dir.path());
    let second_dir = tempfile::tempdir().unwrap();
    make_fixture_tree(second_dir.path());

    let mut session = ProjectSession::new();
    session.install_project(open_folder_sorted(first_dir.path(), SortOrder::Ascending).unwrap());
    let (replaced, _) = session
        .install_project(open_folder_sorted(second_dir.path(), SortOrder::Ascending).unwrap());

    assert_eq!(replaced.unwrap().root.path(), first_dir.path());
    assert_eq!(session.current().unwrap().root.path(), second_dir.path());
}

#[test]
fn install_project_hands_back_the_previous_watcher() {
    let first_dir = tempfile::tempdir().unwrap();
    make_fixture_tree(first_dir.path());
    let second_dir = tempfile::tempdir().unwrap();
    make_fixture_tree(second_dir.path());

    let mut session = ProjectSession::new();
    session.install_project(open_folder_sorted(first_dir.path(), SortOrder::Ascending).unwrap());
    let watcher = ProjectWatcher::start(first_dir.path(), false, |_, _| {}).unwrap();
    assert!(session.install_watcher(first_dir.path(), watcher).is_ok());

    let (_, replaced_watcher) = session
        .install_project(open_folder_sorted(second_dir.path(), SortOrder::Ascending).unwrap());

    assert!(replaced_watcher.is_some());
}

#[test]
fn install_watcher_is_rejected_when_no_project_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = ProjectWatcher::start(dir.path(), false, |_, _| {}).unwrap();

    let mut session = ProjectSession::new();
    assert!(session.install_watcher(dir.path(), watcher).is_err());
}

#[test]
fn install_watcher_is_rejected_when_the_root_no_longer_matches() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let other_dir = tempfile::tempdir().unwrap();
    let watcher = ProjectWatcher::start(other_dir.path(), false, |_, _| {}).unwrap();

    // A different project opened while registration was in flight — the
    // watcher started for `other_dir` must not be installed for `dir`.
    assert!(session.install_watcher(other_dir.path(), watcher).is_err());
}

#[test]
fn install_watcher_applies_and_returns_the_watcher_it_replaced() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let first = ProjectWatcher::start(dir.path(), false, |_, _| {}).unwrap();
    let Ok(none_replaced) = session.install_watcher(dir.path(), first) else {
        panic!("root still matches");
    };
    assert!(none_replaced.is_none());

    let second = ProjectWatcher::start(dir.path(), false, |_, _| {}).unwrap();
    let Ok(replaced) = session.install_watcher(dir.path(), second) else {
        panic!("root still matches");
    };
    assert!(replaced.is_some());
}

#[test]
fn attach_dir_children_is_dropped_when_the_root_no_longer_matches() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let other_dir = tempfile::tempdir().unwrap();
    let entries = list_dir(&dir.path().join("sub"), SortOrder::Ascending).unwrap();
    assert!(session
        .attach_dir_children(other_dir.path(), &dir.path().join("sub"), entries)
        .is_none());
}

#[test]
fn attach_dir_children_applies_for_the_still_open_root() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/f.txt"), "").unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let entries = list_dir(&dir.path().join("sub"), SortOrder::Ascending).unwrap();
    let inserted = session
        .attach_dir_children(dir.path(), &dir.path().join("sub"), entries)
        .unwrap();
    assert_eq!(inserted.len(), 1);
    let sub_id = session
        .current()
        .unwrap()
        .tree
        .node_by_path(&dir.path().join("sub"))
        .unwrap();
    assert_eq!(
        session.current().unwrap().tree.load_state(sub_id),
        LoadState::Loaded
    );
}

#[test]
fn refresh_dir_is_a_noop_for_an_unloaded_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let entries = list_dir(&dir.path().join("sub"), SortOrder::Ascending).unwrap();
    assert!(session
        .refresh_dir(dir.path(), &dir.path().join("sub"), entries)
        .is_none());
}

#[test]
fn opening_nonexistent_path_errors_without_mutating_state() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let config_dir = tempfile::tempdir().unwrap();

    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();
    assert_eq!(session.current().unwrap().root.path(), dir.path());

    let missing = dir.path().join("does-not-exist");
    let result = session.open_folder(&missing, config_dir.path());
    assert!(matches!(result, Err(OpenFolderError::NotFound(_))));

    // Current project must be unchanged after the failed open.
    assert_eq!(session.current().unwrap().root.path(), dir.path());
}

#[test]
fn opening_unreadable_path_errors_without_mutating_state() {
    use std::os::unix::fs::PermissionsExt;

    // Permission bits don't block root, and our mandatory Docker build
    // runs tests as root — skip rather than assert a false positive.
    let is_root = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false);
    if is_root {
        eprintln!(
            "skipping opening_unreadable_path_errors_without_mutating_state: running as root"
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    let config_dir = tempfile::tempdir().unwrap();

    let mut session = ProjectSession::new();
    session.open_folder(dir.path(), config_dir.path()).unwrap();

    let unreadable = tempfile::tempdir().unwrap();
    let mut perms = fs::metadata(unreadable.path()).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(unreadable.path(), perms.clone()).unwrap();

    let result = session.open_folder(unreadable.path(), config_dir.path());
    assert!(matches!(result, Err(OpenFolderError::NotReadable(_, _))));
    assert_eq!(session.current().unwrap().root.path(), dir.path());

    // restore perms so tempdir cleanup can remove it
    perms.set_mode(0o755);
    fs::set_permissions(unreadable.path(), perms).unwrap();
}

#[test]
fn last_opened_project_persists_and_reopens() {
    let project_dir = tempfile::tempdir().unwrap();
    make_fixture_tree(project_dir.path());
    let config_dir = tempfile::tempdir().unwrap();

    let mut session = ProjectSession::new();
    session
        .open_folder(project_dir.path(), config_dir.path())
        .unwrap();

    // Simulate a fresh app launch: a brand-new session, same config dir.
    let mut reopened_session = ProjectSession::new();
    let opened = reopened_session.reopen_last(config_dir.path()).unwrap();

    assert!(opened);
    assert_eq!(
        reopened_session.current().unwrap().root.path(),
        project_dir.path()
    );
}

#[test]
fn reopen_last_with_nothing_persisted_is_a_noop() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut session = ProjectSession::new();
    let opened = session.reopen_last(config_dir.path()).unwrap();
    assert!(!opened);
    assert!(session.current().is_none());
}

#[test]
fn create_file_appears_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file(dir.path(), "new.txt").unwrap();
    assert!(path.is_file());
    assert_eq!(path, dir.path().join("new.txt"));
}

#[test]
fn create_file_errors_when_name_taken() {
    let dir = tempfile::tempdir().unwrap();
    create_file(dir.path(), "dup.txt").unwrap();
    let result = create_file(dir.path(), "dup.txt");
    assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
}

#[test]
fn create_folder_appears_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_folder(dir.path(), "newdir").unwrap();
    assert!(path.is_dir());
}

#[test]
fn create_folder_errors_when_name_taken() {
    let dir = tempfile::tempdir().unwrap();
    create_folder(dir.path(), "dup").unwrap();
    let result = create_folder(dir.path(), "dup");
    assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
}

#[test]
fn rename_path_moves_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file(dir.path(), "old.txt").unwrap();
    let new_path = rename_path(&path, "renamed.txt").unwrap();
    assert!(!path.exists());
    assert!(new_path.is_file());
    assert_eq!(new_path, dir.path().join("renamed.txt"));
}

#[test]
fn rename_path_errors_when_target_name_taken() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file(dir.path(), "a.txt").unwrap();
    create_file(dir.path(), "b.txt").unwrap();
    let result = rename_path(&path, "b.txt");
    assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
    assert!(path.exists(), "original must be untouched on error");
}

#[test]
fn rename_path_errors_when_source_missing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("ghost.txt");
    let result = rename_path(&missing, "renamed.txt");
    assert!(matches!(result, Err(FileOpError::NotFound(_))));
}

#[test]
fn delete_path_removes_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_file(dir.path(), "gone.txt").unwrap();
    delete_path(&path).unwrap();
    assert!(!path.exists());
}

#[test]
fn delete_path_removes_nonempty_folder_recursively() {
    let dir = tempfile::tempdir().unwrap();
    let folder = create_folder(dir.path(), "subdir").unwrap();
    create_file(&folder, "inside.txt").unwrap();
    fs::create_dir(folder.join("nested")).unwrap();
    fs::write(folder.join("nested/deep.txt"), "x").unwrap();

    delete_path(&folder).unwrap();
    assert!(!folder.exists());
}

#[test]
fn delete_path_errors_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    let result = delete_path(&missing);
    assert!(matches!(result, Err(FileOpError::NotFound(_))));
}

#[test]
fn walk_all_entries_lists_everything_except_the_search_index() {
    let dir = tempfile::tempdir().unwrap();
    make_fixture_tree(dir.path());
    fs::create_dir(dir.path().join(".ide-index")).unwrap();
    fs::write(dir.path().join(".ide-index/meta.json"), "{}").unwrap();

    let entries = walk_all_entries(dir.path());
    let paths: Vec<PathBuf> = entries.iter().map(|(p, _)| p.clone()).collect();
    assert!(paths.contains(&dir.path().join("src/main.rs")));
    assert!(paths.contains(&dir.path().join("src/lib.rs")));
    assert!(!paths
        .iter()
        .any(|p| p.starts_with(dir.path().join(".ide-index"))));
    assert!(
        !paths.contains(&dir.path().to_path_buf()),
        "the root itself is excluded"
    );
}

#[test]
fn walk_all_entries_respects_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
    fs::create_dir(dir.path().join("ignored")).unwrap();
    fs::write(dir.path().join("ignored/x.txt"), "").unwrap();
    fs::write(dir.path().join("tracked.txt"), "").unwrap();

    let entries = walk_all_entries(dir.path());
    let paths: Vec<PathBuf> = entries.iter().map(|(p, _)| p.clone()).collect();
    assert!(paths.contains(&dir.path().join("tracked.txt")));
    assert!(!paths
        .iter()
        .any(|p| p.starts_with(dir.path().join("ignored"))));
}
