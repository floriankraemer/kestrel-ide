#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QLabel;
class QPushButton;

namespace ui_shell {

// One reusable info bar above the editor area (the jvm-build-tools plan's
// B4): text plus up to two buttons, shown or hidden entirely from
// `BuildToolsService::bannerKind()` — a trust prompt ("Load"/"Not now") or
// a reload prompt ("Reload"/"Dismiss"). `cpp/` maps the bridge's
// `FfiBannerKind` onto `tr()` text and never decides *when* to show one —
// that is `jvm_build_core::sync::decide`'s rule, already applied on the
// Rust side before `bannerChanged` fires.
class EditorBanner : public QWidget
{
    Q_OBJECT

public:
    explicit EditorBanner(BuildToolsService *buildToolsService, QWidget *parent);

private:
    void refresh();

    BuildToolsService *buildToolsService_;
    QLabel *label_;
    QPushButton *primaryButton_;
    QPushButton *secondaryButton_;
};

} // namespace ui_shell
