#pragma once

#include <QPoint>
#include <QWidget>

class QTextBrowser;

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
public:
    static EditorPopup &instance();

    // Shows `html`, anchored below-right of `globalPos` (clamped to the
    // screen), replacing whatever this popup showed before. A blank `html`
    // hides it instead — nothing to show is not an empty scrollable box.
    void showAt(const QPoint &globalPos, const QString &html);

    // The soft hide every dismissal but Escape/click-outside goes through
    // (the pointer leaving a hovered word, a signature tip whose call the
    // caret left): a no-op while `pin()` has been called, so a user who hit
    // Ctrl+Q to read something does not lose it to the next mouse move.
    void hidePopup();

    // Ctrl+Q (`code.quickDocumentation`): keep this popup open through
    // whatever would otherwise close it. Only Escape or a click outside
    // still does.
    void pin();

private:
    explicit EditorPopup(QWidget *parent = nullptr);

    void forceHide();

    void keyPressEvent(QKeyEvent *event) override;
    bool eventFilter(QObject *watched, QEvent *event) override;

    QTextBrowser *browser_;
    bool pinned_ = false;
};

void showEditorPopup(const QPoint &globalPos, const QString &html);
void hideEditorPopup();
void pinEditorPopup();

// R3 (Ctrl+Q): `showEditorPopup` followed by `pinEditorPopup` when `pin` is
// set — the one line both `editor_tabs.cpp`'s `hoverReady` handler and
// `main_window.cpp`'s `hoverSignatureReady` handler need, since both answer
// either a mouse dwell (`pin` false) or a quick-documentation request
// (`pin` true, `EditorTabs::takeQuickDocPending`).
void showEditorPopupPinnable(const QPoint &globalPos, const QString &html, bool pin);

} // namespace ui_shell
