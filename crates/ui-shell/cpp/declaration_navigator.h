#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QList>
#include <QObject>
#include <QString>

class QMainWindow;

namespace ui_shell {

class EditorTabs;

// Go to Declaration (N2/N8/L4): turns a resolution — from the language
// server or from the index — into either a jump or a chooser.
//
// Two rules this class does *not* contain: which candidate is best
// (`index_core::resolve_declaration` and the server both answer ranked, and
// the first candidate to arrive is the winner), and who answers at all —
// `lsp_core::definition_outcome` decides that, and reaches here as either
// the definitionFound/definitionFinished pair or definitionFallback.
// Presentation is all that is decided here: one candidate jumps straight
// there, several offer the list (resolution may legitimately be ambiguous —
// name-based per ADR-0008, or genuinely several targets per LSP), none
// reports why nothing happened.
class DeclarationNavigator : public QObject
{
public:
    DeclarationNavigator(LanguageService *languageService, SearchModel *searchModel,
                          EditorTabs *editorTabs, QMainWindow *window);

    // Entry point for both the Ctrl+Click gesture and the menu action.
    // `documentPosition` is a UTF-16 document position; the byte offset
    // the index speaks is derived by EditorTabs, which owns the buffer.
    void resolveAt(int documentPosition);

    // Go to Type Declaration: the server's `typeDefinition` answer, with no
    // index fallback (the verdict still arrives as definitionFound/Finished
    // or definitionUnavailable).
    void resolveTypeAt(int documentPosition);

    // ADR-0016's fallback: ADR-0011's name-based index answers whenever the
    // server did not. Never called from a condition evaluated here — it is
    // wired to definitionFallback, which is `lsp_core`'s verdict.
    void askIndex();

private:
    struct Candidate
    {
        QString path;
        quint32 line;
        quint32 column;
        QString kind;
        QString container;
    };

    void finish(FfiResolutionTier tier, const QString &name);

    void jumpTo(const Candidate &candidate);

    // Several same-named declarations: offer them at the caret rather than
    // picking one. A popup menu (not a dialog) keeps the gesture as light
    // as the click that started it.
    void chooseAmong(const QList<Candidate> &candidates, const QString &name);

    void report(const QString &message);


    LanguageService *languageService_;
    SearchModel *searchModel_;
    EditorTabs *editorTabs_;
    QMainWindow *window_;
    QList<Candidate> candidates_;
    // The document position of the gesture being resolved, kept so the
    // index fallback can re-ask about the same spot in its own units.
    int position_ = 0;
};

} // namespace ui_shell
