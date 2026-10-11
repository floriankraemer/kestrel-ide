#pragma once

#include <QString>

class QMenu;

namespace ui_shell {

struct ProjectTreeActions;

// Appends "Run / Debug / Run with Coverage '<name>'" for the tree row at
// `path` (a directory when `isDir`; the project root for the tree's empty
// area) to the open context menu: PHPUnit/Pest tests in a folder, in a test
// file, or the whole suite.
//
// Which name applies, or whether the row holds tests at all, is
// `TestService::treeRunName`'s answer; a no-op when it is empty. The entries
// create or refresh a temporary configuration and then run it exactly as
// the toolbar would, so before-launch tasks and the toolbar selection follow.
void appendTestEntries(QMenu &menu, const QString &path, bool isDir,
                       const ProjectTreeActions &actions);

} // namespace ui_shell
