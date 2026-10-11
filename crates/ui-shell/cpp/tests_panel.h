#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QCoreApplication>
#include <QKeySequence>
#include <QString>
#include <QWidget>

#include <functional>
#include <optional>

class QLabel;
class QPlainTextEdit;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Tests dock (the PHP tooling plan's D5): a run's suite/class/method
// tree, filling in live as `TestService` streams TeamCity messages, with a
// failure pane for whichever node is selected.
//
// Humble view per CLAUDE.md: which nodes exist, their status, and a
// failure's message/details are `test-core`'s answers (`TestTree`, D2),
// read back through `TestService` exactly as `ProblemsPanel` reads
// `DiagnosticsService` — this widget builds the tree, turns a selection
// into a failure-pane refresh, and turns a context-menu click or a toolbar
// button into a call back into `TestService`.
class TestsPanel : public QWidget
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in `QWidget`'s
    // context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(TestsPanel)

public:
    // `openAt(path, line, column)` jumps the editor to a failure's location
    // — the same contract every other panel with a clickable location uses.
    using OpenAt = std::function<void(const QString &, int, int)>;
    // The text of `path` as the editor holds it, `nullopt` when it is not
    // open: Jump to Source finds the line in what the user sees.
    using OpenText = std::function<std::optional<QString>(const QString &)>;

    TestsPanel(TestService *testService, AppSettings *appSettings, OpenAt openAt, OpenText openText,
               QWidget *parent);

    // Targets of the Tests menu / shortcuts, so they act regardless of
    // which widget has focus (the same arrangement `BuildPanel` has with
    // the build shortcuts).
    void runAllTests();
    void runFailedTests();
    void stopTests();

    // E2E only: the toolbar's button rects, the same
    // `containers_panel.cpp`'s `refreshE2eRects` shape — `buildTestsDock`
    // calls this once per `visibilityChanged(true)`, since neither button
    // has a real screen geometry before the dock is actually shown.
    void markE2eToolbar() const;
    void markE2eRows() const;

private:
    void onTestRunStarted();
    void onTestTreeChanged();
    void onTestOutputAppended(const QString &text);
    void onTestRunFinished(bool ok, bool matchedNothing, const QString &message);
    void onSelectionChanged();
    void onFailureLinkActivated(int textPosition);
    void jumpToSource(const QString &nodeId);
    void showContextMenu(const QPoint &pos);
    void report(const FfiResult &result);
    void updateToolbarEnablement();

    // The tree's keys: `tests.jumpToSource` is rebindable and handled here,
    // not as a shortcut, so it cannot be ambiguous with a window action
    // bound to the same key (F4 is `database.goToReferencedRow`'s).
    bool eventFilter(QObject *watched, QEvent *event) override;
    QKeySequence jumpToSourceKey() const;

    TestService *testService_;
    AppSettings *appSettings_;
    OpenAt openAt_;
    OpenText openText_;

    QToolButton *runAllButton_ = nullptr;
    QToolButton *runFailedButton_ = nullptr;
    QToolButton *stopButton_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    QLabel *failureHeader_ = nullptr;
    QPlainTextEdit *failureDetails_ = nullptr;
    QPlainTextEdit *output_ = nullptr;

    // Rebuilt wholesale on every `testTreeChanged`, the same brute-force
    // refresh `ProblemsPanel::refresh` uses for its own tree — a test run's
    // event volume is nowhere near what would make that measurably slow.
    QHash<QString, QTreeWidgetItem *> itemsById_;
    QString selectedNodeId_;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"tests"` — the same one-call pattern `buildBuildDock` uses.
TestsPanel *buildTestsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                           ads::CDockAreaWidget *relativeTo, TestService *testService,
                           AppSettings *appSettings, TestsPanel::OpenAt openAt,
                           TestsPanel::OpenText openText);

} // namespace ui_shell
