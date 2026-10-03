#include "run_toolbar.h"

#include "e2e_mark.h"
#include "theme.h"
#include "ui_tokens.h"

#include <QComboBox>
#include <QHBoxLayout>
#include <QMessageBox>
#include <QPainter>
#include <QPainterPath>
#include <QPixmap>
#include <QToolButton>

#include <functional>

namespace ui_shell {

namespace {

// The 26x26 (toolbar-h - 10, per the blend spec) square glyph buttons in the
// Run/Stop/Rerun cluster: transparent background, `--r-ctl` hover tint,
// no visible label — the glyph and the tooltip carry the meaning instead.
constexpr int kButtonSide = tokens::kToolbarHeight - 10;
constexpr int kIconSide = 15;

QToolButton *makeGlyphButton(const QString &accessibleName, const QString &tooltip,
                             QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setFixedSize(kButtonSide, kButtonSide);
    button->setIconSize(QSize(kIconSide, kIconSide));
    button->setToolTip(tooltip);
    button->setAccessibleName(accessibleName);
    button->setAutoRaise(true);
    button->setCursor(Qt::PointingHandCursor);
    // Transparent ground, `--r-ctl` hover tint: chromeStyleSheet()'s
    // `QToolButton` rule, shared with every other icon button.
    return button;
}

// Renders one glyph into a `kIconSide`x`kIconSide` pixmap, tinted `color`,
// from 16x16-native-coordinate QPainter calls — `paint` draws in that
// 16x16 space and this scales it down, the same "native space, then scale"
// approach the mockup's own SVG `viewBox` uses.
QIcon glyphIcon(QColor color, const std::function<void(QPainter &, QColor)> &paint)
{
    QPixmap pixmap(kIconSide, kIconSide);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    painter.setRenderHint(QPainter::Antialiasing);
    painter.scale(kIconSide / 16.0, kIconSide / 16.0);
    paint(painter, color);
    return QIcon(pixmap);
}

// Run — play triangle, mockup path `M4 2.5v11l10-5.5z`.
QIcon runGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        QPainterPath path;
        path.moveTo(4, 2.5);
        path.lineTo(4, 13.5);
        path.lineTo(14, 8);
        path.closeSubpath();
        painter.fillPath(path, tint);
    });
}

// Stop — rounded square, mockup `<rect x="3.5" y="3.5" width="9" height="9" rx="1"/>`.
QIcon stopGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        QPainterPath path;
        path.addRoundedRect(QRectF(3.5, 3.5, 9, 9), 1, 1);
        painter.fillPath(path, tint);
    });
}

// Rerun — circular refresh arrows. The mockup's path is a single-arc bezier
// too fiddly to hand-transcribe faithfully; a stroked ring plus a filled
// arrowhead reads as the same glyph at this size, which is the bar the
// design brief sets for a hand-approximated icon.
// ponytail: silhouette-only approximation, not the mockup's exact bezier —
// swap for a transcribed QPainterPath if pixel fidelity ever matters here.
QIcon rerunGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        QPen pen(tint, 1.6);
        pen.setCapStyle(Qt::RoundCap);
        painter.setPen(pen);
        painter.setBrush(Qt::NoBrush);
        painter.drawArc(QRectF(2.2, 2.2, 11.6, 11.6), 40 * 16, 260 * 16);

        QPainterPath arrow;
        arrow.moveTo(13.4, 4.6);
        arrow.lineTo(10.0, 4.0);
        arrow.lineTo(11.4, 7.0);
        arrow.closeSubpath();
        painter.setPen(Qt::NoPen);
        painter.setBrush(tint);
        painter.drawPath(arrow);
    });
}

// Build — a hammer reduced to its silhouette: a head bar and a handle. The
// mockup shows the same two shapes; at 16px anything more detailed reads as
// noise.
QIcon buildGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        painter.setPen(Qt::NoPen);
        painter.setBrush(tint);
        QPainterPath head;
        head.addRoundedRect(QRectF(2.5, 3.0, 8.0, 3.2), 1, 1);
        painter.drawPath(head);
        QPainterPath handle;
        handle.addRoundedRect(QRectF(7.6, 6.0, 2.2, 7.0), 1, 1);
        painter.drawPath(handle);
    });
}

// Debug — the mockup's bug silhouette, reduced to a body and two legs. At
// 16px anything more detailed reads as a smudge.
QIcon debugGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        QPen pen(tint, 1.4);
        pen.setCapStyle(Qt::RoundCap);
        painter.setPen(pen);
        painter.drawLine(QPointF(2.5, 6.5), QPointF(5.0, 7.5));
        painter.drawLine(QPointF(13.5, 6.5), QPointF(11.0, 7.5));
        painter.drawLine(QPointF(2.5, 11.5), QPointF(5.0, 10.5));
        painter.drawLine(QPointF(13.5, 11.5), QPointF(11.0, 10.5));
        painter.setPen(Qt::NoPen);
        painter.setBrush(tint);
        painter.drawEllipse(QRectF(5.0, 4.0, 6.0, 9.0));
    });
}

// Listen for PHP debug connections — a telephone handset, IntelliJ's icon
// for the same toggle: an arc with an earpiece and a mouthpiece.
QIcon listenGlyph(QColor color)
{
    return glyphIcon(color, [](QPainter &painter, QColor tint) {
        QPen pen(tint, 2.2);
        pen.setCapStyle(Qt::RoundCap);
        painter.setPen(pen);
        painter.setBrush(Qt::NoBrush);
        painter.drawArc(QRectF(2.5, 4.5, 11, 11), 25 * 16, 130 * 16);
        painter.setPen(Qt::NoPen);
        painter.setBrush(tint);
        painter.drawRoundedRect(QRectF(1.8, 7.2, 3.4, 4.6), 1.2, 1.2);
        painter.drawRoundedRect(QRectF(10.8, 7.2, 3.4, 4.6), 1.2, 1.2);
    });
}

} // namespace

void setPhpListening(DebugService *debugService, QWidget *parent, bool enabled)
{
    const FfiResult result = debugService->setPhpListening(enabled);
    if (result.code != 0) {
        QMessageBox::warning(parent, QObject::tr("PHP Debug"), result.message);
    }
}

RunToolbar::RunToolbar(RunService *runService, BuildService *buildService,
                        DebugService *debugService, QWidget *parent)
  : QWidget(parent)
  , runService_(runService)
  , buildService_(buildService)
  , debugService_(debugService)
{
    // Height comes from the QToolBar this sits in (chromeStyleSheet()'s
    // `QToolBar` rule); the horizontal padding is this layout's, because a
    // QToolBar's QSS padding is applied to all four sides (it would cut
    // 8px off the top and bottom of the 26px controls).
    configCombo_ = new QComboBox(this);
    configCombo_->setMinimumWidth(120);
    configCombo_->setSizeAdjustPolicy(QComboBox::AdjustToContents);

    const SemanticColors semantic = semanticColors();
    const QColor dim = palette().color(QPalette::PlaceholderText);
    runButton_ = makeGlyphButton(tr("Run"), tr("Run"), this);
    runButton_->setIcon(runGlyph(semantic.ok));
    stopButton_ = makeGlyphButton(tr("Stop"), tr("Stop"), this);
    stopButton_->setIcon(stopGlyph(semantic.error));
    rerunButton_ = makeGlyphButton(tr("Rerun"), tr("Rerun"), this);
    rerunButton_->setIcon(rerunGlyph(dim));
    buildButton_ = makeGlyphButton(tr("Build"), tr("Build Project"), this);
    buildButton_->setIcon(buildGlyph(dim));
    debugButton_ = makeGlyphButton(tr("Debug"), tr("Debug"), this);
    debugButton_->setIcon(debugGlyph(dim));
    listenButton_ = makeGlyphButton(tr("Listen"), tr("Start Listening for PHP Debug Connections"),
                                    this);
    listenButton_->setCheckable(true);
    QIcon listenIcon;
    listenIcon.addPixmap(listenGlyph(dim).pixmap(kIconSide, kIconSide), QIcon::Normal, QIcon::Off);
    listenIcon.addPixmap(listenGlyph(semantic.ok).pixmap(kIconSide, kIconSide), QIcon::Normal,
                         QIcon::On);
    listenButton_->setIcon(listenIcon);

    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(tokens::kSp2, 0, tokens::kSp2, 0);
    layout->setSpacing(tokens::kSp1);
    layout->addWidget(runButton_);
    layout->addWidget(stopButton_);
    layout->addWidget(rerunButton_);
    layout->addWidget(buildButton_);
    layout->addWidget(debugButton_);
    layout->addWidget(listenButton_);
    layout->addStretch(1);
    layout->addWidget(configCombo_);

    connect(runService_, &RunService::configurationsChanged, this,
            &RunToolbar::refreshConfigurations);
    connect(configCombo_, &QComboBox::currentIndexChanged, this, &RunToolbar::refreshButtons);
    connect(runButton_, &QToolButton::clicked, this, &RunToolbar::runSelected);
    connect(stopButton_, &QToolButton::clicked, this, &RunToolbar::stopSelected);
    connect(rerunButton_, &QToolButton::clicked, this, &RunToolbar::rerunSelected);
    // The button and `build.build` are two doors onto one call; neither
    // decides anything about the build (ADR-0040).
    connect(buildButton_, &QToolButton::clicked, this,
            [this]() { buildService_->build(); });
    connect(debugButton_, &QToolButton::clicked, this, &RunToolbar::debugSelected);
    connect(listenButton_, &QToolButton::clicked, this,
            [this](bool on) { setPhpListening(debugService_, this, on); });
    // The listener can end by itself (its adapter exiting), and the Run menu
    // toggles it too: the button shows what `DebugService` says.
    connect(debugService_, &DebugService::phpListeningChanged, listenButton_,
            &QToolButton::setChecked);
    listenButton_->setChecked(debugService_->isPhpListening());
    // A PHP debug launch: `DebugService` has the listener up and says which
    // configuration to start with which Xdebug environment.
    connect(debugService_, &DebugService::phpLaunchRequested, this,
            [this](const QString &configId, const QString &envJson) {
                const FfiResult result = runService_->runWithEnv(configId, envJson);
                if (result.code != 0) {
                    QMessageBox::warning(this, tr("Debug"), result.message);
                }
            });

    connect(runService_, &RunService::consoleStarted, this,
            [this](quint64 consoleId, const QString &configId) {
                runningConsoleIdByConfig_.insert(configId, consoleId);
                refreshButtons();
            });
    connect(runService_, &RunService::consoleFinished, this,
            [this](quint64 consoleId, int, bool) {
                // Find-by-value: `consoleFinished` carries the console, not
                // the configuration it was launched from.
                for (auto it = runningConsoleIdByConfig_.begin();
                     it != runningConsoleIdByConfig_.end(); ++it) {
                    if (it.value() == consoleId) {
                        runningConsoleIdByConfig_.erase(it);
                        break;
                    }
                }
                refreshButtons();
            });
    connect(runService_, &RunService::runFailed, this,
            [this](const QString &, FfiResult error) {
                QMessageBox::warning(this, tr("Run"), error.message);
            });

    refreshConfigurations();
}

void RunToolbar::refreshConfigurations()
{
    const QString keepId = selectedConfigId();
    const QSignalBlocker blocker(configCombo_);
    configCombo_->clear();
    int keepIndex = -1;
    for (const FfiRunConfig &config : runService_->configurations()) {
        configCombo_->addItem(config.name, config.id);
        if (config.id == keepId) {
            keepIndex = configCombo_->count() - 1;
        }
    }
    configCombo_->setCurrentIndex(keepIndex >= 0 ? keepIndex : 0);
    refreshButtons();

    // The only signal an E2E flow has that a just-detected or just-persisted
    // configuration has actually reached this combo box — `configurations()`
    // is read fresh from disk on every call, so nothing else here marks a
    // moment worth waiting for.
    e2eMark(QStringLiteral("{\"ev\":\"run_configurations_changed\",\"count\":%1}")
              .arg(configCombo_->count()));
}

void RunToolbar::refreshButtons()
{
    const QString configId = selectedConfigId();
    const bool hasConfig = !configId.isEmpty();
    const bool running = hasConfig && runningConsoleIdByConfig_.contains(configId);
    runButton_->setEnabled(hasConfig);
    stopButton_->setEnabled(running);
    rerunButton_->setEnabled(running);
}

QString RunToolbar::selectedConfigId() const
{
    return configCombo_->currentData().toString();
}

void RunToolbar::runSelected()
{
    const QString configId = selectedConfigId();
    if (!configId.isEmpty()) {
        runService_->run(configId);
    }
}

void RunToolbar::stopSelected()
{
    const QString configId = selectedConfigId();
    const auto it = runningConsoleIdByConfig_.constFind(configId);
    if (it != runningConsoleIdByConfig_.constEnd()) {
        runService_->stop(it.value());
    }
}

void RunToolbar::killSelected()
{
    const QString configId = selectedConfigId();
    const auto it = runningConsoleIdByConfig_.constFind(configId);
    if (it != runningConsoleIdByConfig_.constEnd()) {
        runService_->kill(it.value());
    }
}

void RunToolbar::debugSelected()
{
    const QString configId = selectedConfigId();
    if (!configId.isEmpty()) {
        // Whether an adapter exists for it, and what a launch body looks
        // like, are `DebugService`'s answers (ADR-0041).
        const FfiResult result = debugService_->debug(configId);
        if (result.code != 0) {
            QMessageBox::warning(this, tr("Debug"), result.message);
        }
    }
}

void RunToolbar::rerunSelected()
{
    const QString configId = selectedConfigId();
    const auto it = runningConsoleIdByConfig_.constFind(configId);
    if (it != runningConsoleIdByConfig_.constEnd()) {
        runService_->rerun(it.value());
    } else {
        runSelected();
    }
}

void RunToolbar::focusConfigSelector()
{
    configCombo_->setFocus();
    configCombo_->showPopup();
}

} // namespace ui_shell
