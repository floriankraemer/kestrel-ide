#include "editor_popup.h"

#include "e2e_mark.h"
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
#include <QTextBlock>
#include <QScrollBar>
#include <QTextBrowser>
#include <QTextCursor>
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
    // Everything wraps (signatures are `pre-wrap`), so a horizontal bar could
    // only ever eat the card's last line.
    browser_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    // E2E: the only way a headless flow sees the card scroll from the keyboard.
    connect(browser_->verticalScrollBar(), &QScrollBar::valueChanged, this, [this]() {
        if (isVisible()) {
            e2eMark(QStringLiteral("{\"ev\":\"hover_popup_scrolled\",%1}").arg(e2eStateJson()));
        }
    });
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
    declarationAction_ = menu_->addAction(QObject::tr("Go to Declaration"));
    declarationAction_->setEnabled(false);
    connect(declarationAction_, &QAction::triggered, this,
            [this]() { emit actionRequested(QStringLiteral("source")); });
    menu_->addSeparator();
    docsOnHoverAction_ = menu_->addAction(QObject::tr("Show on Mouse Hover"));
    docsOnHoverAction_->setCheckable(true);
    docsOnHoverAction_->setChecked(true);
    connect(docsOnHoverAction_, &QAction::triggered, this, [this](bool checked) {
        emit actionRequested(checked ? QStringLiteral("docs-on-hover/1")
                                     : QStringLiteral("docs-on-hover/0"));
    });
    QAction *hoverSettingsAction = menu_->addAction(QObject::tr("Hover Settings…"));
    connect(hoverSettingsAction, &QAction::triggered, this,
            [this]() { emit actionRequested(QStringLiteral("settings")); });
    e2eMarkMenuActions(menu_, "hover_menu_action");
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
    // On the heap on purpose: `~QApplication` deletes every top-level widget,
    // so a function-local static would be destroyed a second time at process
    // exit (a SIGSEGV on Ctrl+Q whenever the ⋮ menu had been used).
    static EditorPopup *popup = new EditorPopup;
    return *popup;
}

void EditorPopup::showAt(const QPoint &globalPos, const QString &html)
{
    // A point anchor: a nominal one-line rect that starts at the point, which
    // keeps the historic 20px offset below it.
    showAtRect(QRect(globalPos, QSize(1, 16)), html);
}

void EditorPopup::showAtRect(const QRect &anchor, const QString &html, bool hoverCard)
{
    if (html.isEmpty()) {
        forceHide();
        return;
    }
    if (!hoverCard) {
        endHoverCard();
    }
    hoverCard_ = hoverCard;
    // Each card starts without a declaration; the bridge says when it has one.
    declarationAction_->setEnabled(false);
    // A new popup is never pre-pinned: each `showAt` is a new question
    // (a different word hovered, a different overload's tip), and
    // `pin()` is always a deliberate Ctrl+Q on its own answer.
    pinned_ = false;
    closeTimer_.stop();
    setContent(html);
    placeAt(anchor);

    if (!isVisible()) {
        // Remembered before `show()`: closing after a click-activation
        // hands focus back here.
        returnFocus_ = QApplication::focusWidget();
        show();
        // Now that it is laid out, correct the size for the real chrome.
        fitToContent();
        placeAt(anchor);
        qApp->installEventFilter(this);
    }
}

void EditorPopup::setContent(const QString &html)
{
    applyDocumentStyleSheet();
    browser_->setHtml(html);
    addSeverityIcons();
    html_ = html;
    fitToContent();
}

// Size the card to its content, up to the cap. Measured on scratch documents
// rather than the browser's own: it answers with the layout of its previous
// width until it is shown, and a `pre-wrap` signature reports only its
// shortest wrapped width, which would shrink the card to its minimum. So the
// width comes from a copy with signatures unwrapped (a signature wider than
// the cap then wraps at the cap) and the height from a wrapped one.
void EditorPopup::fitToContent()
{
    // The card's chrome (layout margins, border, the browser's own insets) is
    // whatever is left around the viewport; read it while it is on screen,
    // and keep the last reading for the first card of a session.
    if (isVisible() && !browser_->verticalScrollBar()->isVisible()) {
        chromeWidth_ = width() - browser_->viewport()->width();
        chromeHeight_ = height() - browser_->viewport()->height();
    }
    QTextDocument natural;
    natural.setDefaultStyleSheet(cardStyleSheet(false));
    natural.setHtml(html_);
    natural.setTextWidth(kMaxWidth - chromeWidth_);
    const int cardWidth = std::clamp(static_cast<int>(natural.idealWidth()) + chromeWidth_,
                                     kMinWidth, kMaxWidth);
    QTextDocument wrapped;
    wrapped.setDefaultStyleSheet(cardStyleSheet(true));
    wrapped.setHtml(html_);
    wrapped.setTextWidth(cardWidth - chromeWidth_);
    const int height =
      std::min(kMaxHeight, static_cast<int>(wrapped.size().height()) + chromeHeight_);
    // A scrollbar only when the card hit its cap; otherwise a rounding
    // overshoot would show one on a card that fits.
    browser_->setVerticalScrollBarPolicy(height >= kMaxHeight ? Qt::ScrollBarAsNeeded
                                                              : Qt::ScrollBarAlwaysOff);
    resize(cardWidth, std::max(height, 32));
}

void EditorPopup::placeAt(const QRect &anchor)
{
    anchor_ = anchor;
    const QScreen *screen = QGuiApplication::screenAt(anchor.topLeft());
    const QRect available = screen ? screen->availableGeometry() : QRect(0, 0, 1920, 1080);
    int x = std::min(anchor.left(), available.right() - width());
    int y = anchor.bottom() + kAnchorGap;
    if (y + height() > available.bottom()) {
        y = anchor.top() - height() - kAnchorGap;
    }
    move(std::max(x, available.left()), std::max(y, available.top()));
}

void EditorPopup::updateHtml(const QString &html)
{
    if (!isVisible() || html.isEmpty()) {
        return;
    }
    setContent(html);
    placeAt(anchor_);
}

void EditorPopup::setDeclarationEnabled(bool enabled)
{
    declarationAction_->setEnabled(enabled);
}

void EditorPopup::setDeclarationShortcut(const QString &shortcut)
{
    QString text = QObject::tr("Go to Declaration");
    if (!shortcut.isEmpty()) {
        text += QLatin1Char('\t') + shortcut;
    }
    declarationAction_->setText(text);
}

void EditorPopup::dismiss()
{
    forceHide();
}

// The semantic classes `lsp_core::hover_card::render` emits, mapped onto the
// active theme. Set on every show so a live theme switch is honoured.
void EditorPopup::applyDocumentStyleSheet()
{
    browser_->document()->setDefaultStyleSheet(cardStyleSheet(true));
}

// `wrapSignature` off is the card's natural (unwrapped) look, which is what
// its width is measured from.
QString EditorPopup::cardStyleSheet(bool wrapSignature) const
{
    const QPalette palette = qApp->palette();
    const QString dim = palette.color(QPalette::PlaceholderText).name();
    const QString rule =
      cardBorderColor(palette.color(QPalette::Mid), palette.color(QPalette::PlaceholderText))
        .name();
    // A subtle chip for inline code: the card's own text colour, faint,
    // over its surface — legible in light and dark alike.
    const QColor surface = palette.color(QPalette::Window);
    const QColor text = palette.color(QPalette::WindowText);
    const QColor chip(surface.red() * 88 / 100 + text.red() * 12 / 100,
                      surface.green() * 88 / 100 + text.green() * 12 / 100,
                      surface.blue() * 88 / 100 + text.blue() * 12 / 100);
    const QString sheet =
      QStringLiteral(
        "code { font-family: monospace; background-color: %4; }"
        ".dim { color: %1; } .source { color: %1; }"
        "a { color: %2; text-decoration: none; }"
        "p { margin-top: 0px; margin-bottom: 0px; }"
        ".problem { margin-top: 2px; margin-bottom: 2px; }"
        "pre { margin-top: 0px; margin-bottom: 0px; }"
        ".signature { font-family: monospace; white-space: %5; }"
        ".path { font-family: monospace; }"
        ".fixrow { margin-left: 22px; margin-top: 0px; margin-bottom: 2px; }"
        "td.sec { padding-top: 0px; padding-bottom: 6px; padding-right: 22px; }"
        "td.sep { padding-top: 6px; padding-bottom: 6px; border-top: 1px solid %3; }")
        .arg(dim, palette.color(QPalette::Link).name(), rule, chip.name(),
             wrapSignature ? QStringLiteral("pre-wrap") : QStringLiteral("pre"));
    return sheet;
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

void EditorPopup::setDocsOnHover(bool enabled)
{
    docsOnHoverAction_->setChecked(enabled);
}

void EditorPopup::pin()
{
    if (isVisible()) {
        pinned_ = true;
    }
}

void EditorPopup::endHoverCard()
{
    if (hoverCard_) {
        hoverCard_ = false;
        emit hoverCardEnded();
    }
}

QString EditorPopup::e2eStateJson() const
{
    QStringList links;
    for (QTextBlock block = browser_->document()->begin(); block.isValid();
         block = block.next()) {
        for (auto it = block.begin(); !it.atEnd(); ++it) {
            const QTextCharFormat format = it.fragment().charFormat();
            if (!format.isAnchor()) {
                continue;
            }
            QTextCursor cursor(browser_->document());
            cursor.setPosition(it.fragment().position());
            const QRect line = browser_->cursorRect(cursor);
            // A few pixels in from the fragment's start is inside its text.
            const QPoint at = browser_->viewport()->mapToGlobal(
              QPoint(line.left() + 6, line.center().y()));
            links << QStringLiteral("{\"href\":%1,\"x\":%2,\"y\":%3}")
                       .arg(e2eJson(format.anchorHref()))
                       .arg(at.x())
                       .arg(at.y());
        }
    }
    const QPoint menuAt = menuButton_->mapToGlobal(menuButton_->rect().center());
    return QStringLiteral("\"rect\":[%1,%2,%3,%4],\"links\":[%5],\"menu\":[%6,%7],"
                        "\"anchor\":[%8,%9,%10,%11],\"doc\":[%12,%13,%14,%15],\"scroll\":%16")
      .arg(x()).arg(y()).arg(width()).arg(height())
      .arg(links.join(QLatin1Char(',')))
      .arg(menuAt.x()).arg(menuAt.y())
      .arg(anchor_.x()).arg(anchor_.y()).arg(anchor_.width()).arg(anchor_.height())
      .arg(static_cast<int>(browser_->document()->size().width()))
      .arg(static_cast<int>(browser_->document()->size().height()))
      .arg(browser_->viewport()->width())
      .arg(browser_->viewport()->height())
      .arg(browser_->verticalScrollBar()->value());
}

void EditorPopup::forceHide()
{
    if (isVisible()) {
        e2eMark("{\"ev\":\"hover_popup_hidden\"}");
    }
    endHoverCard();
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

void dismissEditorPopup()
{
    EditorPopup::instance().dismiss();
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
