//! The project tree's source-root / output-dir decoration join (the
//! jvm-build-tools plan's B7, ADR-0057).
//!
//! `jvm-build-core` owns the synced [`jvm_build_core::model::BuildModel`]
//! and never learns a folder icon's name; `icon-theme` owns the icon pack
//! and never learns what a Gradle source set or a Maven module is. Wiring
//! the two together would give each a dependency it was deliberately built
//! without, so per [ADR-0026] they meet here — the same reasoning
//! [`crate::icons`]'s own module doc gives for the icon-theme join.
//!
//! [ADR-0026]: ../../../docs/architecture/decisions/0026-plugin-host.md

use std::path::Path;

use jvm_build_core::model::{BuildModel, SourceContent, SourceRootKind};

/// What a source-root directory is, for the icon it paints as — independent
/// of the directory's own name, since Maven's `src/main/java` is a
/// directory literally named `main`, not `src`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRole {
    Main,
    Test,
    Resource,
}

impl FolderRole {
    /// The icon pack's own folder-name key this role paints as
    /// (`icon_theme::IconPack::folder_icon`'s `folder_name` argument),
    /// substituted for the directory's real name so a `main`/`test` set
    /// still gets the pack's `src`/`test` art regardless of what the build
    /// tool happened to call it.
    pub fn canonical_name(self) -> &'static str {
        match self {
            FolderRole::Main => "src",
            FolderRole::Test => "test",
            FolderRole::Resource => "resources",
        }
    }
}

/// Is `path` one of `model`'s own source roots, and if so, what kind?
///
/// A pure join over whatever the sync itself already reported — no
/// filesystem access, no path-name convention of its own beyond what
/// [`FolderRole::canonical_name`] paints, extending `jvm-build-core`'s own
/// delegate-and-never-model rule (ADR-0040) to this decoration.
pub fn folder_role(path: &Path, model: &BuildModel) -> Option<FolderRole> {
    model
        .modules
        .iter()
        .flat_map(|module| &module.source_roots)
        .find(|root| root.path == path)
        .map(|root| match (root.kind, root.content) {
            (_, SourceContent::Resources) => FolderRole::Resource,
            (SourceRootKind::Test, _) => FolderRole::Test,
            (SourceRootKind::Main, _) => FolderRole::Main,
        })
}

/// Is `path` one of `model`'s own output directories — greyed in the tree,
/// the same restraint a `.gitignore`d directory already gets.
pub fn is_output_dir(path: &Path, model: &BuildModel) -> bool {
    model
        .modules
        .iter()
        .any(|module| module.output_dirs.iter().any(|dir| dir == path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use jvm_build_core::model::{Module, SourceRoot, Tool};

    fn model_with(source_roots: Vec<SourceRoot>, output_dirs: Vec<PathBuf>) -> BuildModel {
        BuildModel {
            tool: Tool::Maven,
            root: PathBuf::from("/proj"),
            root_name: "proj".to_string(),
            modules: vec![Module {
                path: "app".to_string(),
                name: "app".to_string(),
                dir: PathBuf::from("/proj/app"),
                build_file: PathBuf::from("/proj/app/pom.xml"),
                source_roots,
                output_dirs,
                jdk: None,
                dependencies: vec![],
                plugins: vec![],
                children: vec![],
            }],
            tasks: vec![],
            warnings: vec![],
            profiles: vec![],
            synced_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_main_java_root_named_main_still_resolves_to_the_src_role() {
        let model = model_with(
            vec![SourceRoot {
                path: PathBuf::from("/proj/app/src/main/java"),
                kind: SourceRootKind::Main,
                content: SourceContent::Java,
            }],
            vec![],
        );
        let role = folder_role(Path::new("/proj/app/src/main/java"), &model);
        assert_eq!(role, Some(FolderRole::Main));
        assert_eq!(role.unwrap().canonical_name(), "src");
    }

    #[test]
    fn a_test_root_resolves_to_the_test_role_regardless_of_content() {
        let model = model_with(
            vec![SourceRoot {
                path: PathBuf::from("/proj/app/src/test/kotlin"),
                kind: SourceRootKind::Test,
                content: SourceContent::Kotlin,
            }],
            vec![],
        );
        assert_eq!(
            folder_role(Path::new("/proj/app/src/test/kotlin"), &model),
            Some(FolderRole::Test)
        );
    }

    #[test]
    fn a_resources_root_wins_over_its_main_test_kind() {
        let model = model_with(
            vec![SourceRoot {
                path: PathBuf::from("/proj/app/src/main/resources"),
                kind: SourceRootKind::Main,
                content: SourceContent::Resources,
            }],
            vec![],
        );
        assert_eq!(
            folder_role(Path::new("/proj/app/src/main/resources"), &model),
            Some(FolderRole::Resource)
        );
    }

    #[test]
    fn a_path_that_is_not_a_source_root_has_no_role() {
        let model = model_with(vec![], vec![]);
        assert_eq!(folder_role(Path::new("/proj/app/pom.xml"), &model), None);
    }

    #[test]
    fn an_output_directory_is_reported_as_such() {
        let model = model_with(vec![], vec![PathBuf::from("/proj/app/target")]);
        assert!(is_output_dir(Path::new("/proj/app/target"), &model));
        assert!(!is_output_dir(Path::new("/proj/app/src"), &model));
    }
}
