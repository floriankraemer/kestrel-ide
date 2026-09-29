#include "markdown_preview_panel.h"

#include "e2e_mark.h"

#include <QAction>
#include <QEvent>
#include <QImage>
#include <QLabel>
#include <QMenu>
#include <QScrollBar>
#include <QStackedLayout>
#include <QTextBlock>
#include <QTextBrowser>
#include <QTextCursor>
#include <QUrl>
#include <QVBoxLayout>

#include "editor_tabs.h"
#include "keymap_page.h"

namespace ui_shell {

namespace {

// Anchor names are exactly `L{line}` (`markdown_preview::html::rewrite`'s
// own spelling) — parsed back into a line number here rather than carried
// over the FFI seam a second time, since the string already crossed it once
// inside the HTML.
int lineFromAnchorName(const QString &name)
{
    if (!name.startsWith(QLatin1Char('L'))) {
        return -1;
    }
    bool ok = false;
    const int line = name.mid(1).toInt(&ok);
    return ok ? line : -1;
}

// Every heading anchor in `document`, as `(blockNumber, sourceLine)`, in
// document order. Walked fresh per call rather than cached: a preview
// document is small, and caching would be one more thing to invalidate on
// every re-render.
QVector<QPair<int, int>> anchorBlocks(QTextDocument *document)
{
    QVector<QPair<int, int>> anchors;
    for (QTextBlock block = document->begin(); block.isValid(); block = block.next()) {
        for (auto it = block.begin(); !it.atEnd(); ++it) {
            const QTextFragment fragment = it.fragment();
            if (!fragment.isValid() || !fragment.charFormat().isAnchor()) {
                continue;
            }
            for (const QString &name : fragment.charFormat().anchorNames()) {
                const int line = lineFromAnchorName(name);
                if (line >= 0) {
                    anchors.append({block.blockNumber(), line});
                }
            }
        }
    }
    return anchors;
}

} // namespace

MarkdownPreviewPanel::MarkdownPreviewPanel(PreviewProvider *provider, QWidget *parent)
  : QWidget(parent)
  , provider_(provider)
{
    auto *stack = new QStackedLayout(this);

    browser_ = new QTextBrowser(this);
    // SECURITY (ADR-0021): a previewed Markdown file is untrusted content,
    // same requirement `AiChatPanel` meets for assistant output. Both
    // switches are needed — see that class's own comment — and every click
    // instead reaches `onAnchorClicked`, which asks `previewLinkTarget`
    // rather than deciding anything itself.
    browser_->setOpenLinks(false);
    browser_->setOpenExternalLinks(false);
    browser_->setReadOnly(true);
    // So `setFocus()` on the panel lands in the browser rather than
    // nowhere: the in-tab view mode (editor_tabs_preview.cpp) takes focus
    // away from the editor by focusing this panel, and focus is the only
    // thing keeping keystrokes out of the buffer there. Inert for the dock
    // instance, which nothing focuses programmatically.
    setFocusProxy(browser_);
    stack->addWidget(browser_);

    emptyState_ = new QLabel(tr("Nothing to preview."), this);
    emptyState_->setAlignment(Qt::AlignCenter);
    emptyState_->setWordWrap(true);
    stack->addWidget(emptyState_);

    connect(browser_, &QTextBrowser::anchorClicked, this, &MarkdownPreviewPanel::onAnchorClicked);
    connect(static_cast<PreviewProvider *>(provider_),
            &PreviewProvider::previewReady,
            this,
            &MarkdownPreviewPanel::onPreviewReady);
}

void MarkdownPreviewPanel::setOpenFileHandler(OpenFileHandler handler)
{
    openFileHandler_ = std::move(handler);
}

void MarkdownPreviewPanel::setStatusHandler(StatusHandler handler)
{
    statusHandler_ = std::move(handler);
}

void MarkdownPreviewPanel::setCurrentTab(quint64 tabId, const QString &path,
                                         const QString &content)
{
    currentTabId_ = tabId;
    currentPath_ = path;
    currentContent_ = content;

    if (path.isEmpty() || !provider_->hasPreview(path)) {
        showEmptyState(tr("Nothing to preview."));
        return;
    }
    requestRender();
}

void MarkdownPreviewPanel::changeEvent(QEvent *event)
{
    QWidget::changeEvent(event);
    if (event->type() == QEvent::PaletteChange && !currentPath_.isEmpty()
        && provider_->hasPreview(currentPath_)) {
        requestRender();
    }
}

void MarkdownPreviewPanel::requestRender()
{
    if (currentPath_.isEmpty()) {
        return;
    }
    const int width = qMax(browser_->viewport()->width(), 1);
    provider_->requestPreview(currentTabId_, currentPath_, currentContent_,
                              static_cast<quint32>(width));
}

void MarkdownPreviewPanel::showEmptyState(const QString &message)
{
    emptyState_->setText(message);
    static_cast<QStackedLayout *>(layout())->setCurrentWidget(emptyState_);
}

void MarkdownPreviewPanel::onPreviewReady(quint64 tabId, quint64 revision)
{
    if (tabId != currentTabId_) {
        return;
    }
    // A re-render replaces the whole document, which resets the
    // scrollbar to the top — preserved here by re-finding the nearest
    // anchor to where the view was before `setHtml` runs, and moving back
    // to the same one afterwards. `-1` (no anchor yet, or the document was
    // empty) means there is nothing to restore.
    const int scrollLine = nearestSourceLine();

    const QString html = provider_->previewHtml(tabId);
    auto *document = browser_->document();
    for (const auto &image : provider_->previewImages(tabId)) {
        QImage decoded(reinterpret_cast<const uchar *>(image.pixels.constData()),
                        static_cast<int>(image.width), static_cast<int>(image.height),
                        QImage::Format_RGBA8888_Premultiplied);
        // `.copy()`: `image.pixels` (the QByteArray) dies at the end of
        // this loop iteration, and QImage over borrowed bytes does not
        // own them — the icon cache hits the identical trap, see
        // `icon_cache.cpp`.
        document->addResource(QTextDocument::ImageResource, QUrl(QStringLiteral("ide-preview:%1").arg(image.key)),
                              QVariant(decoded.copy()));
    }
    browser_->setHtml(html);
    static_cast<QStackedLayout *>(layout())->setCurrentWidget(browser_);
    if (scrollLine >= 0) {
        syncToEditorLine(scrollLine);
    }

    e2eMark(QStringLiteral("{\"ev\":\"preview_ready\",\"tab_id\":%1,\"revision\":%2,\"html\":%3}")
              .arg(tabId)
              .arg(revision)
              .arg(e2eJson(html)));
}

void MarkdownPreviewPanel::onAnchorClicked(const QUrl &url)
{
    const auto target = provider_->previewLinkTarget(currentPath_, url.toString());
    switch (target.kind) {
    case FfiPreviewLinkKind::Anchor:
        browser_->scrollToAnchor(target.message);
        break;
    case FfiPreviewLinkKind::OpenFile:
        if (openFileHandler_) {
            openFileHandler_(target.path, target.line);
        }
        break;
    case FfiPreviewLinkKind::Refused:
        if (statusHandler_) {
            statusHandler_(target.message);
        }
        break;
    }
}

void MarkdownPreviewPanel::syncToEditorLine(int firstVisibleLine)
{
    const auto anchors = anchorBlocks(browser_->document());
    QString nearest;
    for (const auto &entry : anchors) {
        if (entry.second <= firstVisibleLine) {
            nearest = QStringLiteral("L%1").arg(entry.second);
        } else {
            break;
        }
    }
    if (nearest.isEmpty()) {
        return;
    }
    syncingScroll_ = true;
    browser_->scrollToAnchor(nearest);
    syncingScroll_ = false;
}

int MarkdownPreviewPanel::nearestSourceLine() const
{
    if (syncingScroll_) {
        return -1;
    }
    const QTextCursor topCursor = browser_->cursorForPosition(QPoint(0, 0));
    const int topBlock = topCursor.blockNumber();
    const auto anchors = anchorBlocks(browser_->document());
    int nearest = -1;
    for (const auto &entry : anchors) {
        if (entry.first <= topBlock) {
            nearest = entry.second;
        } else {
            break;
        }
    }
    return nearest;
}

void wirePreviewModeAction(QMenu *viewMenu,
                           EditorTabs *editorTabs,
                           AppSettings *appSettings,
                           QHash<QString, QAction *> &actions)
{
    QAction *action = registerAction(viewMenu, QStringLiteral("view.togglePreviewMode"),
                                     QObject::tr("Preview Mode"), appSettings, actions);
    QObject::connect(action, &QAction::triggered, viewMenu,
                     [editorTabs]() { editorTabs->togglePreviewMode(); });
}

} // namespace ui_shell
