#pragma once

#include <QString>

class QMenu;

namespace ui_shell {

struct ProjectTreeActions;

// Appends a "Git" submenu to the project tree's open context menu, for the
// file at `absolutePath`.
//
// A no-op when the project is not a repository: five permanently disabled
// entries teach the user nothing, so the submenu is absent instead.
//
// Its own translation unit because `project_tree_dock.cpp`'s context-menu
// lambda is already long enough that a fourth concern inside it is the
// thing a reader has to skip past to find the first three.
void appendGitSubmenu(QMenu &menu, const QString &absolutePath,
                       const ProjectTreeActions &actions);

// The project root's own "Git" submenu (R6): "Compare Project with Branch,
// Tag or Revision…", which lists the files `git diff --name-only
// <revision>` reports and opens the picked one in the diff tab. Appended
// when the context menu is opened on the tree's empty area, i.e. the
// project itself. Same no-op-outside-a-repository rule as above.
void appendProjectGitSubmenu(QMenu &menu, const ProjectTreeActions &actions);

} // namespace ui_shell
