#include "editor_tabs.h"

#include "code_editor.h"
#include "e2e_mark.h"
#include "editor_popup.h"

#include <QMainWindow>
#include <QStatusBar>
#include <QTimer>

namespace ui_shell {

// H2: everything the hover card needs from `EditorTabs` — its translated
// fixed words, the popup's `ide:` anchors, and where an answer is shown.
void EditorTabs::wireHoverCard()
{
    // H2: the hover card's fixed words, translated here (the view owns
    // `tr()`) and stored by the bridge for `render`.
    languageService_->setHoverLabels(QStringList{
      QObject::tr("Looking for fixes…"), QObject::tr("More actions…"), QObject::tr("Source:"),
      QObject::tr("Error"), QObject::tr("Warning"), QObject::tr("Info"), QObject::tr("Hint")});
    connect(&EditorPopup::instance(), &EditorPopup::actionRequested, this,
            &EditorTabs::onHoverCardAction);

    // L3/R3: one popup for the whole window. The answer is asynchronous,
    // so it is shown where the pointer is when it arrives — safe only
    // because `lsp_core::HoverTracker` has already dropped everything
    // the user has moved on from, so whatever reaches here is still
    // about the word under the cursor. `hoverAt`'s html already carries
    // the LSP hover and every diagnostic at that position composed
    // together (`lsp_core::hover_card`), so this is the one place that
    // paints either or both. Ctrl+Q's own request comes back on this same
    // signal (`requestQuickDocumentation`'s doc comment), which is why
    // pinning happens here rather than at the request site.
    connect(languageService_, &LanguageService::hoverReady, this, [this](const QString &html) {
        EditorPopup::instance().showAtRect(hoverAnchor(), html, true);
        if (takeQuickDocPending()) {
            pinEditorPopup();
        }
        // R3 E2E: the only way a headless flow can see the popup's content
        // — it is a separate toplevel with no model behind it, the same
        // reason `e2eMarkMenuActions` exists for a QMenu.
        e2eMark(QStringLiteral("{\"ev\":\"hover_popup_shown\",\"html\":%1}").arg(e2eJson(html)));
    });
    // H3: the card's fixes are usable only while it is what the popup shows.
    connect(&EditorPopup::instance(), &EditorPopup::hoverCardEnded, this,
            [this]() { languageService_->clearHoverFixes(); });
    // H3: a problem's fixes arrived; repaint the card where it stands.
    connect(languageService_, &LanguageService::hoverCardUpdated, this, [](const QString &html) {
        EditorPopup::instance().updateHtml(html);
        e2eMark(QStringLiteral("{\"ev\":\"hover_popup_updated\",\"html\":%1}").arg(e2eJson(html)));
    });
}

// `ide:fix/<i>` applies problem i's primary fix, `ide:more/<i>` opens its
// intentions under the card; `ide:source` is H4's.
void EditorTabs::onHoverCardAction(const QString &action)
{
    bool ok = false;
    if (action.startsWith(QLatin1String("fix/"))) {
        const quint32 problem = QStringView(action).mid(4).toUInt(&ok);
        if (ok) {
            languageService_->applyHoverFix(problem, documentRevision());
            dismissEditorPopup();
        }
    } else if (action.startsWith(QLatin1String("more/"))) {
        const quint32 problem = QStringView(action).mid(5).toUInt(&ok);
        if (ok && languageService_->selectHoverIntentions(problem)) {
            const QPoint below = EditorPopup::instance().geometry().bottomLeft();
            showIntentionsMenu(&below);
            dismissEditorPopup();
        }
    }
}

void EditorTabs::setHoverShortcuts(const QString &applyFix, const QString &moreActions)
{
    applyFixShortcut_ = applyFix;
    languageService_->setHoverShortcuts(applyFix, moreActions);
}

// `code.applyPreferredFix`: the visible hover card's primary fix if it has
// one, else the caret's — which needs the caret's intentions first.
void EditorTabs::applyPreferredFixNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    // The bridge holds fixes only while the popup shows that hover card.
    if (languageService_->applyPreferredHoverFix(documentRevision())) {
        dismissEditorPopup();
        return;
    }
    // The request may never answer (no server for this file); do not let a
    // stale flag apply a fix on some later, unrelated answer.
    applyPreferredPending_ = true;
    QTimer::singleShot(5000, this, [this]() { applyPreferredPending_ = false; });
    requestIntentionsFor(editor, false);
}

void EditorTabs::applyPreferredFromCaretAnswer()
{
    if (languageService_->applyPreferredIntention(documentRevision())) {
        return;
    }
    if (auto *main = qobject_cast<QMainWindow *>(window_)) {
        main->statusBar()->showMessage(QObject::tr("No quick fix is available here."), 4000);
    }
}


} // namespace ui_shell
