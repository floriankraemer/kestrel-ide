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
    ProblemsPanel(LanguageService *languageService, BuildService *buildService,
                  AnalysisService *analysisService, DiagnosticsService *diagnosticsService,
                  OpenAt openAt, QWidget *parent);

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

    LanguageService *languageService_;
    BuildService *buildService_;
    DiagnosticsService *diagnosticsService_;
    OpenAt openAt_;
    std::function<void()> firstDiagnostic_;
    bool announced_ = false;
    QString currentFile_;
    QString serverStatus_;

    QLineEdit *filterEdit_ = nullptr;
    QPushButton *errorsButton_ = nullptr;
    QPushButton *warningsButton_ = nullptr;
    QPushButton *infosButton_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    QLabel *statusLabel_ = nullptr;
};

} // namespace ui_shell
