#include "editor_popup.h"

#include "theme.h"

#include <QApplication>
#include <QClipboard>
#include <QCursor>
#include <QDesktopServices>
#include <QFrame>
#include <QGuiApplication>
#include <QKeyEvent>
#include <QMenu>
#include <QPainterPath>
#include <QRegion>
#include <QMouseEvent>
#include <QPainter>
#include <QPixmap>
#include <QScreen>
#include <QTextBrowser>
#include <QTextDocument>
#include <QToolButton>
#include <QVBoxLayout>

#include <algorithm>

namespace ui_shell {

namespace {
constexpr int kMaxWidth = 560;
constexpr int kMaxHeight = 400;
constexpr int kMinWidth = 240;
// The pointer's trip from the hovered word into the card, and back out.
constexpr int kCloseGraceMs = 300;
constexpr int kMenuColumnWidth = 22;
constexpr int kMenuInset = 4;
constexpr int kOuterMargin = 8;
// Between the anchor word/caret line and the card.
constexpr int kAnchorGap = 4;
constexpr int kSeverityIconSize = 14;
// Must match the `border-radius` of the `ui_shell--EditorPopup` rule in theme.cpp.
constexpr int kCornerRadius = 8;

bool isModifierKey(int key)
{
    return key == Qt::Key_Control || key == Qt::Key_Shift || key == Qt::Key_Alt
      || key == Qt::Key_Meta || key == Qt::Key_AltGr;
}
} // namespace

EditorPopup::EditorPopup(QWidget *parent)
  : QWidget(parent, Qt::Tool | Qt::FramelessWindowHint | Qt::WindowStaysOnTopHint)
  , browser_(new QTextBrowser(this))
  , menuButton_(new QToolButton(this))
  , menu_(new QMenu(this))
{
    // Painted by the `ui_shell--EditorPopup` rule in theme.cpp (surface,
    // border, radius) — a plain QWidget only paints a stylesheet
    // background when asked to.
    setAttribute(Qt::WA_StyledBackground);
    closeTimer_.setSingleShot(true);
    closeTimer_.setInterval(kCloseGraceMs);
    connect(&closeTimer_, &QTimer::timeout, this, &EditorPopup::closeAfterGrace);
    connect(qApp, &QGuiApplication::applicationStateChanged, this,
            [this](Qt::ApplicationState state) {
                if (state != Qt::ApplicationActive) {
                    hidePopup();
                }
            });

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
    // `ide:` anchors are this card's own actions, never a destination.
    connect(browser_, &QTextBrowser::anchorClicked, this, [this](const QUrl &url) {
        if (url.scheme() == QLatin1String("ide")) {
            emit actionRequested(url.path());
            return;
        }
        QDesktopServices::openUrl(url);
    });

    menuButton_->setText(QStringLiteral("\u22EE"));
    menuButton_->setAccessibleName(QObject::tr("Hover card options"));
    menuButton_->setFocusPolicy(Qt::NoFocus);
    menuButton_->setCursor(Qt::ArrowCursor);
    menuButton_->setFixedSize(kMenuColumnWidth, kMenuColumnWidth);
    QAction *pinAction = menu_->addAction(QObject::tr("Pin"));
    connect(pinAction, &QAction::triggered, this, &EditorPopup::pin);
    QAction *copyAction = menu_->addAction(QObject::tr("Copy"));
    connect(copyAction, &QAction::triggered, this,
            [this]() { QGuiApplication::clipboard()->setText(browser_->toPlainText()); });
    connect(menuButton_, &QToolButton::clicked, this, [this]() {
        menu_->popup(menuButton_->mapToGlobal(QPoint(0, menuButton_->height())));
    });
    // The pointer may have wandered off while the menu was open.
    connect(menu_, &QMenu::aboutToHide, this, [this]() {
        if (!containsGlobal(QCursor::pos())) {
            closeTimer_.start();
        }
    });

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(kOuterMargin, kOuterMargin - 2, kOuterMargin, kOuterMargin - 2);
    layout->addWidget(browser_);
    // An overlay, not a layout column, so the section rules can run the
    // full card width; the first section reserves room under it instead.
    menuButton_->raise();
    resize(kMaxWidth, kMaxHeight / 2);
}

EditorPopup &EditorPopup::instance()
{
    static EditorPopup popup;
    return popup;
}

void EditorPopup::showAt(const QPoint &globalPos, const QString &html)
{
    // A point anchor: a nominal one-line rect that starts at the point, which
    // keeps the historic 20px offset below it.
    showAtRect(QRect(globalPos, QSize(1, 16)), html);
}

void EditorPopup::showAtRect(const QRect &anchor, const QString &html)
{
    if (html.isEmpty()) {
        forceHide();
        return;
    }
    // A new popup is never pre-pinned: each `showAt` is a new question
    // (a different word hovered, a different overload's tip), and
    // `pin()` is always a deliberate Ctrl+Q on its own answer.
    pinned_ = false;
    closeTimer_.stop();
    applyDocumentStyleSheet();
    browser_->setHtml(html);
    addSeverityIcons();
    // Shrink to the content (up to the cap) rather than always being as
    // wide as the widest possible card.
    QTextDocument *document = browser_->document();
    const int chrome = 2 * kOuterMargin + 2 * document->documentMargin();
    document->setTextWidth(kMaxWidth - chrome);
    const int cardWidth = std::clamp(static_cast<int>(document->idealWidth()) + chrome, kMinWidth, kMaxWidth);
    document->setTextWidth(cardWidth - chrome);
    const int height = std::min(
      kMaxHeight, static_cast<int>(document->size().height()) + 2 * kOuterMargin);
    // A scrollbar only when the card hit its cap; otherwise a rounding
    // overshoot would show one on a card that fits.
    browser_->setVerticalScrollBarPolicy(height >= kMaxHeight ? Qt::ScrollBarAsNeeded
                                                              : Qt::ScrollBarAlwaysOff);
    resize(cardWidth, std::max(height, 32));

    const QScreen *screen = QGuiApplication::screenAt(anchor.topLeft());
    const QRect available = screen ? screen->availableGeometry() : QRect(0, 0, 1920, 1080);
    int x = std::min(anchor.left(), available.right() - width());
    int y = anchor.bottom() + kAnchorGap;
    if (y + height > available.bottom()) {
        y = anchor.top() - height - kAnchorGap;
    }
    move(std::max(x, available.left()), std::max(y, available.top()));

    if (!isVisible()) {
        // Remembered before `show()`: closing after a click-activation
        // hands focus back here.
        returnFocus_ = QApplication::focusWidget();
        show();
        qApp->installEventFilter(this);
    }
}

// The semantic classes `lsp_core::hover_card::render` emits, mapped onto the
// active theme. Set on every show so a live theme switch is honoured.
void EditorPopup::applyDocumentStyleSheet()
{
    const QPalette palette = qApp->palette();
    const QString dim = palette.color(QPalette::PlaceholderText).name();
    const QString rule =
      cardBorderColor(palette.color(QPalette::Mid), palette.color(QPalette::PlaceholderText))
        .name();
    const QString sheet =
      QStringLiteral(
        ".dim { color: %1; } .source { color: %1; }"
        "a { color: %2; text-decoration: none; }"
        "p { margin-top: 0px; margin-bottom: 0px; }"
        ".problem { margin-top: 2px; margin-bottom: 2px; }"
        "pre { margin-top: 0px; margin-bottom: 0px; }"
        ".signature { font-family: monospace; }"
        "td.sec { padding-top: 0px; padding-bottom: 6px; padding-right: 22px; }"
        "td.sep { padding-top: 6px; padding-bottom: 6px; border-top: 1px solid %3; }")
        .arg(dim, palette.color(QPalette::Link).name(), rule);
    browser_->document()->setDefaultStyleSheet(sheet);
}

// `<img src="ide-sev:<kind>">` in the card: a filled disc in the severity
// colour with a white glyph, painted at the screen's pixel ratio.
void EditorPopup::addSeverityIcons()
{
    const SemanticColors semantic = semanticColors();
    const struct
    {
        const char *name;
        QColor color;
        QChar glyph;
    } kinds[] = {
        {"error", semantic.error, QLatin1Char('!')},
        {"warning", semantic.warning, QLatin1Char('!')},
        {"info", semantic.info, QLatin1Char('i')},
        {"hint", semantic.muted, QLatin1Char('i')},
    };
    const qreal ratio = devicePixelRatioF();
    const int side = qRound(kSeverityIconSize * ratio);
    for (const auto &kind : kinds) {
        QPixmap pixmap(side, side);
        pixmap.fill(Qt::transparent);
        pixmap.setDevicePixelRatio(ratio);
        QPainter painter(&pixmap);
        painter.setRenderHint(QPainter::Antialiasing);
        painter.setPen(Qt::NoPen);
        painter.setBrush(kind.color);
        painter.drawEllipse(QRectF(0, 0, kSeverityIconSize, kSeverityIconSize));
        QFont font = painter.font();
        font.setBold(true);
        font.setPixelSize(kSeverityIconSize - 4);
        painter.setFont(font);
        painter.setPen(Qt::white);
        painter.drawText(QRectF(0, 0, kSeverityIconSize, kSeverityIconSize), Qt::AlignCenter,
                         QString(kind.glyph));
        painter.end();
        browser_->document()->addResource(
          QTextDocument::ImageResource,
          QUrl(QStringLiteral("ide-sev:") + QLatin1String(kind.name)), pixmap);
    }
}

bool EditorPopup::containsGlobal(const QPoint &globalPos) const
{
    return isVisible() && geometry().contains(globalPos);
}

void EditorPopup::scheduleClose(const QPoint &pointerGlobalPos)
{
    if (!isVisible() || containsGlobal(pointerGlobalPos)) {
        return;
    }
    closeTimer_.start();
}

void EditorPopup::closeAfterGrace()
{
    // A menu opened from the card takes the pointer with it.
    if (menu_->isVisible() || QApplication::activePopupWidget() != nullptr) {
        return;
    }
    hidePopup();
}

// Clips the window to the QSS 8px radius so no square corner pixels show.
// A soft drop shadow is deliberately not attempted: it needs a translucent
// window, which is unreliable without a compositor (X11/Xvfb).
void EditorPopup::resizeEvent(QResizeEvent *event)
{
    QPainterPath path;
    path.addRoundedRect(QRectF(rect()), kCornerRadius, kCornerRadius);
    setMask(QRegion(path.toFillPolygon().toPolygon()));
    menuButton_->move(width() - kMenuColumnWidth - kMenuInset, kMenuInset);
    menuButton_->raise();
    QWidget::resizeEvent(event);
}

void EditorPopup::enterEvent(QEnterEvent *event)
{
    closeTimer_.stop();
    QWidget::enterEvent(event);
}

void EditorPopup::leaveEvent(QEvent *event)
{
    if (isVisible()) {
        closeTimer_.start();
    }
    QWidget::leaveEvent(event);
}

bool EditorPopup::event(QEvent *event)
{
    // Only an *activated* popup (a click gave it focus) can be deactivated;
    // a plain hover card never was, so this never fires for it.
    if (event->type() == QEvent::WindowDeactivate && isVisible() && !menu_->isVisible()
        && QApplication::activePopupWidget() == nullptr) {
        hidePopup();
    }
    return QWidget::event(event);
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
    closeTimer_.stop();
    qApp->removeEventFilter(this);
    const bool wasActive = isActiveWindow();
    hide();
    if (wasActive && returnFocus_ != nullptr) {
        returnFocus_->window()->activateWindow();
        returnFocus_->setFocus();
    }
    returnFocus_.clear();
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
        if (geometry().contains(mouseEvent->globalPosition().toPoint())) {
            // A click inside activates the card, so arrows and PgUp/PgDn
            // scroll it; the show itself never activated it.
            if (!isActiveWindow()) {
                activateWindow();
                browser_->setFocus();
            }
        } else if (QApplication::activePopupWidget() == nullptr) {
            forceHide();
        }
    } else if (event->type() == QEvent::KeyPress) {
        const int key = static_cast<QKeyEvent *>(event)->key();
        if (key == Qt::Key_Escape) {
            forceHide();
        } else if (!isActiveWindow() && !isModifierKey(key)) {
            // Typing in the editor: the card no longer describes what is
            // under the caret. (Once activated, keys belong to the card.)
            hidePopup();
        }
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

void scheduleHideEditorPopup(const QPoint &pointerGlobalPos)
{
    EditorPopup::instance().scheduleClose(pointerGlobalPos);
}

void showEditorPopupPinnable(const QRect &anchor, const QString &html, bool pin)
{
    EditorPopup::instance().showAtRect(anchor, html);
    if (pin) {
        pinEditorPopup();
    }
}

void showEditorPopupPinnable(const QPoint &globalPos, const QString &html, bool pin)
{
    showEditorPopup(globalPos, html);
    if (pin) {
        pinEditorPopup();
    }
}

} // namespace ui_shell
