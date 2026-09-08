#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

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
public:
    // `openAt(path, line, column)` jumps the editor to a failure's location
    // — the same contract every other panel with a clickable location uses.
    using OpenAt = std::function<void(const QString &, int, int)>;

    TestsPanel(TestService *testService, OpenAt openAt, QWidget *parent);

    // Targets of the Tests menu / shortcuts, so they act regardless of
    // which widget has focus (the same arrangement `BuildPanel` has with
    // the build shortcuts).
    void runAllTests();
    void runFailedTests();
    void stopTests();

private:
    void onTestRunStarted();
    void onTestTreeChanged();
    void onTestOutputAppended(const QString &text);
    void onTestRunFinished(bool ok, const QString &message);
    void onSelectionChanged();
    void onFailureLinkActivated(int textPosition);
    void showContextMenu(const QPoint &pos);
    void report(const FfiResult &result);
    void updateToolbarEnablement();

    TestService *testService_;
    OpenAt openAt_;

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
                           TestsPanel::OpenAt openAt);

} // namespace ui_shell
