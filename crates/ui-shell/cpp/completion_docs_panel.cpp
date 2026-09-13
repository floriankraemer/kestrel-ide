#include "completion_docs_panel.h"

#include <QFrame>
#include <QGuiApplication>
#include <QScreen>
#include <QTextBrowser>
#include <QVBoxLayout>

#include <algorithm>

namespace ui_shell {

namespace {
constexpr int kPanelWidth = 320;
constexpr int kPanelHeight = 220;
} // namespace

CompletionDocsPanel::CompletionDocsPanel(QWidget *parent)
  : QWidget(parent, Qt::ToolTip | Qt::FramelessWindowHint)
  , browser_(new QTextBrowser(this))
{
    // SECURITY (ADR-0021, same requirement `MarkdownPreviewPanel` meets): a
    // completion's documentation comes from a language server reading an
    // opened project's files — untrusted content, so no link ever
    // auto-opens and no external image ever loads.
    browser_->setOpenLinks(false);
    browser_->setOpenExternalLinks(false);
    browser_->setReadOnly(true);
    browser_->setFrameShape(QFrame::NoFrame);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(browser_);
    resize(kPanelWidth, kPanelHeight);
}

void CompletionDocsPanel::showBeside(const QString &html, const QRect &popupGeometry)
{
    if (html.isEmpty()) {
        hidePanel();
        return;
    }
    browser_->setHtml(html);

    const QScreen *screen = QGuiApplication::screenAt(popupGeometry.center());
    const QRect available = screen ? screen->availableGeometry() : QRect(0, 0, 1920, 1080);

    int x = popupGeometry.right() + 1;
    if (x + kPanelWidth > available.right()) {
        x = popupGeometry.left() - kPanelWidth - 1;
    }
    int y = popupGeometry.top();
    if (y + kPanelHeight > available.bottom()) {
        y = available.bottom() - kPanelHeight;
    }
    move(x, std::max(y, available.top()));
    show();
}

void CompletionDocsPanel::hidePanel()
{
    hide();
}

} // namespace ui_shell
