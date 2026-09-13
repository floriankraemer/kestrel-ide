#include "editor_popup.h"

#include <QApplication>
#include <QDesktopServices>
#include <QFrame>
#include <QGuiApplication>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QScreen>
#include <QTextBrowser>
#include <QVBoxLayout>

#include <algorithm>

namespace ui_shell {

namespace {
constexpr int kMaxWidth = 480;
constexpr int kMaxHeight = 320;
} // namespace

EditorPopup::EditorPopup(QWidget *parent)
  : QWidget(parent, Qt::Tool | Qt::FramelessWindowHint | Qt::WindowStaysOnTopHint)
  , browser_(new QTextBrowser(this))
{
    // Never activated on its own (`WA_ShowWithoutActivating`): a hover
    // dwell must not steal keyboard focus from the editor mid-keystroke.
    // Escape and click-outside are still caught, through the application-
    // wide event filter `showAt` installs — not focus.
    setAttribute(Qt::WA_ShowWithoutActivating);
    setMaximumSize(kMaxWidth, kMaxHeight);

    // SECURITY (ADR-0021): a hover's Markdown/diagnostic text comes from an
    // opened project's language server or build tool — untrusted content,
    // so links never auto-open. R3 does want a clicked link to go
    // somewhere, unlike the read-only completion docs panel, so this opens
    // it explicitly through the desktop's own handler rather than loading
    // it into the browser (which would still be this process rendering
    // untrusted remote content).
    browser_->setOpenLinks(false);
    browser_->setReadOnly(true);
    browser_->setFrameShape(QFrame::NoFrame);
    connect(browser_, &QTextBrowser::anchorClicked, this,
            [](const QUrl &url) { QDesktopServices::openUrl(url); });

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(6, 4, 6, 4);
    layout->addWidget(browser_);
    resize(kMaxWidth, kMaxHeight / 2);
}

EditorPopup &EditorPopup::instance()
{
    static EditorPopup popup;
    return popup;
}

void EditorPopup::showAt(const QPoint &globalPos, const QString &html)
{
    if (html.isEmpty()) {
        forceHide();
        return;
    }
    // A new popup is never pre-pinned: each `showAt` is a new question
    // (a different word hovered, a different overload's tip), and
    // `pin()` is always a deliberate Ctrl+Q on its own answer.
    pinned_ = false;
    browser_->setHtml(html);
    browser_->document()->setTextWidth(kMaxWidth - 16);
    const int height = std::min(
      kMaxHeight, static_cast<int>(browser_->document()->size().height()) + 12);
    resize(kMaxWidth, std::max(height, 32));

    const QScreen *screen = QGuiApplication::screenAt(globalPos);
    const QRect available = screen ? screen->availableGeometry() : QRect(0, 0, 1920, 1080);
    int x = std::min(globalPos.x(), available.right() - width());
    int y = globalPos.y() + 20;
    if (y + height > available.bottom()) {
        y = globalPos.y() - height - 4;
    }
    move(std::max(x, available.left()), std::max(y, available.top()));

    if (!isVisible()) {
        show();
        qApp->installEventFilter(this);
    }
}

void EditorPopup::hidePopup()
{
    if (pinned_) {
        return;
    }
    forceHide();
}

void EditorPopup::pin()
{
    if (isVisible()) {
        pinned_ = true;
    }
}

void EditorPopup::forceHide()
{
    pinned_ = false;
    qApp->removeEventFilter(this);
    hide();
}

void EditorPopup::keyPressEvent(QKeyEvent *event)
{
    if (event->key() == Qt::Key_Escape) {
        event->accept();
        forceHide();
        return;
    }
    QWidget::keyPressEvent(event);
}

bool EditorPopup::eventFilter(QObject *watched, QEvent *event)
{
    if (event->type() == QEvent::MouseButtonPress) {
        const auto *mouseEvent = static_cast<QMouseEvent *>(event);
        if (!geometry().contains(mouseEvent->globalPosition().toPoint())) {
            forceHide();
        }
    } else if (event->type() == QEvent::KeyPress
               && static_cast<QKeyEvent *>(event)->key() == Qt::Key_Escape) {
        forceHide();
    }
    return QWidget::eventFilter(watched, event);
}

void showEditorPopup(const QPoint &globalPos, const QString &html)
{
    EditorPopup::instance().showAt(globalPos, html);
}

void hideEditorPopup()
{
    EditorPopup::instance().hidePopup();
}

void pinEditorPopup()
{
    EditorPopup::instance().pin();
}

void showEditorPopupPinnable(const QPoint &globalPos, const QString &html, bool pin)
{
    showEditorPopup(globalPos, html);
    if (pin) {
        pinEditorPopup();
    }
}

} // namespace ui_shell
