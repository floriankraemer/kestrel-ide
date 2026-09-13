#include "vcs_gutter.h"

#include "theme.h"

#include <QMenu>
#include <QObject>
#include <QPoint>
#include <QWidget>

namespace ui_shell {

QColor changeMarkerColor(ChangeMarkerKind kind)
{
    const DiffColors colors = diffColors();
    switch (kind) {
    case ChangeMarkerKind::Added:
        return colors.addedMarker;
    case ChangeMarkerKind::Removed:
        return colors.deletedMarker;
    case ChangeMarkerKind::Modified:
        return colors.modifiedMarker;
    }
    return QColor();
}

void showHunkPopup(QWidget *parent, const QPoint &globalPos, const HunkPopupActions &actions)
{
    QMenu menu(parent);
    if (actions.showDiff) {
        QObject::connect(menu.addAction(QObject::tr("Show Diff")), &QAction::triggered, &menu,
                          [&actions]() { actions.showDiff(); });
    }
    if (actions.stageHunk) {
        QObject::connect(menu.addAction(QObject::tr("Stage Hunk")), &QAction::triggered, &menu,
                          [&actions]() { actions.stageHunk(); });
    }
    if (actions.unstageHunk) {
        QObject::connect(menu.addAction(QObject::tr("Unstage Hunk")), &QAction::triggered, &menu,
                          [&actions]() { actions.unstageHunk(); });
    }
    if (actions.stageFile) {
        QObject::connect(menu.addAction(QObject::tr("Stage File")), &QAction::triggered, &menu,
                          [&actions]() { actions.stageFile(); });
    }
    if (actions.revert) {
        QObject::connect(menu.addAction(QObject::tr("Revert Hunk")), &QAction::triggered, &menu,
                          [&actions]() { actions.revert(); });
    }
    if (menu.isEmpty()) {
        return;
    }
    menu.exec(globalPos);
}

} // namespace ui_shell
