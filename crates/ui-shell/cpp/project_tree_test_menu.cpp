#include "project_tree_test_menu.h"

#include "project_tree_dock.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>

#include <functional>

namespace ui_shell {

void appendTestEntries(QMenu &menu, const QString &path, bool isDir,
                       const ProjectTreeActions &actions)
{
    TestService *tests = actions.testService;
    if (tests == nullptr || actions.runService == nullptr) {
        return;
    }
    const QString name = tests->treeRunName(path, isDir);
    if (name.isEmpty()) {
        return;
    }
    menu.addSeparator();
    QMainWindow *window = actions.window;
    // Storing the configuration first is what puts it in the toolbar combo;
    // the launch itself is then the ordinary Run / Debug / Coverage of it.
    const auto add = [&](const QString &label, const QString &title,
                         std::function<FfiResult(const QString &)> launch) {
        QObject::connect(menu.addAction(label), &QAction::triggered, &menu,
                          [=]() {
                              const QString id = tests->rememberTreeRun(path, isDir);
                              if (id.isEmpty()) {
                                  return;
                              }
                              const FfiResult result = launch(id);
                              if (result.code != 0) {
                                  QMessageBox::warning(window, title, result.message);
                              }
                          });
    };
    RunService *runs = actions.runService;
    DebugService *debug = actions.debugService;
    add(QObject::tr("Run '%1'").arg(name), QObject::tr("Run"),
        [runs](const QString &id) { return runs->run(id); });
    if (debug != nullptr) {
        add(QObject::tr("Debug '%1'").arg(name), QObject::tr("Debug"),
            [debug](const QString &id) { return debug->debug(id); });
    }
    add(QObject::tr("Run '%1' with Coverage").arg(name), QObject::tr("Run"),
        [runs](const QString &id) { return runs->runWithCoverage(id); });
}

} // namespace ui_shell
