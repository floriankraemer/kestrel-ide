#include "status_bar.h"

#include "dock_layout.h"
#include "editor_tabs.h"
#include "problems_panel.h"
#include "theme.h"
#include "vcs_menu.h"

#include <QApplication>
#include <QColor>
#include <QLabel>
#include <QMainWindow>
#include <QMenuBar>
#include <QProgressBar>
#include <QStatusBar>
#include <QToolButton>
#include <QTreeView>

namespace ui_shell {

namespace {

// Guards `showProjectOpening`/the clearing connections below against a
// stray `QApplication::restoreOverrideCursor()` with nothing pushed — e.g.
// `projectOpened` firing at startup when nothing ever called
// `showProjectOpening` (no explicit "Open Folder..."/Recent Projects click
// happened). One open at a time is the only case that matters in practice
// (a second explicit open before the first settles is rare and, worst
// case, just clears one open's indication a little early).
bool projectOpeningBusy = false;

} // namespace

void showProjectOpening(QMainWindow *window)
{
    if (projectOpeningBusy) {
        return;
    }
    projectOpeningBusy = true;
    QApplication::setOverrideCursor(Qt::WaitCursor);
    window->statusBar()->showMessage(QObject::tr("Opening project..."));
}

namespace {

void clearProjectOpening(QStatusBar *statusBar)
{
    if (!projectOpeningBusy) {
        return;
    }
    projectOpeningBusy = false;
    QApplication::restoreOverrideCursor();
    statusBar->clearMessage();
}

} // namespace

UiFontTargets buildStatusBar(QMainWindow *window, AppSettings *appSettings,
                              LanguageService *languageService, BuildService *buildService,
                              DiagnosticsService *diagnosticsService, SearchModel *searchModel,
                              VcsService *vcsService, EditorTabs *editorTabs,
                              QTreeView *projectTree, DockRegistry *docks,
                              ProblemsPanel *problemsPanel, ProjectTreeModel *treeModel)
{
    // L3: line:col + language update per current tab / cursor move; "UTF-8"
    // is static since only UTF-8 is supported today (US-2b's binary-file
    // rejection already rules out anything else reaching an open tab).
    auto *statusBar = window->statusBar();
    auto *languageLabel = new QLabel(statusBar);
    auto *positionLabel = new QLabel(statusBar);
    auto *encodingLabel = new QLabel(QStringLiteral("UTF-8"), statusBar);
    // Task L2: a compact problem counter, coloured by the worst severity
    // present and empty when there is nothing wrong. A button rather than a
    // label because clicking it opens the Problems dock.
    auto *problemsButton = new QToolButton(statusBar);
    problemsButton->setAutoRaise(true);
    problemsButton->setVisible(false);
    QObject::connect(problemsButton, &QToolButton::clicked, window, [docks, problemsPanel]() {
        docks->show(QStringLiteral("problems"));
        problemsPanel->focusTree();
    });
    const auto updateProblemsButton = [problemsButton, diagnosticsService]() {
        const FfiDiagnosticCounts counts = diagnosticsService->diagnosticCounts();
        const bool any = counts.errors > 0 || counts.warnings > 0;
        problemsButton->setVisible(any);
        if (!any) {
            return;
        }
        problemsButton->setText(QObject::tr("%1 errors, %2 warnings")
                                   .arg(counts.errors)
                                   .arg(counts.warnings));
        const QColor color = severityColor(counts.errors > 0 ? FfiSeverity::Error
                                                             : FfiSeverity::Warning);
        problemsButton->setStyleSheet(QStringLiteral("color: %1;").arg(color.name()));
    };
    // Either source changing means this counter is stale (ADR-0046): a
    // build's rows never used to reach it, same as they never used to
    // reach the editor's squiggles before `DiagnosticsService` existed.
    QObject::connect(languageService, &LanguageService::diagnosticsChanged, window,
                      updateProblemsButton);
    QObject::connect(buildService, &BuildService::diagnosticsChanged, window,
                      updateProblemsButton);
    // F3-18: the branch widget (vcs_menu.cpp).
    auto *branchButton = buildBranchWidget(vcsService, window, statusBar);
    // The project index builds on a background thread for seconds to minutes
    // after a folder is opened. Until this existed the only way to find that
    // out was to run a search and be told to try again later.
    // Two plain permanent widgets rather than a laid-out container: the
    // status bar already spaces its own children, and a container's label
    // stretches to fill whatever room is going, which pushed the bar a
    // hand's width away from its own caption.
    auto *indexLabel = new QLabel(statusBar);
    auto *indexBar = new QProgressBar(statusBar);
    indexLabel->setVisible(false);
    indexBar->setVisible(false);
    indexBar->setTextVisible(false);
    indexBar->setFixedWidth(90);
    indexBar->setFixedHeight(statusBar->fontMetrics().height());

    // Everything a font scale has to reach now exists: the menu bar is
    // created lazily by menuBar() just below, the tree came out of
    // buildCentralWidget, and the indexing bar is right above. Applied here
    // (rather than only in run_app) because the two per-widget scales have no
    // widget to land on until this point.
    const UiFontTargets uiFontTargets{window->menuBar(), projectTree, indexBar};
    applyUiFontScales(appSettings->uiFontScales(), uiFontTargets);
    QObject::connect(searchModel, &SearchModel::indexProgress, window,
                      [indexLabel, indexBar](quint32 done, quint32 total) {
                          const QString text =
                              QObject::tr("Indexing... %1/%2").arg(done).arg(total);
                          // Reserve the width of the widest reading this run
                          // will ever show — `total/total`. Without it the
                          // label is sized for "565/2223" one frame and
                          // "1204/2223" the next, and clips while it catches
                          // up.
                          indexLabel->setMinimumWidth(indexLabel->fontMetrics().horizontalAdvance(
                              QObject::tr("Indexing... %1/%2").arg(total).arg(total)));
                          indexLabel->setStyleSheet(QString());
                          indexLabel->setText(text);
                          indexBar->setRange(0, static_cast<int>(total));
                          indexBar->setValue(static_cast<int>(done));
                          indexLabel->setVisible(true);
                          indexBar->setVisible(true);
                      });
    QObject::connect(searchModel, &SearchModel::indexReady, window,
                      [indexLabel, indexBar]() {
                          indexLabel->setMinimumWidth(0);
                          indexLabel->setVisible(false);
                          indexBar->setVisible(false);
                      });
    QObject::connect(searchModel, &SearchModel::indexFailed, window,
                      [indexLabel, indexBar](const QString &message) {
                          indexLabel->setMinimumWidth(0);
                          indexBar->setVisible(false);
                          indexLabel->setStyleSheet(QStringLiteral("color: %1;")
                                                       .arg(severityColor(FfiSeverity::Error).name()));
                          indexLabel->setText(QObject::tr("Index failed: %1").arg(message));
                          indexLabel->setVisible(true);
                      });

    // F0-16: the same treatment for a language server that is still
    // working. `initialize` returning does not mean rust-analyzer can answer
    // yet — it accepts requests while it indexes and answers every one of
    // them with nothing — so the same label-plus-bar pair says which server
    // is busy, in the server's own words, with its percentage when it
    // reports one. Separate widgets from the index pair above because both
    // can be running at once.
    auto *serverLabel = new QLabel(statusBar);
    auto *serverBar = new QProgressBar(statusBar);
    serverLabel->setVisible(false);
    serverBar->setVisible(false);
    serverBar->setTextVisible(false);
    serverBar->setFixedWidth(90);
    QObject::connect(languageService, &LanguageService::serverBusyChanged, window,
                      [serverLabel, serverBar](bool busy, const QString &name,
                                               const QString &activity, bool hasPercent,
                                               quint32 percent) {
                          serverLabel->setVisible(busy);
                          serverBar->setVisible(busy);
                          if (!busy) {
                              return;
                          }
                          // Sized here rather than at build time so a
                          // changed UI font scale is picked up without this
                          // bar joining `UiFontTargets` — it is only ever
                          // visible for a few seconds at a time.
                          serverBar->setFixedHeight(serverBar->fontMetrics().height());
                          serverLabel->setText(QObject::tr("%1: %2...").arg(name, activity));
                          // No percentage is not 0%: an indeterminate bar
                          // says "working, length unknown" where an empty
                          // one would claim no progress has been made.
                          serverBar->setRange(0, hasPercent ? 100 : 0);
                          if (hasPercent) {
                              serverBar->setValue(static_cast<int>(percent));
                          }
                      });

    // ADR-0037: clears whatever `showProjectOpening` set, regardless of
    // which call site triggered the open or how it ended.
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, statusBar,
                      [statusBar]() { clearProjectOpening(statusBar); });
    QObject::connect(treeModel, &ProjectTreeModel::projectOpenFailed, statusBar,
                      [statusBar](const FfiResult &) { clearProjectOpening(statusBar); });

    statusBar->addPermanentWidget(indexLabel);
    statusBar->addPermanentWidget(indexBar);
    statusBar->addPermanentWidget(serverLabel);
    statusBar->addPermanentWidget(serverBar);
    statusBar->addPermanentWidget(problemsButton);
    statusBar->addPermanentWidget(branchButton);
    statusBar->addPermanentWidget(languageLabel);
    statusBar->addPermanentWidget(positionLabel);
    statusBar->addPermanentWidget(encodingLabel);
    editorTabs->attachStatusBar(positionLabel, languageLabel);

    return uiFontTargets;
}

} // namespace ui_shell
