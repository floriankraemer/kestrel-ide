#pragma once

#include "refactor_preview_dialog.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCoreApplication>
#include <QHash>
#include <QList>
#include <QObject>
#include <QPair>
#include <QString>

class QAction;
class QMainWindow;
class QMenu;

namespace ui_shell {

class EditorTabs;

// One line of an edit, for the preview. A multi-line insertion is shown by
// its first line: the dialog says what is changing and where, not what the
// new text is in full. Free rather than a member of RefactorController: the
// AI panel's Apply runs the same preview over the same FfiTextEdit rows
// (ADR-0021 §5), and a second copy of this would drift.
QString previewText(const QString &newText);

// RF11: every refactoring gesture, in one place.
//
// It contains no refactoring logic and no rules about when a refactoring is
// safe. What it does is ask, then paint what came back: whether a preview is
// required is `lsp_core::EditPlan::touches_other_files`, whether a rename may
// go ahead at all is `lsp_core::rename`'s and `index_core`'s to say, and
// which sites of a name-based rename start ticked is decided before the
// dialog is built. Every branch below is on a flag or a signal, never on a
// judgement made here.
class RefactorController : public QObject
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in a Qt base
    // class's context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(RefactorController)

public:
    RefactorController(LanguageService *languageService, SearchModel *searchModel,
                        EditorTabs *editorTabs, QMainWindow *window);

    // Shift+F6. Asks the server whether the symbol can be renamed at all;
    // a server that does not implement the question answers "go ahead",
    // which is `lsp_core::rename::prepare_outcome`'s rule.
    void renameSymbol();

    // Ctrl+Alt+M and its siblings: ask for a kind family, then offer
    // whatever the server actually has.
    void extract(const QString &kind, const QString &nothingFound);

    // `code.reformat` and `code.showIntentions` (F2-10), added to
    // `refactorMenu` here rather than inline in `buildMainWindow` — which
    // has no headroom left under the file-size ceiling — since this class
    // already owns everything both actions need.
    void buildCodeActions(QMenu *refactorMenu, AppSettings *appSettings,
                          QHash<QString, QAction *> &actions);

private:
    QPair<quint32, quint32> caret() const;

    void askForNewName(const QString &placeholder);

    // ADR-0016's fallback, reached only when `lsp_core` said no server
    // answered — never from a condition evaluated here.
    void askIndexToRename();

    // Why a name-based rename will not run. Three cases are a sentence; the
    // unsaved-files case is a dead end the user can get out of, so it offers
    // the way out instead of describing it.
    //
    // These used to go to the status bar, where a message the user did not
    // happen to be looking at made a refused rename indistinguishable from a
    // broken one.
    void onRenameRefused(FfiRenameRefusal reason, const QString &message);

    void onRefactorReady(const FfiRefactorSummary &summary);

    void applyPending();

    void onIndexRenameReady(const QString &name, bool ambiguous);

    void onCodeActionsReady();

    // How many distinct files a batch of edits changes in their buffers.
    static int countFiles(const ::rust::Vec<FfiTextEdit> &edits);

    static int countPaths(const QList<RefactorPreviewDialog::Row> &rows);

    static int countBufferFiles(const ::rust::Vec<FfiTextEdit> &edits);

    void report(const QString &message);

    LanguageService *languageService_;
    SearchModel *searchModel_;
    EditorTabs *editorTabs_;
    QMainWindow *window_;
    QString pendingWord_;
    QString pendingName_;
    QString nothingFound_;
    int revision_ = 0;
    // Files changed in their buffers by the refactoring being applied, so
    // the outcome can be reported as a whole rather than as the disk half.
    int bufferFiles_ = 0;
};

} // namespace ui_shell
