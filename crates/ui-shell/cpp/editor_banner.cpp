#include "editor_banner.h"

#include <QHBoxLayout>
#include <QLabel>
#include <QPushButton>

namespace ui_shell {

EditorBanner::EditorBanner(BuildToolsService *buildToolsService, QWidget *parent)
  : QWidget(parent)
  , buildToolsService_(buildToolsService)
{
    label_ = new QLabel(this);
    primaryButton_ = new QPushButton(this);
    secondaryButton_ = new QPushButton(this);

    auto *layout = new QHBoxLayout(this);
    layout->addWidget(label_, 1);
    layout->addWidget(primaryButton_);
    layout->addWidget(secondaryButton_);

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
    setVisible(true);
}

} // namespace ui_shell
