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

} // namespace ui_shell
