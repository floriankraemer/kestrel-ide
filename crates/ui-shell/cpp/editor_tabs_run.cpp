// R1-7: the editor's half of running from context — the gutter Run icon.
//
// Its own translation unit for the same reason `editor_tabs_vcs.cpp` is one:
// `editor_tabs.cpp` sits near the file-size ceiling (ADR-0025), and this is
// one service's wiring rather than tab machinery.
//
// Humble view throughout: whether a file has a run target is
// `RunService::canRunFile`/`canRunContainerfile`/`canRunComposeFile`, and
// what running it launches is `RunService::runContext`/`runContainerfile`/
// `runComposeFile`. Nothing here decides either — the gutter click's popup
// (C5, ADR-0056) only decides *which already-existing action* the user
// picked, the same shape a context menu always is.

#include "editor_tabs.h"

#include "code_editor.h"
#include "run_config_dialog.h"

#include <QAction>
#include <QCursor>
#include <QDesktopServices>
#include <QMainWindow>
#include <QMenu>
#include <QPlainTextEdit>
#include <QSet>
#include <QStatusBar>
#include <QUrl>

namespace ui_shell {

void wireRunService(RunService *runService, EditorTabs *editorTabs, RunConfigEditor *runConfigEditor,
                    ContainerService *containerService)
{
    editorTabs->setRunService(runService);
    editorTabs->setContainerRunContext(runConfigEditor, containerService); // C5
    // A configuration list that just changed can make a file runnable that
    // was not — the first `cargo run` entry a detection scan adds, say — so
    // the open editor's gutter is re-asked rather than left stale.
    QObject::connect(runService, &RunService::configurationsChanged, editorTabs,
                      [editorTabs]() { editorTabs->refreshRunMarkers(); });
}

void EditorTabs::setRunService(RunService *runService)
{
    runService_ = runService;
}

void EditorTabs::setContainerRunContext(RunConfigEditor *runConfigEditor,
                                        ContainerService *containerService)
{
    runConfigEditor_ = runConfigEditor;
    containerService_ = containerService;
    if (containerService_ == nullptr) {
        return;
    }
    // C6: editor assistance. The two services never reach into each other
    // on the Rust side; this is the one place they are introduced.
    connect(containerService_, &ContainerService::treeChanged, this, [this]() {
        languageService_->setLocalImages(containerService_->localImageNames());
        refreshComposeLenses();
    });
    connect(languageService_, &LanguageService::containerActionRequested, this,
            [this](const QString &, const QString &payload) {
                const FfiResult result = containerService_->pullImage(payload);
                auto *mainWindow = qobject_cast<QMainWindow *>(window_);
                if (result.code != 0 && mainWindow != nullptr) {
                    mainWindow->statusBar()->showMessage(QString(result.message), 6000);
                }
            });
    connect(containerService_, &ContainerService::openUrlRequested, this,
            [](const QString &url) { QDesktopServices::openUrl(QUrl(url)); });
}

void EditorTabs::refreshComposeLensesFor(CodeEditor *editor)
{
    if (containerService_ == nullptr || editor == nullptr) {
        return;
    }
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty() || !containerService_->ownsLenses(path)) {
        return;
    }
    QVector<CodeLensSpan> lenses;
    for (const FfiComposeLens &lens : containerService_->composeLenses(path, editor->toPlainText())) {
        QString label;
        if (lens.is_open_url) {
            label = tr("Open localhost:%1").arg(QString(lens.host_port));
        } else if (lens.running > 0) {
            label = tr("\u25CF running (%1/%2)").arg(lens.running).arg(lens.total);
        } else if (lens.exit_code >= 0) {
            label = tr("\u2717 exited (%1)").arg(lens.exit_code);
        } else {
            label = tr("\u25CB stopped");
        }
        lenses.append(CodeLensSpan{ static_cast<int>(lens.line), label, lens.clickable });
    }
    editor->setCodeLenses(lenses);
}

void EditorTabs::refreshComposeLenses()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        refreshComposeLensesFor(qobject_cast<CodeEditor *>(editor));
    });
}

void EditorTabs::refreshRunMarker(CodeEditor *editor)
{
    if (!runService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    QSet<int> lines;
    if (!path.isEmpty()) {
        for (const quint32 line : runService_->runLines(path, editor->toPlainText())) {
            lines.insert(static_cast<int>(line));
        }
    }
    editor->setRunLines(lines);
}

void EditorTabs::refreshRunMarkers()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        refreshRunMarker(qobject_cast<CodeEditor *>(editor));
    });
}

void EditorTabs::requestRunFor(CodeEditor *editor, int line)
{
    if (!runService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }

    // A Dockerfile/Containerfile or compose file's gutter offers a choice
    // (C5, ADR-0056) instead of running outright — the same distinction
    // JetBrains' own gutter icon makes for these two file types, since
    // "run" is ambiguous for them (build only? build and run? which
    // connection?) in a way it is not for `cargo run`.
    if (runService_->canRunContainerfile(path)) {
        // C9: Podman's naming convention gets its own word in the popup
        // rather than always saying "Dockerfile" — `isNamedContainerfile`
        // is the one rule (`container_core::run_config::
        // is_named_containerfile`), this only picks which translated word.
        const QString word = runService_->isNamedContainerfile(path) ? tr("Containerfile")
                                                                       : tr("Dockerfile");
        QMenu menu(editor);
        QAction *build = menu.addAction(tr("Build Image from %1").arg(word));
        QAction *run = menu.addAction(tr("Run Container"));
        QAction *newConfig = menu.addAction(tr("New Configuration..."));
        QAction *chosen = menu.exec(QCursor::pos());
        if (chosen == build) {
            runService_->buildContainerfile(path);
        } else if (chosen == run) {
            runService_->runContainerfile(path);
        } else if (chosen == newConfig && runConfigEditor_) {
            const QString id = runService_->newContainerfileConfiguration(path);
            if (!id.isEmpty()) {
                showRunConfigDialog(editor, runConfigEditor_, containerService_, id);
            }
        }
        return;
    }
    if (runService_->canRunComposeFile(path)) {
        // C6: a marker on a service line scopes the popup to that service;
        // the `services:` line (and `run.runContext`) means the whole file.
        const QString service =
          runService_->composeServiceAt(path, editor->toPlainText(), static_cast<quint32>(line));
        QMenu menu(editor);
        QAction *run = menu.addAction(service.isEmpty() ? tr("Run Compose Project")
                                                        : tr("Run Service '%1'").arg(service));
        QAction *newConfig = menu.addAction(tr("New Configuration..."));
        QAction *chosen = menu.exec(QCursor::pos());
        if (chosen == run) {
            if (service.isEmpty()) {
                runService_->runComposeFile(path);
            } else {
                runService_->runComposeService(path, service);
            }
        } else if (chosen == newConfig && runConfigEditor_) {
            const QString id = service.isEmpty()
              ? runService_->newComposeFileConfiguration(path)
              : runService_->newComposeServiceConfiguration(path, service);
            if (!id.isEmpty()) {
                showRunConfigDialog(editor, runConfigEditor_, containerService_, id);
            }
        }
        return;
    }

    runService_->runContext(path);
}

} // namespace ui_shell
