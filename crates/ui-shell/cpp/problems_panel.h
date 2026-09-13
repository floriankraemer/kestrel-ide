#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QColor>
#include <QVector>
#include <QString>
#include <QWidget>

#include <functional>

class QLabel;
class QLineEdit;
class QPushButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class EditorTabs;

// Underline/label colour for one severity, in a hue that stays legible on
// both the light and the dark themes (the VS Code diagnostic hues, which are
// chosen for exactly that). Shared with the editor's squiggles so a row and
// its underline can never disagree about what red means.
QColor severityColor(FfiSeverity severity);

// The Problems dock (Task L2): every diagnostic from every source, grouped
// by file — the shape `docs/design/language-platform-ui.md` section 5
// specifies, and deliberately the same QTreeWidget-grouped-by-file structure
// as SearchResultsPanel so a user who has used one knows this one.
//
// Humble view per CLAUDE.md: which rows exist, their order and their severity
// ranking are `diagnostics-core`'s (`DiagnosticStore::rows`, read through
// `DiagnosticsService`, ADR-0046); this builds widgets, applies the two
// view-local filters (severity toggles, substring box) and turns a
// double-click into a caret jump.
class ProblemsPanel : public QWidget
{
public:
    // `openAt(path, line, column)` jumps the editor to a diagnostic.
    using OpenAt = std::function<void(const QString &, int, int)>;

    // `languageService`/`buildService`/`analysisService` are read only for
    // their `diagnosticsChanged` signal (a source's rows changed, so
    // refresh) and `languageService`'s server-state text; the rows
    // themselves come from `diagnosticsService` alone (ADR-0046) — a
    // build's, a language server's and an analyzer's diagnostics for the
    // same file already coexist in the one shared store, so there is no
    // second merge to do here.
    //
    // `showQuickFixesAt` (R4) is `openAt` plus opening the intentions
    // popup right after — a row's context menu reuses the same caret-based
    // request Alt+Return does (`EditorTabs::showIntentionsNow`) rather than
    // a second "quick fix" request path into the language server.
    ProblemsPanel(LanguageService *languageService, BuildService *buildService,
                  AnalysisService *analysisService, DiagnosticsService *diagnosticsService,
                  OpenAt openAt, OpenAt showQuickFixesAt, QWidget *parent);

    // Called once, the first time a diagnostic arrives in a session, so the
    // window can raise the dock. Never called again: a panel that reopens
    // itself on every failed compile is a panel the user learns to fight.
    void setFirstDiagnosticCallback(std::function<void()> callback);

    // Which file the editor is showing, so its group sorts to the top — the
    // diagnostics the user is acting on are almost always in front of them.
    void setCurrentFile(const QString &path);

    // Focused when the dock is shown from the View menu or the status bar.
    void focusTree();

private:
    void refresh();
    void applyFilter();
    void openRow(QTreeWidgetItem *item, int column);
    void copySelection();
    bool severityEnabled(FfiSeverity severity) const;
    void updateStatus(int shown, int total);
    // R4: the row's context menu — right now just "Quick Fixes...", but its
    // own function since a row menu is not the double-click/Enter path
    // `openRow` already owns.
    void showRowContextMenu(const QPoint &pos);

    LanguageService *languageService_;
    BuildService *buildService_;
    DiagnosticsService *diagnosticsService_;
    OpenAt openAt_;
    OpenAt showQuickFixesAt_;
    std::function<void()> firstDiagnostic_;
    bool announced_ = false;
    QString currentFile_;
    QString serverStatus_;

    QLineEdit *filterEdit_ = nullptr;
    QPushButton *errorsButton_ = nullptr;
    QPushButton *warningsButton_ = nullptr;
    QPushButton *infosButton_ = nullptr;
    // R4: "Current file" scope toggle, off by default — the dock's
    // long-standing default is every open file, and a toggle a user never
    // notices is one they never find.
    QPushButton *currentFileOnlyButton_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    QLabel *statusLabel_ = nullptr;
};

// Builds the panel with its `showQuickFixesAt` wired from `editorTabs`
// (`openFileAtLine` then `showIntentionsNow`) — kept out of
// `main_window.cpp`, which is already at its own line ceiling (ADR-0025),
// the same reason `wireDiagnosticsService` keeps its own wiring out of it.
ProblemsPanel *createProblemsPanel(LanguageService *languageService, BuildService *buildService,
                                    AnalysisService *analysisService,
                                    DiagnosticsService *diagnosticsService, EditorTabs *editorTabs,
                                    ProblemsPanel::OpenAt openAt, QWidget *parent);

} // namespace ui_shell
