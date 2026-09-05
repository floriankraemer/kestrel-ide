#include "settings_dialog.h"

#include "ai_providers_page.h"
#include "appearance_page.h"
#include "e2e_mark.h"
#include "editor_page.h"
#include "editing_page.h"
#include "editor_tabs.h"
#include "keymap_page.h"
#include "language_servers_page.h"
#include "languages_page.h"
#include "mcp_page.h"
#include "plugins_page.h"
#include "icon_decoration_proxy.h"
#include "syntax_colors_page.h"
#include "terminal_page.h"
#include "terminal_sessions_panel.h"

#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFont>
#include <QHBoxLayout>
#include <QLabel>
#include <QListWidget>
#include <QObject>
#include <QPushButton>
#include <QSpinBox>
#include <QStackedWidget>
#include <QStandardItemModel>
#include <QString>
#include <QTimer>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

void showSettingsDialog(QWidget *parent, const SettingsContext &context)
{
    AppSettings *appSettings = context.appSettings;
    EditorTabs *editorTabs = context.editorTabs;

    const FfiEditorFont originalFont = appSettings->editorFont();

    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Settings"));
    // The pages' own minimums add up to roughly 740x510, which is enough to
    // lay a page out but not enough to read one: the Languages tree needs
    // room for four columns before Matches has anything to elide. Sized here
    // rather than in the pages because the dialog is what the user sees, and
    // one number beats four minimums fighting over the same window.
    dialog.resize(960, 640);

    auto *categoryList = new QListWidget(&dialog);
    categoryList->addItem(QObject::tr("Appearance"));
    categoryList->addItem(QObject::tr("Editor"));
    // Editing before Syntax Colors, matching the order the widgets are
    // added to the stack below. They disagreed until F0-10: the list said
    // Syntax Colors fourth and the stack put Editing there, so picking one
    // showed the other.
    categoryList->addItem(QObject::tr("Editing"));
    categoryList->addItem(QObject::tr("Syntax Colors"));
    categoryList->addItem(QObject::tr("Keymap"));
    categoryList->addItem(QObject::tr("Languages"));
    categoryList->addItem(QObject::tr("Language Servers"));
    categoryList->addItem(QObject::tr("AI Providers"));
    categoryList->addItem(QObject::tr("Plugins"));
    categoryList->addItem(QObject::tr("Terminal"));
    categoryList->addItem(QObject::tr("MCP"));
    // Derived from the widest category, floored at the blend spec's ~200px
    // nav width: the interface font scale below can make "Language Servers"
    // wider than 200px at large scales, and a clipped category list is the
    // first thing a user of the scale setting would see, so the floor only
    // ever grows the list, never shrinks it below what the spec asks for.
    categoryList->setMaximumWidth(qMax(
      200, categoryList->fontMetrics().horizontalAdvance(QObject::tr("Language Servers")) + 40));

    auto *pages = new QStackedWidget(&dialog);

    // A page whose settings a project may override carries a badge saying
    // which layer the values it shows came from. The badge is per *area*,
    // not per field, because that is the granularity a project overrides at
    // (`settings_model::scope::ScopedField`) — a per-widget badge would be
    // claiming a precision the file format does not have.
    //
    // Wrapped here rather than inside each `buildXPage` so the pages stay
    // ignorant of scope: they edit whatever draft they were handed, and the
    // dialog is what knows there are two layers.
    auto scopedPage = [&dialog, appSettings](const QString &fieldId, QWidget *page) {
        auto *wrapper = new QWidget(&dialog);
        auto *layout = new QVBoxLayout(wrapper);
        layout->setContentsMargins(0, 0, 0, 0);
        auto *badge = new QLabel(
          QObject::tr("Showing: %1").arg(appSettings->fieldOrigin(fieldId)), wrapper);
        badge->setEnabled(false);
        layout->addWidget(badge);
        layout->addWidget(page, 1);
        return wrapper;
    };

    // Every cached icon behind the tree, dropped: called by the Appearance
    // page when either theme changes, and by the Plugins page when a plugin
    // that contributes icons is switched off.
    auto refreshIcons = [targets = context.uiFontTargets, editorTabs]() {
        refreshTreeIcons(targets.projectTree);
        editorTabs->refreshTabIcons();
    };

    const AppearancePage appearance = buildAppearancePage(
      &dialog, appSettings, context.uiFontTargets,
      AppearanceHooks{
        [editorTabs]() { editorTabs->refreshHighlighting(); },
        refreshIcons,
        [categoryList]() {
            // The dialog is scaling under its own feet: its category list
            // was sized for the font in force when it opened.
            categoryList->setMaximumWidth(qMax(
              200,
              categoryList->fontMetrics().horizontalAdvance(QObject::tr("Language Servers")) + 40));
        },
      });
    pages->addWidget(appearance.widget);

    const EditorPage editor = buildEditorPage(&dialog, appSettings, editorTabs);
    pages->addWidget(editor.widget);

    // Editing commits on OK, like Keymap and Language Servers: the tab
    // width a user is halfway through typing is not a setting worth
    // applying keystroke by keystroke.
    context.editingEditor->beginEdit(appSettings->settingsScope());
    const int editingIndex = pages->addWidget(
      scopedPage(QStringLiteral("editing"), buildEditingPage(&dialog, context.editingEditor)));

    // Syntax Colors follows Appearance rather than Keymap: it applies live,
    // so the user sees the colour in the open editor while picking it, and
    // the Cancel branch below reverts it the same way the theme is reverted.
    context.syntaxColorEditor->beginEdit();
    pages->addWidget(buildSyntaxColorsPage(
      &dialog, context.syntaxColorEditor,
      QFont(originalFont.family, static_cast<int>(originalFont.size)),
      [editorTabs]() { editorTabs->refreshHighlighting(); }));

    // Unlike Appearance/Editor, the keymap isn't applied live: the page edits
    // a draft held in Rust, so Cancel discards it by never committing, and
    // the next beginEdit() re-reads from disk.
    context.keymapEditor->beginEdit();
    pages->addWidget(buildKeymapPage(&dialog, context.keymapEditor));

    // Languages needs no draft: nothing on it is a setting. Adding a
    // language, clearing a quarantine and reloading all take effect when
    // pressed, which is why the page offers no OK-shaped promise.
    pages->addWidget(buildLanguagesPage(
      &dialog, context.languageCatalog,
      [&dialog, editorTabs](const QString &path) {
          editorTabs->openFileAtLine(path, 1, 1);
          dialog.accept();
      },
      [editorTabs]() { editorTabs->reloadHighlighterLanguages(); }));

    // Language Servers commits on OK, like Keymap and MCP: starting and
    // stopping a server on every keystroke in a command field is not a
    // preview.
    context.languageServerEditor->beginEdit(appSettings->settingsScope());
    const int languageServersIndex = pages->addWidget(scopedPage(
      QStringLiteral("languageServers"),
      buildLanguageServersPage(&dialog, context.languageServerEditor, context.languageService)));

    // AI Providers sits next to Language Servers — both configure an
    // external process the IDE talks to — and commits on OK for the same
    // reason: a half-typed base URL is not a setting worth applying. There
    // is no API key field on the page, by ADR-0021 decision 3.
    context.aiProviderEditor->beginEdit();
    pages->addWidget(buildAiProvidersPage(&dialog, context.aiProviderEditor));

    // Plugins needs no draft, for the reason Languages needs none: nothing
    // on it is a setting the dialog holds. Switching a plugin off rebuilds
    // the registry there and then, which is why the page makes no
    // OK-shaped promise.
    pages->addWidget(buildPluginsPage(&dialog, context.pluginCatalog, refreshIcons));

    // Terminal is project-scoped, so it is rebuilt when the scope changes
    // like Editing and Language Servers. Held by handle rather than by
    // value because that rebuild replaces the page — and with it the
    // `commit` closure bound to its widgets — while the OK branch below
    // has already been written to call one.
    auto terminalPage =
      std::make_shared<TerminalPage>(buildTerminalPage(&dialog, appSettings));
    const int terminalIndex =
      pages->addWidget(scopedPage(QStringLiteral("terminal"), terminalPage->widget));

    const McpPage mcp =
      buildMcpPage(&dialog, appSettings, context.docManager, *context.mcpStatus);
    pages->addWidget(mcp.widget);

    QObject::connect(categoryList, &QListWidget::currentRowChanged, pages,
                      &QStackedWidget::setCurrentIndex);
    categoryList->setCurrentRow(0);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    // OK runs the AI page's commit first, because it is the one page that
    // can refuse: `settings-model` validates the draft and says what is
    // wrong with it, and a false answer means the dialog stays open on the
    // field the user has to fix. Nothing else is committed until it passes.
    QObject::connect(
      buttons, &QDialogButtonBox::accepted, &dialog,
      [&dialog, aiProviderEditor = context.aiProviderEditor,
       editingEditor = context.editingEditor]() {
          if (commitAiProvidersPage(&dialog, aiProviderEditor)
              && commitEditingPage(&dialog, editingEditor)) {
              dialog.accept();
          }
      });
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    auto *bodyLayout = new QHBoxLayout();
    bodyLayout->addWidget(categoryList);
    bodyLayout->addWidget(pages, 1);

    // The scope selector: which layer the project-scoped pages edit
    // (ADR-0022). It sits above every page rather than on each one, because
    // it is one choice about the whole dialog, and a per-page selector would
    // let two pages disagree about which file is being written.
    auto *scopeRow = new QHBoxLayout();
    auto *scopeLabel = new QLabel(QObject::tr("Editing settings for:"), &dialog);
    auto *scopeBox = new QComboBox(&dialog);
    scopeBox->addItem(QObject::tr("All projects (global)"), QStringLiteral("global"));
    scopeBox->addItem(QObject::tr("This project"), QStringLiteral("project"));
    auto *scopeHint = new QLabel(&dialog);
    scopeHint->setEnabled(false);

    const bool projectOpen = appSettings->isProjectOpen();
    if (!projectOpen) {
        // Not hidden: the choice exists, there is just nowhere to put the
        // answer yet, and saying so is more useful than a control that
        // silently is not there.
        auto *model = qobject_cast<QStandardItemModel *>(scopeBox->model());
        if (model != nullptr && model->item(1) != nullptr) {
            model->item(1)->setEnabled(false);
        }
        scopeHint->setText(QObject::tr("Open a project to give it settings of its own."));
    } else if (!appSettings->hasProjectSettings()) {
        scopeHint->setText(QObject::tr("This project overrides nothing yet."));
    }
    scopeBox->setCurrentIndex(appSettings->settingsScope() == QStringLiteral("project") ? 1 : 0);
    scopeRow->addWidget(scopeLabel);
    scopeRow->addWidget(scopeBox);
    scopeRow->addWidget(scopeHint, 1);

    // Switching scope reloads the two pages that have a per-project layer.
    // Rebuilding them is what keeps the widgets and the draft in step: the
    // pages read their rows from the Rust draft when they are built, so a
    // page left standing after `beginEdit` would show one layer's values and
    // save them into the other.
    QObject::connect(
      scopeBox, &QComboBox::currentIndexChanged, &dialog,
      [&dialog, appSettings, pages, editingIndex, languageServersIndex, scopeHint,
       scopeBox, scopedPage, editingEditor = context.editingEditor,
       languageServerEditor = context.languageServerEditor,
       languageService = context.languageService, terminalPage, terminalIndex]() {
          const QString scope = scopeBox->currentData().toString();
          appSettings->setSettingsScope(scope);
          scopeHint->setText(appSettings->hasProjectSettings()
                               ? QString()
                               : QObject::tr("This project overrides nothing yet."));

          const int current = pages->currentIndex();

          editingEditor->beginEdit(scope);
          QWidget *staleEditing = pages->widget(editingIndex);
          pages->insertWidget(
            editingIndex,
            scopedPage(QStringLiteral("editing"), buildEditingPage(&dialog, editingEditor)));
          pages->removeWidget(staleEditing);
          staleEditing->deleteLater();

          languageServerEditor->beginEdit(scope);
          QWidget *staleServers = pages->widget(languageServersIndex);
          pages->insertWidget(
            languageServersIndex,
            scopedPage(QStringLiteral("languageServers"),
                       buildLanguageServersPage(&dialog, languageServerEditor, languageService)));
          pages->removeWidget(staleServers);
          staleServers->deleteLater();

          QWidget *staleTerminal = pages->widget(terminalIndex);
          *terminalPage = buildTerminalPage(&dialog, appSettings);
          pages->insertWidget(terminalIndex,
                              scopedPage(QStringLiteral("terminal"), terminalPage->widget));
          pages->removeWidget(staleTerminal);
          staleTerminal->deleteLater();

          pages->setCurrentIndex(current);

          // #143: the scope switch just replaced the Editing page's widget
          // tree, so the `tab_width_rect` `dialog_shown` gave the flow is
          // stale — the spinner it named was just `deleteLater()`d. A
          // fresh rect for whichever one now exists, once laid out.
          //
          // `size()` here is not the spinner's final, settled width — it
          // reads as the full form column's width until a later layout
          // pass narrows it to the spinner's own size hint (confirmed
          // against a screenshot, not guessed; a second nested
          // `singleShot(0, ...)` did not change it either, so this is not
          // a one-more-event-loop-turn problem). `topLeft()` is stable
          // from the first turn, though, and it is all a click needs: the
          // flow focuses the spinner with a click near its left edge, then
          // edits the value with the keyboard (select-all, type, Tab)
          // rather than clicking the up/down arrows at its right edge,
          // which is what would have needed the width.
          QTimer::singleShot(0, &dialog, [pages, editingIndex]() {
              auto *tabWidthSpin = pages->widget(editingIndex)->findChild<QSpinBox *>(
                QStringLiteral("editingTabWidth"));
              if (!tabWidthSpin) {
                  return;
              }
              const QPoint topLeft = tabWidthSpin->mapToGlobal(QPoint(0, 0));
              e2eMark(QStringLiteral("{\"ev\":\"settings_scope_switched\","
                                      "\"tab_width_top_left\":[%1,%2]}")
                        .arg(topLeft.x())
                        .arg(topLeft.y()));
          });
      });

    auto *mainLayout = new QVBoxLayout(&dialog);
    mainLayout->addLayout(scopeRow);
    mainLayout->addLayout(bodyLayout);
    mainLayout->addWidget(buttons);

    // #143: `dialog_shown`/`dialog_closed`, the same convention
    // `run_config_dialog.cpp` marks itself with — a modal `exec()` blocks
    // here, so `dialog_shown` has to fire once the dialog is actually up,
    // from a zero-delay timer inside `exec()`'s own modal loop (tried
    // ahead of `exec()`, reverted: the dialog showed but never took X
    // input focus). The rects an E2E flow needs to drive the per-project
    // settings scenario without guessing a tab order: the scope combo, the
    // "Editing" category row, the Editing page's own tab-width spinner
    // (found by object name — `editing_page.cpp`'s own `RowFields` shape
    // is otherwise anonymous), and OK. The tab-width spinner is inside a
    // `QStackedWidget` page that need not be current yet: `QStackedWidget`
    // lays out every page to the same geometry as soon as the dialog
    // itself has been shown, current or not, so its rect is already valid
    // here.
    QTimer::singleShot(0, &dialog, [=]() {
        const auto rectJson = [](const QRect &rect) {
            return QStringLiteral("[%1,%2,%3,%4]")
              .arg(rect.x())
              .arg(rect.y())
              .arg(rect.width())
              .arg(rect.height());
        };
        const QRect scopeRect(scopeBox->mapToGlobal(QPoint(0, 0)), scopeBox->size());
        const QRect editingCategoryRect(
          categoryList->mapToGlobal(categoryList->visualItemRect(categoryList->item(2)).topLeft()),
          categoryList->visualItemRect(categoryList->item(2)).size());
        auto *tabWidthSpin = pages->widget(editingIndex)->findChild<QSpinBox *>(
          QStringLiteral("editingTabWidth"));
        const QRect tabWidthRect = tabWidthSpin
          ? QRect(tabWidthSpin->mapToGlobal(QPoint(0, 0)), tabWidthSpin->size())
          : QRect();
        QPushButton *okButton = buttons->button(QDialogButtonBox::Ok);
        const QRect okRect(okButton->mapToGlobal(QPoint(0, 0)), okButton->size());
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"settings_dialog\","
                                "\"scope_rect\":%1,\"editing_category_rect\":%2,"
                                "\"tab_width_rect\":%3,\"ok_rect\":%4}")
                  .arg(rectJson(scopeRect), rectJson(editingCategoryRect), rectJson(tabWidthRect),
                       rectJson(okRect)));
    });
    QObject::connect(&dialog, &QDialog::finished, &dialog, [](int result) {
        e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"settings_dialog\","
                                "\"accepted\":%1}")
                  .arg(result == QDialog::Accepted ? QLatin1String("true")
                                                    : QLatin1String("false")));
    });

    if (dialog.exec() == QDialog::Accepted) {
        appearance.commit();
        editor.commit();
        context.keymapEditor->commit();
        applyKeymap(*context.actions, appSettings);
        context.terminalPanel->reapplyKeymap();
        terminalPage->commit();
        mcp.commit();
        // The AI draft was already committed by the OK handler above; this
        // is the chat session re-reading the provider, the mode and the
        // persistence setting it had cached.
        context.aiChat->applyAiSettings();
        context.languageServerEditor->commit();
        // Reconciling is the Rust side's decision: it stops what the new
        // settings no longer describe and leaves the rest running, and the
        // re-announcement below starts the replacements.
        context.languageService->applyServerSettings();
        editorTabs->reannounceDocuments();
    } else {
        context.aiProviderEditor->revert();
        context.syntaxColorEditor->revert();
        appearance.revert();
        editorTabs->refreshHighlighting();
        editor.revert();
    }
}

} // namespace ui_shell
