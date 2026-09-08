#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

class QWidget;

namespace ui_shell {

// Settings > Language: the UI locale (en default, de/es/fr shipped).
//
// No `revert`, unlike AppearancePage: a language switch does not preview
// live — there is no retranslateUi() plumbing (ADR-0049) — so there is
// nothing on screen for a Cancel path to put back. `commit` just persists
// whatever the combo is set to; the effect wants a restart, which the page
// says so via a disabled notice label rather than pretending it can do more.
struct LanguagePage
{
    QWidget *widget;
    std::function<void()> commit;
};

LanguagePage buildLanguagePage(QWidget *parent, AppSettings *appSettings);

} // namespace ui_shell
