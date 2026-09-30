//! Which icon-pack entry each Build Tools row and toolbar button paints as.
//!
//! The rule half of the Build Tools panel's icons: `jvm-build-core` knows a
//! row's kind and tool and nothing about icon packs, `icon-theme` knows a
//! pack and nothing about Gradle, so the two meet here (ADR-0026), the same
//! join [`crate::build_tools_tree`] makes for the project tree. The view only
//! asks for a key and paints it; resolution (appearance, pack fallback to the
//! pack's default file/folder) is [`crate::icons::IconService::pack_icon_key`].

use jvm_build_core::model::Tool;
use jvm_build_core::view::{GroupKind, NodeKind};

use crate::build_tools_tree::FolderRole;

/// A question put to the icon pack, answered by its own tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackIcon {
    /// What the pack paints for a file with this name (`pom.xml`, `a.jar`),
    /// so a pack's own name/extension art wins and a pack with none falls
    /// back to its default file icon.
    File(&'static str),
    /// What the pack paints for a folder with this name, collapsed and
    /// expanded; a pack with none falls back to its default folder.
    Folder(&'static str),
    /// One icon by its id in the pack, for art no file or folder name
    /// reaches (`settings`). A pack lacking the id paints its default file.
    Id(&'static str),
}

/// A toolbar button whose icon comes from the pack. The rest of the panel's
/// toolbar (reload, run, offline, conflicts) has no pack art that reads
/// clearly at 16px and uses the app's own icon set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarIcon {
    Settings,
    SkipTests,
}

/// The pack entry for a tree row; `None` for a row that paints no icon (a
/// Maven profile, which is a checkbox).
///
/// `source_role` is what a `SourceRoot` row reports (its own kind/content),
/// so a `src/main/java` and a `src/test/java` row get the pack's `src` and
/// `test` art whatever the directories are literally called; `group` does the
/// same for a `Group` row's section.
pub fn row_icon(
    kind: NodeKind,
    tool: Tool,
    source_role: Option<FolderRole>,
    group: Option<GroupKind>,
) -> Option<PackIcon> {
    let icon = match kind {
        NodeKind::ToolRoot => match tool {
            Tool::Gradle => PackIcon::File("build.gradle"),
            Tool::Maven => PackIcon::File("pom.xml"),
        },
        // Something you run, not something you are: the tool's own mark on
        // every task row drowned the root row's.
        NodeKind::Task | NodeKind::Goal => PackIcon::Id("console"),
        NodeKind::Group => PackIcon::Folder(match group {
            Some(GroupKind::Modules) => "components",
            Some(GroupKind::Plugins) => "plugins",
            Some(GroupKind::Dependencies) => "dependencies",
            Some(GroupKind::DependencyModule) => module_folder(tool),
            Some(GroupKind::Configuration) => "configuration",
            Some(GroupKind::Profiles) => "environments",
            Some(GroupKind::Tasks | GroupKind::TaskGroup | GroupKind::Lifecycle) | None => "tasks",
        }),
        NodeKind::Module => PackIcon::Folder(module_folder(tool)),
        NodeKind::SourceRoot => {
            PackIcon::Folder(source_role.unwrap_or(FolderRole::Main).canonical_name())
        }
        NodeKind::Dependency => PackIcon::File("dependency.jar"),
        NodeKind::Plugin => PackIcon::Folder("plugins"),
        NodeKind::Profile => return None,
    };
    Some(icon)
}

/// A module's folder art: the pack's Gradle folder, or its Java one for Maven
/// (Material has no Maven folder).
fn module_folder(tool: Tool) -> &'static str {
    match tool {
        Tool::Gradle => "gradle",
        Tool::Maven => "java",
    }
}

/// The pack entry for a pack-drawn toolbar button.
pub fn toolbar_icon(button: ToolbarIcon) -> PackIcon {
    match button {
        ToolbarIcon::Settings => PackIcon::Id("settings"),
        ToolbarIcon::SkipTests => PackIcon::Folder("test"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_root_takes_the_tools_own_file_art() {
        assert_eq!(
            row_icon(NodeKind::ToolRoot, Tool::Gradle, None, None),
            Some(PackIcon::File("build.gradle"))
        );
        assert_eq!(
            row_icon(NodeKind::ToolRoot, Tool::Maven, None, None),
            Some(PackIcon::File("pom.xml"))
        );
    }

    #[test]
    fn a_task_and_a_goal_paint_as_something_runnable() {
        for tool in [Tool::Gradle, Tool::Maven] {
            for kind in [NodeKind::Task, NodeKind::Goal] {
                assert_eq!(
                    row_icon(kind, tool, None, None),
                    Some(PackIcon::Id("console"))
                );
            }
        }
    }

    #[test]
    fn a_module_folder_differs_per_tool() {
        assert_eq!(
            row_icon(NodeKind::Module, Tool::Gradle, None, None),
            Some(PackIcon::Folder("gradle"))
        );
        assert_eq!(
            row_icon(NodeKind::Module, Tool::Maven, None, None),
            Some(PackIcon::Folder("java"))
        );
    }

    #[test]
    fn a_source_root_takes_the_folder_art_of_its_role_not_its_directory_name() {
        for (role, name) in [
            (FolderRole::Main, "src"),
            (FolderRole::Test, "test"),
            (FolderRole::Resource, "resources"),
        ] {
            assert_eq!(
                row_icon(NodeKind::SourceRoot, Tool::Maven, Some(role), None),
                Some(PackIcon::Folder(name))
            );
        }
    }

    #[test]
    fn group_dependency_and_plugin_rows_have_their_own_art() {
        assert_eq!(
            row_icon(NodeKind::Group, Tool::Gradle, None, None),
            Some(PackIcon::Folder("tasks"))
        );
        assert_eq!(
            row_icon(NodeKind::Dependency, Tool::Maven, None, None),
            Some(PackIcon::File("dependency.jar"))
        );
        assert_eq!(
            row_icon(NodeKind::Plugin, Tool::Maven, None, None),
            Some(PackIcon::Folder("plugins"))
        );
    }

    #[test]
    fn a_group_row_takes_the_art_of_its_section() {
        let folder = |group, tool| row_icon(NodeKind::Group, tool, None, Some(group));
        for (group, name) in [
            (GroupKind::Tasks, "tasks"),
            (GroupKind::TaskGroup, "tasks"),
            (GroupKind::Lifecycle, "tasks"),
            (GroupKind::Modules, "components"),
            (GroupKind::Plugins, "plugins"),
            (GroupKind::Dependencies, "dependencies"),
            (GroupKind::Configuration, "configuration"),
            (GroupKind::Profiles, "environments"),
        ] {
            assert_eq!(folder(group, Tool::Gradle), Some(PackIcon::Folder(name)));
        }
        assert_eq!(
            folder(GroupKind::DependencyModule, Tool::Gradle),
            Some(PackIcon::Folder("gradle"))
        );
        assert_eq!(
            folder(GroupKind::DependencyModule, Tool::Maven),
            Some(PackIcon::Folder("java"))
        );
    }

    #[test]
    fn a_profile_row_paints_no_icon() {
        assert_eq!(row_icon(NodeKind::Profile, Tool::Maven, None, None), None);
    }

    #[test]
    fn pack_drawn_toolbar_buttons() {
        assert_eq!(
            toolbar_icon(ToolbarIcon::Settings),
            PackIcon::Id("settings")
        );
        assert_eq!(
            toolbar_icon(ToolbarIcon::SkipTests),
            PackIcon::Folder("test")
        );
    }
}
