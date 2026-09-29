#pragma once

#include <QPoint>
#include <QRect>
#include <QPointer>
#include <QTimer>
#include <QWidget>

class QMenu;
class QTextBrowser;
class QToolButton;

namespace ui_shell {

// R3: the one popup hover, hover's diagnostic text, the index-declaration
// fallback and F2-11's signature tip all show through now, replacing three
// separate `QToolTip::showText` call sites (`editor_tabs.cpp`,
// `main_window.cpp`, `signature_tip.cpp`). A `QTextBrowser` body rather
// than `QToolTip`'s plain text: R2's Markdown rendering needs real HTML
// (lists, links, tables), and a documentation link has to be clickable,
// which a `QToolTip` cannot offer.
//
// One instance for the whole window, exactly the single-`QToolTip`
// precedent it replaces (`editor_tabs.cpp`'s "one tooltip for the whole
// window" L3 comment) — accessed through the free functions below so every
// call site stays as small as its `QToolTip::showText`/`hideText` call was.
class EditorPopup : public QWidget
{
    Q_OBJECT

public:
    static EditorPopup &instance();

    // Shows `html`, anchored below-right of `globalPos` (clamped to the
    // screen), replacing whatever this popup showed before. A blank `html`
    // hides it instead — nothing to show is not an empty scrollable box.
    void showAt(const QPoint &globalPos, const QString &html);

    // Same, anchored to `anchor` (global coordinates — a word's or the
    // caret's cursor rect): the card opens just below it, or just above when
    // there is no room below.
    // `hoverCard` marks the content as the hover card (whose fixes shortcuts
    // may apply); any other content, or hiding, ends that state and emits
    // `hoverCardEnded`.
    void showAtRect(const QRect &anchor, const QString &html, bool hoverCard = false);

    // The card's content changed while it is up (a problem's fixes arrived):
    // repaint and re-fit it at the same anchor, keeping it pinned/open. A
    // no-op when hidden.
    void updateHtml(const QString &html);

    // The live keymap binding shown beside the ⋮ menu's "Go to Declaration".
    void setDeclarationShortcut(const QString &shortcut);

    // The ⋮ menu's "Show on Mouse Hover" check state (the docs-on-hover
    // setting); set by the window whenever the setting changes.
    void setDocsOnHover(bool enabled);

    // Whether the shown card has a declaration to go to (told by the bridge;
    // reset by each new card).
    void setDeclarationEnabled(bool enabled);

    // Close now, even when pinned — the card acted on something (a fix was applied).
    void dismiss();

    // The soft hide every dismissal but Escape/click-outside goes through
    // (the pointer leaving a hovered word, a signature tip whose call the
    // caret left): a no-op while `pin()` has been called, so a user who hit
    // Ctrl+Q to read something does not lose it to the next mouse move.
    void hidePopup();

    // Ctrl+Q (`code.quickDocumentation`): keep this popup open through
    // whatever would otherwise close it. Only Escape or a click outside
    // still does.
    void pin();

    // The pointer left the hovered span (or the popup): close after a short
    // grace so it can travel into the card. A no-op while the pointer is
    // already inside it, or a popup menu of its own is open.
    void scheduleClose(const QPoint &pointerGlobalPos);

    bool containsGlobal(const QPoint &globalPos) const;

    // E2E only (`e2e_mark.h`): the popup's global rect, the click point of
    // every anchor in its document, the ⋮ button and the word it is anchored to, as the tail of a JSON
    // object (`"rect":[..],"links":[..],"menu":[..]`) — a headless flow cannot
    // find a link inside a `QTextBrowser` any other way.
    QString e2eStateJson() const;

signals:
    // An `ide:` anchor was clicked (`ide:fix/0`, `ide:more/0`, `ide:source`);
    // the payload is the path after the scheme. Deciding what it means is
    // `EditorTabs`'s job, not the popup's.
    void actionRequested(const QString &action);

    // The popup stopped showing a hover card (closed, or shows other content).
    void hoverCardEnded();

private:
    explicit EditorPopup(QWidget *parent = nullptr);

    void forceHide();
    void endHoverCard();
    void setContent(const QString &html);
    void fitToContent();
    void placeAt(const QRect &anchor);

    void closeAfterGrace();
    void applyDocumentStyleSheet();
    QString cardStyleSheet(bool wrapSignature) const;
    void addSeverityIcons();

    void keyPressEvent(QKeyEvent *event) override;
    bool eventFilter(QObject *watched, QEvent *event) override;
    bool event(QEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;
    void enterEvent(QEnterEvent *event) override;
    void leaveEvent(QEvent *event) override;

    QTextBrowser *browser_;
    QToolButton *menuButton_;
    QMenu *menu_;
    QAction *declarationAction_ = nullptr;
    QAction *docsOnHoverAction_ = nullptr;
    QTimer closeTimer_;
    // Where keyboard focus lived before a click activated the popup, so
    // closing it hands the caret back to the editor.
    QPointer<QWidget> returnFocus_;
    bool pinned_ = false;
    QRect anchor_;
    QString html_;
    // Card size minus viewport size; see `fitToContent`. Defaults are the QSS
    // border, layout margins and the browser's insets as measured once.
    int chromeWidth_ = 32;
    int chromeHeight_ = 20;
    bool hoverCard_ = false;
};

void showEditorPopup(const QPoint &globalPos, const QString &html);
void showEditorPopupPinnable(const QRect &anchor, const QString &html, bool pin);
void hideEditorPopup();
void dismissEditorPopup();
void pinEditorPopup();
// `EditorPopup::scheduleClose`, for callers that only hold the free functions.
void scheduleHideEditorPopup(const QPoint &pointerGlobalPos);

// R3 (Ctrl+Q): `showEditorPopup` followed by `pinEditorPopup` when `pin` is
// set — the one line both `editor_tabs.cpp`'s `hoverReady` handler and
// `main_window.cpp`'s `hoverSignatureReady` handler need, since both answer
// either a mouse dwell (`pin` false) or a quick-documentation request
// (`pin` true, `EditorTabs::takeQuickDocPending`).
void showEditorPopupPinnable(const QPoint &globalPos, const QString &html, bool pin);

} // namespace ui_shell
