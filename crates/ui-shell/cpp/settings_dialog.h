#pragma once

#include "appearance_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <memory>

class QAction;
class QWidget;

namespace ui_shell {

class EditorTabs;
class TerminalSessionsPanel;

// Everything the Settings dialog talks to, in one place.
//
// This is a parameter object and nothing more (Fowler, "Introduce Parameter
// Object"): showSettingsDialog took these fourteen collaborators positionally,
// which meant every reader of the call site had to count commas to find out
// which QObject was which. Regrouping them changes no lifetime and adds no
// behaviour — the dialog is modal and blocking, so every member outlives the
// call, exactly as the fourteen arguments did.
//
// `actions` is the action registry `cpp/keymap_page.{h,cpp}` fills and
// re-reads: the dialog's OK branch hands it straight back to applyKeymap()
// so a rebinding takes effect without a restart. It is held by handle rather
// than by reference for the reason buildMainWindow already holds it that
// way: the menus fill the map after this wiring is set up, so what the
// dialog needs is the map itself, not a snapshot of it.
struct SettingsContext
{
    AppSettings *appSettings;
    EditorTabs *editorTabs;
    KeymapEditor *keymapEditor;
    std::shared_ptr<QHash<QString, QAction *>> actions;
    DocumentManager *docManager;
    std::shared_ptr<QString> mcpStatus;
    SyntaxColorEditor *syntaxColorEditor;
    LanguageCatalog *languageCatalog;
    LanguageServerEditor *languageServerEditor;
    EditingEditor *editingEditor;
    LanguageService *languageService;
    AiProviderEditor *aiProviderEditor;
    AiChat *aiChat;
    PluginCatalog *pluginCatalog;
    // The PHP tooling plan's B7/B9: which analyzers are enabled and their
    // trigger (`AnalysisEditor`), and their live detection status
    // (`AnalysisService`), for the Analysis page.
    AnalysisEditor *analysisEditor;
    AnalysisService *analysisService;
    UiFontTargets uiFontTargets;
    // F4-14b: every open terminal tab's Copy/Paste shortcut is re-read from
    // `appSettings` here on OK, rather than through `actions` — see that
    // field's doc comment above for why a per-tab QAction can't live in a
    // shared-by-id map.
    TerminalSessionsPanel *terminalPanel;
};

// Settings dialog (S1: category list + stacked detail pane). One page per
// category, each built by its own `buildXPage` in its own translation unit;
// what is left here is the category list, the stack, and what OK and Cancel
// mean across the pages taken together.
//
// The pages divide into two kinds and the order of the branches at the
// bottom of the implementation is the visible difference: the ones that
// apply live (Appearance, Editor, Syntax Colors) own a revert that Cancel
// runs, and the ones that hold a draft (Keymap, Language Servers, AI
// Providers, MCP) commit on OK and discard by never committing.
//
// Modal and blocking, so every lambda in the implementation capturing the
// dialog only ever runs while the dialog is still alive on that stack frame.
void showSettingsDialog(QWidget *parent, const SettingsContext &context);

} // namespace ui_shell
