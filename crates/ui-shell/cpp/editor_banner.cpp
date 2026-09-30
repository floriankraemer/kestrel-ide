#include "editor_banner.h"
#include "e2e_mark.h"
#include "theme.h"

#include <QBoxLayout>
#include <QGridLayout>
#include <QLabel>
#include <QPushButton>
#include <QResizeEvent>
#include <QShowEvent>
#include <QTimer>

namespace ui_shell {

namespace {

// The mark's own vocabulary for `FfiBannerKind` — never the enum's numeric
// value, which would make the mark meaningless without also shipping the
// bridge header to whoever reads the marker stream.
const char *bannerKindName(FfiBannerKind kind)
{
    switch (kind) {
    case FfiBannerKind::TrustGradle:
        return "trust_gradle";
    case FfiBannerKind::TrustMaven:
        return "trust_maven";
    case FfiBannerKind::ReloadNeeded:
        return "reload_needed";
    case FfiBannerKind::None:
        break;
    }
    return "none";
}

} // namespace

EditorBanner::EditorBanner(BuildToolsService *buildToolsService, QWidget *parent)
  : QWidget(parent)
  , buildToolsService_(buildToolsService)
{
    // A plain widget spliced into the dock manager's tree rather than one
    // of its own dock/area classes, so `dockStyleSheet()`'s cascade is not
    // reliable for it (confirmed against a screenshot, not assumed) — set
    // directly from the active theme's palette instead. Does not follow a
    // live theme switch; acceptable for a bar shown only while a prompt is
    // pending, and easy to revisit if that gap ever matters.
    const ChromePalette palette = chromePaletteForTheme(activeThemeName());
    setStyleSheet(QStringLiteral("QWidget#editorBanner { background-color: %1; "
                                 "border-bottom: 1px solid %2; }")
                    .arg(palette.surface2.name(), palette.border.name()));
    setObjectName(QStringLiteral("editorBanner"));

    label_ = new QLabel(this);
    primaryButton_ = new QPushButton(this);
    secondaryButton_ = new QPushButton(this);

    label_->setWordWrap(true);
    label_->setMinimumWidth(0);
    label_->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);

    buttonBar_ = new QWidget(this);
    buttons_ = new QBoxLayout(QBoxLayout::LeftToRight, buttonBar_);
    buttons_->setContentsMargins(0, 0, 0, 0);
    buttons_->setSpacing(8);
    buttons_->addWidget(primaryButton_);
    buttons_->addWidget(secondaryButton_);

    grid_ = new QGridLayout(this);
    grid_->setContentsMargins(12, 8, 12, 8);
    grid_->setSpacing(8);
    grid_->setColumnStretch(0, 1);
    // No layout-imposed minimum: `minimumSizeHint()` says how small we go.
    grid_->setSizeConstraint(QLayout::SetNoConstraint);
    reflow();

    connect(buildToolsService_, &BuildToolsService::bannerChanged, this, &EditorBanner::refresh);
    refresh();
}

void EditorBanner::refresh()
{
    const FfiBannerKind kind = buildToolsService_->bannerKind();
    if (kind == FfiBannerKind::None) {
        setVisible(false);
        return;
    }

    // Disconnect whatever the previous banner's buttons were wired to —
    // `disconnect()` with no arguments clears every connection made to
    // `this`, which is exactly the two `connect` calls below.
    primaryButton_->disconnect();
    secondaryButton_->disconnect();

    switch (kind) {
    case FfiBannerKind::TrustGradle:
        label_->setText(tr("Gradle project detected. Load it?"));
        primaryButton_->setText(tr("Load"));
        secondaryButton_->setText(tr("Not Now"));
        connect(primaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->trustAndLoad(); });
        connect(secondaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->dismissBanner(); });
        break;
    case FfiBannerKind::TrustMaven:
        label_->setText(tr("Maven project detected. Load it?"));
        primaryButton_->setText(tr("Load"));
        secondaryButton_->setText(tr("Not Now"));
        connect(primaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->trustAndLoad(); });
        connect(secondaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->dismissBanner(); });
        break;
    case FfiBannerKind::ReloadNeeded:
        label_->setText(tr("Build files changed. Reload?"));
        primaryButton_->setText(tr("Reload"));
        secondaryButton_->setText(tr("Dismiss"));
        connect(primaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->sync(); });
        connect(secondaryButton_, &QPushButton::clicked, buildToolsService_,
                [this]() { buildToolsService_->dismissBanner(); });
        break;
    case FfiBannerKind::None:
        break;
    }
    reflow();
    setVisible(true);
    markGeometry();
}

QSize EditorBanner::minimumSizeHint() const
{
    return QSize(0, QWidget::minimumSizeHint().height());
}

// One row while the text and both buttons fit side by side; otherwise the
// (wrapping) text takes the full width and the buttons sit right-aligned
// beneath it.
void EditorBanner::reflow()
{
    const QMargins margins = grid_->contentsMargins();
    const int buttonsWide = primaryButton_->sizeHint().width() + buttons_->spacing()
                            + secondaryButton_->sizeHint().width();
    const int gutters = margins.left() + margins.right();
    const int wide = gutters + grid_->spacing()
                     + label_->fontMetrics().horizontalAdvance(label_->text()) + buttonsWide;
    // Narrower than the two buttons side by side: stack those too.
    buttons_->setDirection(width() < gutters + buttonsWide ? QBoxLayout::TopToBottom
                                                           : QBoxLayout::LeftToRight);
    const bool stack = width() < wide;
    if (stack == stacked_ && grid_->count() > 0) {
        return;
    }
    stacked_ = stack;
    grid_->removeWidget(label_);
    grid_->removeWidget(buttonBar_);
    if (stack) {
        grid_->addWidget(label_, 0, 0, 1, 2);
        grid_->addWidget(buttonBar_, 1, 0, 1, 2, Qt::AlignRight);
    } else {
        grid_->addWidget(label_, 0, 0);
        grid_->addWidget(buttonBar_, 0, 1);
    }
}

void EditorBanner::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    markGeometry();
}

void EditorBanner::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    reflow();
    markGeometry();
}

// Deferred a turn, the same "not laid out yet" reason
// `BuildToolsPanel::markE2eRows` defers its own rects: neither
// `setVisible` nor a show/resize event guarantees the buttons have their
// final geometry the instant the call returns. Only a mapped banner is
// reported — an unmapped one's `mapToGlobal` is meaningless, and a flow
// reading the *last* mark would otherwise click where nothing is.
void EditorBanner::markGeometry()
{
    QTimer::singleShot(0, this, [this]() {
        if (!isVisible() || window() == nullptr || !window()->isVisible()) {
            return;
        }
        const FfiBannerKind kind = buildToolsService_->bannerKind();
        if (kind == FfiBannerKind::None) {
            return;
        }
        const QPoint primaryOrigin = primaryButton_->mapToGlobal(QPoint(0, 0));
        const QSize primarySize = primaryButton_->size();
        const QPoint secondaryOrigin = secondaryButton_->mapToGlobal(QPoint(0, 0));
        const QSize secondarySize = secondaryButton_->size();
        e2eMark(QStringLiteral("{\"ev\":\"build_tools_banner\",\"kind\":%1,"
                                "\"primary_rect\":[%2,%3,%4,%5],"
                                "\"secondary_rect\":[%6,%7,%8,%9]}")
                  .arg(e2eJson(QString::fromUtf8(bannerKindName(kind))))
                  .arg(primaryOrigin.x())
                  .arg(primaryOrigin.y())
                  .arg(primarySize.width())
                  .arg(primarySize.height())
                  .arg(secondaryOrigin.x())
                  .arg(secondaryOrigin.y())
                  .arg(secondarySize.width())
                  .arg(secondarySize.height()));
    });
}

} // namespace ui_shell
