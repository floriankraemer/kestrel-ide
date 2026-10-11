#include "tests_menu.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "keymap_page.h"
#include "tests_panel.h"

#include "DockWidget.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QTimer>

namespace ui_shell {

void buildTestsMenu(QMainWindow *window, AppSettings *appSettings,
                     QHash<QString, QAction *> &actions, DockRegistry *docks, QMenu *viewMenu)
{
    QAction *viewTestsAction = registerAction(viewMenu, QStringLiteral("view.tests"),
                                              QObject::tr("Tests"), appSettings, actions);
    QObject::connect(viewTestsAction, &QAction::triggered, window, [docks]() {
        docks->show(QStringLiteral("tests"));
        // A dock already up (a run showed it) gets no `visibilityChanged`:
        // its rows are reported here instead.
        auto *panel = dynamic_cast<TestsPanel *>(docks->dock(QStringLiteral("tests"))->widget());
        if (panel != nullptr && e2eMarksEnabled()) {
            QTimer::singleShot(0, panel, [panel]() {
                panel->markE2eToolbar();
                panel->markE2eRows();
            });
        }
    });
    QAction *viewCoverageAction = registerAction(viewMenu, QStringLiteral("view.coverage"),
                                                 QObject::tr("Coverage"), appSettings, actions);
    QObject::connect(viewCoverageAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("coverage")); });
}

} // namespace ui_shell
