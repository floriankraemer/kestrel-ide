#include "splash_screen.h"

#include <QApplication>
#include <QFont>
#include <QColor>
#include <QPainter>
#include <QPixmap>
#include <QProgressBar>
#include <QRectF>
#include <QSize>
#include <QString>

namespace ui_shell {

namespace {

constexpr int SplashWidth = 420;
constexpr int SplashHeight = 190;
constexpr int Margin = 16;
constexpr int ProgressBarHeight = 6;
// Room under the bar for the stage message QSplashScreen draws itself.
constexpr int MessageHeight = 24;

// Painted in code rather than loaded from a .qrc or an install-relative
// asset: the app ships as a single binary (docker/Dockerfile), and theme.cpp
// resolved the same question the same way for the stylesheets.
QPixmap renderBackground(const ThemeColors &colors, qreal devicePixelRatio)
{
    QPixmap pixmap(QSize(SplashWidth, SplashHeight) * devicePixelRatio);
    pixmap.setDevicePixelRatio(devicePixelRatio);
    pixmap.fill(Qt::transparent);

    QPainter painter(&pixmap);
    painter.setRenderHint(QPainter::Antialiasing);

    const QRectF panel(0.5, 0.5, SplashWidth - 1.0, SplashHeight - 1.0);
    painter.setBrush(colors.background);
    painter.setPen(colors.accent);
    painter.drawRoundedRect(panel, 6.0, 6.0);

    QFont titleFont = painter.font();
    titleFont.setPointSize(26);
    titleFont.setBold(true);
    painter.setFont(titleFont);
    painter.setPen(colors.foreground);
    // Centred in the area above the progress bar and the stage message.
    painter.drawText(panel.adjusted(Margin, Margin, -Margin,
                                    -Margin - MessageHeight - ProgressBarHeight),
                     Qt::AlignCenter, QStringLiteral("Kestrel"));

    return pixmap;
}

// `ratio` of `towards` mixed into `base`.
QColor blend(const QColor &base, const QColor &towards, double ratio)
{
    return QColor::fromRgbF(base.redF() * (1.0 - ratio) + towards.redF() * ratio,
                            base.greenF() * (1.0 - ratio) + towards.greenF() * ratio,
                            base.blueF() * (1.0 - ratio) + towards.blueF() * ratio);
}

} // namespace

SplashScreen::SplashScreen(const QString &themeName)
  : QSplashScreen(renderBackground(colorsForTheme(themeName), qApp->devicePixelRatio()))
  , colors_(colorsForTheme(themeName))
  , progressBar_(new QProgressBar(this))
{
    // The pixmap's rounded corners are transparent; without this the
    // platform would paint the default window background into them.
    setAttribute(Qt::WA_TranslucentBackground);

    progressBar_->setRange(0, StageCount);
    progressBar_->setValue(0);
    progressBar_->setTextVisible(false);
    progressBar_->setGeometry(Margin,
                              SplashHeight - Margin - MessageHeight - ProgressBarHeight,
                              SplashWidth - 2 * Margin,
                              ProgressBarHeight);
    // Styled here rather than by the app stylesheet: the splash is up before
    // qApp->setStyleSheet() has been called with the persisted theme. The
    // track is a blend towards the foreground rather than lighter()/darker(),
    // which leaves white unchanged and made the track invisible in the light
    // theme.
    progressBar_->setStyleSheet(
      QStringLiteral("QProgressBar { background-color: %1; border: none; }"
                     "QProgressBar::chunk { background-color: %2; }")
        .arg(blend(colors_.background, colors_.foreground, 0.25).name(),
             colors_.accent.name()));
}

void SplashScreen::setStage(int step, const QString &text)
{
    progressBar_->setValue(step);
    showMessage(text, Qt::AlignBottom | Qt::AlignHCenter, colors_.foreground);
    // buildMainWindow() blocks this thread from here until the next stage and
    // QApplication::exec() has not been reached yet, so without this the
    // splash would show its first frame and then freeze.
    QApplication::processEvents();
}

} // namespace ui_shell
