#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > Language Servers (task L6): one row per server (a language that
// runs several has several), the command and arguments behind it, and a
// Status column that is live while the page is open.
//
// Humble view (ADR-0002): which rows exist, in which order, what is worth
// persisting and whether a row differs from what is saved are all
// `LanguageServerEditor` calls into `settings-model`/`lsp-core`. The live
// half of the Status column arrives on `LanguageService::serverStateChanged`
// and is only ever rendered here, never decided here — nothing in this file
// starts, stops or retries anything except the explicit `Restart Server`.
//
// A failing command never opens a dialog: `LspManager` retries on a backoff,
// and a modal per retry would make the editor unusable.
// Starts recording every server state the manager announces, from app start,
// so a Language Servers page opened later shows the Status of servers that
// were launched (or ruled out) before it existed.
void trackLanguageServerStates(LanguageService *languageService);

QWidget *buildLanguageServersPage(QWidget *parent,
                                  LanguageServerEditor *editor,
                                  LanguageService *languageService);

} // namespace ui_shell
