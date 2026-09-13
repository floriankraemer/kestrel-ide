#include "vcs_gutter.h"

#include "e2e_mark.h"
#include "theme.h"

#include <QLabel>
#include <QMenu>
#include <QObject>
#include <QPoint>
#include <QStringList>
#include <QWidget>
#include <QWidgetAction>

namespace ui_shell {

QColor changeMarkerColor(ChangeMarkerKind kind, ChangeMarkerState state)
{
    const DiffColors colors = diffColors();
    QColor base;
    switch (kind) {
    case ChangeMarkerKind::Added:
        base = colors.addedMarker;
        break;
    case ChangeMarkerKind::Removed:
        base = colors.deletedMarker;
        break;
    case ChangeMarkerKind::Modified:
        base = colors.modifiedMarker;
        break;
    }
    switch (state) {
    case ChangeMarkerState::Unstaged:
        return base;
    case ChangeMarkerState::Staged:
        return base.lighter(160);
    case ChangeMarkerState::Both:
        return base.darker(140);
    }
    return base;
}

namespace {

QString stateLabel(ChangeMarkerState state)
{
    switch (state) {
    case ChangeMarkerState::Unstaged:
        return QObject::tr("Not staged");
    case ChangeMarkerState::Staged:
        return QObject::tr("Staged");
    case ChangeMarkerState::Both:
        return QObject::tr("Partially staged");
    }
    return QString();
}

} // namespace

void showHunkPopup(QWidget *parent, const QPoint &globalPos, const HunkPopupActions &actions)
{
    QMenu menu(parent);
    // Same helper `project_tree_git_menu.cpp`'s submenu uses — an E2E flow
    // needs a real on-screen rect to click "Stage Hunk" by, the same way it
    // already clicks that submenu's "Stage File".
    e2eMarkMenuActions(&menu, "vcs_hunk_menu_action");

    // Header: the stage state, then the removed lines inline (R6) — a
    // label inside the menu rather than a second popup, so what a hunk
    // took away is one click away, like IDEA's own gutter popup.
    auto *header = new QLabel(&menu);
    QString headerText = QStringLiteral("<b>%1</b>").arg(stateLabel(actions.state).toHtmlEscaped());
    if (!actions.removedText.isEmpty()) {
        QStringList removed;
        for (const QString &line : actions.removedText.split(QLatin1Char('\n'))) {
            removed.append(QStringLiteral("- %1").arg(line.toHtmlEscaped()));
        }
        headerText += QStringLiteral("<pre style=\"margin:0\">%1</pre>").arg(removed.join(QLatin1Char('\n')));
    }
    header->setText(headerText);
    header->setTextFormat(Qt::RichText);
    header->setContentsMargins(8, 4, 8, 4);
    auto *headerAction = new QWidgetAction(&menu);
    headerAction->setDefaultWidget(header);
    menu.addAction(headerAction);
    menu.addSeparator();

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
