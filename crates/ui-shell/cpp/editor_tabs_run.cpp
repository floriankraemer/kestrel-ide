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
#include <QMenu>
#include <QPlainTextEdit>

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
}

void EditorTabs::refreshRunMarker(CodeEditor *editor)
{
    if (!runService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        editor->setRunnable(false);
        return;
    }
    editor->setRunnable(runService_->canRunFile(path) || runService_->canRunContainerfile(path)
                       || runService_->canRunComposeFile(path));
}

void EditorTabs::refreshRunMarkers()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        refreshRunMarker(qobject_cast<CodeEditor *>(editor));
    });
}

void EditorTabs::requestRunFor(CodeEditor *editor)
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
        QMenu menu(editor);
        QAction *build = menu.addAction(tr("Build Image"));
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
        QMenu menu(editor);
        QAction *run = menu.addAction(tr("Run Compose Project"));
        QAction *newConfig = menu.addAction(tr("New Configuration..."));
        QAction *chosen = menu.exec(QCursor::pos());
        if (chosen == run) {
            runService_->runComposeFile(path);
        } else if (chosen == newConfig && runConfigEditor_) {
            const QString id = runService_->newComposeFileConfiguration(path);
            if (!id.isEmpty()) {
                showRunConfigDialog(editor, runConfigEditor_, containerService_, id);
            }
        }
        return;
    }

    runService_->runContext(path);
}

} // namespace ui_shell
