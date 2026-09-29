#include "ai_menu.h"

#include "ai_chat_panel.h"
#include "dock_layout.h"
#include "editor_tabs.h"
#include "keymap_page.h"
#include "refactor_controller.h"

#include <QAction>
#include <QDialog>
#include <QList>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QStatusBar>

namespace ui_shell {

void wireAiChatToEditor(QMainWindow *window, AiChat *aiChat, AiChatPanel *aiChatPanel,
                         EditorTabs *editorTabs, SearchModel *searchModel)
{
    // The AI panel has no route to the editor, so the window hands it the
    // two things it asks for: the buffer the user is looking at, and what
    // Apply means.
    aiChatPanel->setCurrentTextProvider([editorTabs]() { return editorTabs->currentContent(); });
    aiChatPanel->setApplyHandler(
      [window, aiChat, aiChatPanel, editorTabs, searchModel](quint64 messageIndex,
                                                              quint64 blockIndex) {
          // The same protocol — and the same discipline — a refactoring
          // runs (ADR-0021 §5): the revision is read *before* the plan is
          // made and handed back to `takePendingEdits`, so an answer
          // applied to a buffer that has since moved is refused by
          // `lsp_core::EditGate` instead of being spliced in blind.
          const int revision = editorTabs->documentRevision();
          const FfiRefactorSummary summary =
            aiChat->prepareApply(messageIndex, blockIndex, aiChatPanel->currentText(), revision);
          if (summary.title.isEmpty()) {
              // Nothing was planned. Why is `ai-chat-core`'s sentence, and
              // the user pressed a button, so it is said out loud rather
              // than dropped in the status bar.
              QMessageBox::information(window, QObject::tr("AI Chat"),
                                        aiChat->applyRefusal().message);
              return;
          }

          // A change confined to the file the user is looking at applies
          // straight away and is undone with Ctrl+Z; anything wider is
          // shown first — and which of the two this is was decided in Rust,
          // exactly as for a refactoring.
          if (summary.touches_other_files) {
              QList<RefactorPreviewDialog::Row> rows;
              for (const FfiTextEdit &edit : aiChat->pendingEdits()) {
                  rows.append({edit.path, static_cast<int>(edit.start_line),
                                previewText(edit.new_text), true, true});
              }
              RefactorPreviewDialog dialog(
                summary.title,
                QObject::tr("%n change(s) across %1 file(s). Changes to files that are not open "
                            "are written to disk and cannot be undone.",
                            "", static_cast<int>(summary.edit_count))
                  .arg(summary.document_count),
                rows, window);
              if (dialog.exec() != QDialog::Accepted) {
                  aiChat->cancelApply();
                  return;
              }
              for (const QString &path : dialog.excludedPaths()) {
                  aiChat->excludeFromApply(path);
              }
          }

          const ::rust::Vec<FfiTextEdit> edits = aiChat->takePendingEdits(revision);
          if (edits.empty()) {
              window->statusBar()->showMessage(
                QObject::tr("The file changed while the change was being prepared; nothing was "
                            "applied."),
                6000);
              return;
          }
          editorTabs->applyBufferEdits(edits);
          // Files nobody has open are rewritten and re-indexed by the index
          // worker; it ignores the buffer edits in the same vector.
          searchModel->applyFileEdits(edits);
      });

    // Agent-mode tools take the same route MCP's edit_buffer does: the run
    // thread has already marshalled these onto the Qt thread
    // (CxxQtThread::queue), and each lands on the handler DocumentManager's
    // own signal would have reached. Without them the Rust Document moves
    // under an agent's edit while the widget keeps showing stale text.
    QObject::connect(aiChat, &AiChat::toolOpenedTab, editorTabs,
                      [editorTabs](quint64 tabId, const QString &title) {
                          editorTabs->onTabOpened(tabId, title);
                      });
    QObject::connect(aiChat, &AiChat::toolEditedBuffer, editorTabs,
                      [editorTabs](quint64 tabId, const QString &content) {
                          editorTabs->onBufferEditedExternally(tabId, content);
                      });
    QObject::connect(aiChat, &AiChat::liveTextRequested, editorTabs,
                      &EditorTabs::onLiveTextRequested, Qt::DirectConnection);
    QObject::connect(aiChat, &AiChat::toolSavedBuffer, editorTabs, [editorTabs](quint64 tabId) {
        editorTabs->onTabModifiedChanged(tabId, false);
    });
}

void buildAiMenu(QMainWindow *window, AiChat *aiChat, EditorTabs *editorTabs,
                  AppSettings *appSettings, QHash<QString, QAction *> &actions,
                  DockRegistry *docks, AiChatPanel *aiChatPanel)
{
    // Every path that shows the AI chat goes through here — see
    // DockRegistry::show (dock_layout.h) for why "re-add if homeless" runs
    // on every call rather than only at startup.
    const auto showAiChat = [docks]() { docks->show(QStringLiteral("aiChat")); };

    // ADR-0021: the AI menu. Every entry is a registered action, so its
    // shortcut comes from the persisted keymap and Settings > Keymap can
    // rebind it like any other.
    QMenu *aiMenu = window->menuBar()->addMenu(QObject::tr("&AI"));

    // Both selection entries share this: the only difference between them
    // is whether the conversation is cleared first, and duplicating the
    // 0-based-to-1-based conversion is how one of the two copies ends up
    // off by one.
    const auto attachSelection = [window, aiChat, aiChatPanel, editorTabs, showAiChat](bool newChat) {
        if (newChat) {
            aiChat->newConversation();
        }
        // The protocol positions the editor reports are 0-based; an
        // attachment names the lines the way the user reads them off the
        // gutter.
        const auto range = editorTabs->selectionRange();
        const FfiResult result = aiChat->attachSelection(editorTabs->currentPath(),
                                                          range.first.first + 1,
                                                          range.second.first + 1,
                                                          editorTabs->selectedText());
        if (result.code != 0) {
            // An attachment can be refused — a secret-shaped file, a path
            // outside the project — and the reason is Rust's sentence, not
            // one composed here.
            QMessageBox::information(window, QObject::tr("AI Chat"), result.message);
            return;
        }
        showAiChat();
        aiChatPanel->attachAndFocus();
    };

    QAction *aiAddSelectionAction =
      registerAction(aiMenu, QStringLiteral("ai.addSelection"),
                      QObject::tr("Add Selection to AI Chat"), appSettings, actions);
    QObject::connect(aiAddSelectionAction, &QAction::triggered, window,
                      [attachSelection]() { attachSelection(false); });

    QAction *aiAddSelectionNewChatAction =
      registerAction(aiMenu, QStringLiteral("ai.addSelectionNewChat"),
                      QObject::tr("Add Selection to New AI Chat"), appSettings, actions);
    QObject::connect(aiAddSelectionNewChatAction, &QAction::triggered, window,
                      [attachSelection]() { attachSelection(true); });

    QAction *aiAddFileAction = registerAction(aiMenu, QStringLiteral("ai.addFile"),
                                               QObject::tr("Add File to AI Chat"), appSettings,
                                               actions);
    QObject::connect(aiAddFileAction, &QAction::triggered, window,
                      [window, aiChat, aiChatPanel, editorTabs, showAiChat]() {
                          const FfiResult result = aiChat->attachFile(editorTabs->currentPath());
                          if (result.code != 0) {
                              QMessageBox::information(window, QObject::tr("AI Chat"),
                                                        result.message);
                              return;
                          }
                          showAiChat();
                          aiChatPanel->attachAndFocus();
                      });

    aiMenu->addSeparator();
    QAction *aiNewChatAction = registerAction(aiMenu, QStringLiteral("ai.newChat"),
                                               QObject::tr("New AI Chat"), appSettings, actions);
    QObject::connect(aiNewChatAction, &QAction::triggered, window,
                      [aiChatPanel, aiChat, showAiChat]() {
        aiChat->newConversation();
        showAiChat();
        aiChatPanel->attachAndFocus();
    });

    QAction *aiTogglePanelAction = registerAction(aiMenu, QStringLiteral("ai.togglePanel"),
                                                   QObject::tr("AI Chat"), appSettings, actions);
    QObject::connect(aiTogglePanelAction, &QAction::triggered, window,
                      [docks, aiChatPanel, showAiChat]() {
        // A real toggle, unlike the View menu's panels: this one has a
        // shortcut of its own, and a shortcut that only ever opens a panel
        // gives the user no way back with the same keys.
        if (docks->isClosed(QStringLiteral("aiChat"))) {
            showAiChat();
            aiChatPanel->focusComposer();
        } else {
            docks->hide(QStringLiteral("aiChat"));
        }
    });
}

} // namespace ui_shell
