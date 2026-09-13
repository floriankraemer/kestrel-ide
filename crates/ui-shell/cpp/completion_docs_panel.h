#pragma once

#include <QWidget>

class QTextBrowser;

namespace ui_shell {

// R2: the completion popup's documentation, as a scrollable Markdown-
// rendered panel beside it — replacing the row tooltip's plain, non-
// scrollable text. A frameless, always-on-top widget rather than a second
// `QCompleter`-owned pane: `QCompleter`'s popup offers no slot for a
// sibling widget, so this positions itself off the popup's own geometry
// instead (`showBeside`), the same "float next to the anchor" shape
// `QToolTip` already uses for the signature tip.
//
// Humble view: the HTML it shows is already-rendered
// (`markdown_preview::render`, done in `LanguageServiceRust`) — this class
// decides nothing about Markdown, only where to sit and when to be visible.
class CompletionDocsPanel : public QWidget
{
public:
    explicit CompletionDocsPanel(QWidget *parent = nullptr);

    // Shows `html` in the panel, placed to the right of `popupGeometry`
    // (screen coordinates) — or to its left when there is not enough room
    // on the right. A blank `html` hides the panel instead: nothing to
    // show is not an empty scrollable box.
    void showBeside(const QString &html, const QRect &popupGeometry);
    void hidePanel();

private:
    QTextBrowser *browser_;
};

} // namespace ui_shell
