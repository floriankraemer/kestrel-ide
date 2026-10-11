#include "project_tree_test_menu.h"

#include "project_tree_dock.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>

#include <functional>
#include <optional>

namespace ui_shell {

namespace {

// The run's name as PhpStorm words it; which kind applies is Rust's answer.
QString runName(const FfiTreeRun &run)
{
    switch (run.kind) {
    case FfiTreeRunKind::AllTests:
        return QObject::tr("All tests");
    case FfiTreeRunKind::Directory:
        return QObject::tr("Tests in '%1'").arg(run.subject);
    case FfiTreeRunKind::Named:
        break;
    }
    return run.subject;
}

} // namespace

void appendTestEntries(QMenu &menu, const QString &path, bool isDir,
                       const ProjectTreeActions &actions)
{
    TestService *tests = actions.testService;
    if (tests == nullptr || actions.runService == nullptr) {
        return;
    }
    const std::optional<QString> open =
      actions.openText ? actions.openText(path) : std::optional<QString>();
    const FfiTreeRun run = tests->treeRun(path, isDir, open.value_or(QString()), open.has_value());
    if (!run.runnable) {
        return;
    }
    const QString name = runName(run);
    menu.addSeparator();
    QMainWindow *window = actions.window;
    // Storing the configuration first is what puts it in the toolbar combo;
    // the launch itself is then the ordinary Run / Debug / Coverage of it.
    const auto add = [&](const QString &label, const QString &title,
                         std::function<FfiResult(const QString &)> launch) {
        QObject::connect(menu.addAction(label), &QAction::triggered, &menu, [=]() {
            const FfiConfigIdResult remembered = tests->rememberTreeRun(path, isDir);
            FfiResult result = remembered.result;
            if (result.code == 0) {
                result = launch(remembered.config_id);
            }
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
