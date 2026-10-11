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
#include "e2e_mark.h"
#include "run_config_dialog.h"
#include "run_console_panel.h"

#include <algorithm>
#include <QAction>
#include <QCursor>
#include <QDesktopServices>
#include <QDir>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>
#include <QHash>
#include <QPlainTextEdit>
#include <QSet>
#include <QTimer>
#include <QStatusBar>
#include <QUrl>

namespace ui_shell {

namespace {
// `errors::CODE_NOT_A_TEST`: the file is not a test file, run it the ordinary way.
constexpr int kNotATestCode = 1014;
// E3: a run streams tree changes by the message; the icons repaint at most
// this often.
constexpr int kStatusRepaintMs = 100;

RunMarkerGlyph toGlyph(FfiMarkerState state)
{
    switch (state) {
    case FfiMarkerState::Running:
        return RunMarkerGlyph::Running;
    case FfiMarkerState::Passed:
        return RunMarkerGlyph::Passed;
    case FfiMarkerState::Failed:
        return RunMarkerGlyph::Failed;
    case FfiMarkerState::Ignored:
        return RunMarkerGlyph::Ignored;
    case FfiMarkerState::NeverRun:
        break;
    }
    return RunMarkerGlyph::Passed; // unreachable: NeverRun entries are not pushed
}

// Words for the counts and message `TestService::markerStatuses` returns.
QString statusTooltip(const FfiMarkerStatus &status)
{
    const QString message(status.message);
    switch (status.state) {
    case FfiMarkerState::Running:
        return QObject::tr("Running…");
    case FfiMarkerState::Failed:
        return !message.isEmpty()
                 ? message
                 : QObject::tr("%1 of %n tests failed", nullptr, static_cast<int>(status.total))
                     .arg(status.failed);
    case FfiMarkerState::Ignored:
        return message.isEmpty() ? QObject::tr("Ignored") : QObject::tr("Ignored: %1").arg(message);
    case FfiMarkerState::Passed:
        return status.passed > 1
                 ? QObject::tr("Passed (%n tests, %1 ms)", nullptr, static_cast<int>(status.passed))
                     .arg(status.duration_ms)
                 : QObject::tr("Passed (%1 ms)").arg(status.duration_ms);
    case FfiMarkerState::NeverRun:
        break;
    }
    return {};
}

QString comparablePath(const QString &path)
{
    return QDir::cleanPath(QDir::fromNativeSeparators(path));
}
}  // namespace

void wireRunService(RunService *runService, EditorTabs *editorTabs, RunConfigEditor *runConfigEditor,
                    ContainerService *containerService, RunConsolePanel *runConsolePanel,
                    DebugService *debugService, TestService *testService)
{
    editorTabs->setRunService(runService);
    wirePhpUnitConfigurations(runService, runConsolePanel, debugService, testService,
                              runConsolePanel->window()); // E4
    editorTabs->setContainerRunContext(runConfigEditor, containerService); // C5
    // A configuration list that just changed can make a file runnable that
    // was not — the first `cargo run` entry a detection scan adds, say — so
    // the open editor's gutter is re-asked rather than left stale.
    QObject::connect(runService, &RunService::configurationsChanged, editorTabs,
                      [editorTabs]() { editorTabs->refreshRunMarkers(); });
}

void wireTestGutter(TestService *testService, DebugService *debugService, EditorTabs *editorTabs)
{
    editorTabs->setTestService(testService);
    // A coverage run (or clearing it) repaints every open editor's stripes.
    QObject::connect(testService, &TestService::coverageChanged, editorTabs,
                      [editorTabs]() { editorTabs->refreshRunMarkers(); });
    // E3: the tree changes with every streamed message; coalesce into one
    // icon repaint per interval.
    auto *statusRepaint = new QTimer(editorTabs);
    statusRepaint->setSingleShot(true);
    statusRepaint->setInterval(kStatusRepaintMs);
    QObject::connect(statusRepaint, &QTimer::timeout, editorTabs,
                      [editorTabs]() { editorTabs->refreshRunMarkerStatuses(); });
    QObject::connect(testService, &TestService::testTreeChanged, statusRepaint, [statusRepaint]() {
        if (!statusRepaint->isActive()) {
            statusRepaint->start();
        }
    });
    // A debugged test starts once `DebugService` has the listener up.
    QObject::connect(debugService, &DebugService::phpTestLaunchRequested, testService,
                      [testService](const QString &envJson) { testService->runPendingWithEnv(envJson); });
}

void wirePhpUnitConfigurations(RunService *runService, RunConsolePanel *runConsolePanel,
                               DebugService *debugService, TestService *testService,
                               QWidget *window)
{
    const auto report = [window](const QString &title, const FfiResult &result) {
        if (result.code != 0) {
            QMessageBox::warning(window, title, result.message);
        }
    };
    QObject::connect(runService, &RunService::testConfigurationRequested, testService,
                      [testService, report](const QString &configId, bool withCoverage) {
                          report(QObject::tr("Run"),
                                 testService->runConfiguration(configId, withCoverage));
                      });
    QObject::connect(debugService, &DebugService::testConfigurationDebugRequested, testService,
                      [testService, debugService, report](const QString &configId) {
                          FfiResult result = testService->prepareDebugConfiguration(configId);
                          if (result.code == 0) {
                              result = debugService->debugPhpTests();
                          }
                          report(QObject::tr("Debug"), result);
                      });
    // An editor run just created a temporary configuration: the list gains
    // it, then the toolbar selects it, so Rerun and the combo name what ran.
    QObject::connect(testService, &TestService::configurationRemembered, runConsolePanel,
                      [runService, runConsolePanel](const QString &configId) {
                          runService->detectConfigurations();
                          runConsolePanel->selectConfiguration(configId);
                      });
}

void EditorTabs::setRunService(RunService *runService)
{
    runService_ = runService;
}

void EditorTabs::setTestService(TestService *testService)
{
    testService_ = testService;
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
    e2eMark(QStringLiteral("{\"ev\":\"compose_lenses\",\"path\":%1,\"count\":%2}")
              .arg(e2eJson(path)).arg(lenses.size()));
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
        const QString text = editor->toPlainText();
        for (const quint32 line : runService_->runLines(path, text)) {
            lines.insert(static_cast<int>(line));
        }
        if (testService_ != nullptr) {
            for (const quint32 line : testService_->markerLines(path, text)) {
                lines.insert(static_cast<int>(line));
            }
        }
    }
    editor->setRunLines(lines);
    pushRunMarkerStatuses(editor, path);
    QSet<int> covered;
    QSet<int> uncovered;
    if (testService_ != nullptr && !path.isEmpty()) {
        for (const quint32 line : testService_->coveredLines(path)) {
            covered.insert(static_cast<int>(line));
        }
        for (const quint32 line : testService_->uncoveredLines(path)) {
            uncovered.insert(static_cast<int>(line));
        }
    }
    editor->setCoverageLines(covered, uncovered);
    if (!covered.isEmpty() || !uncovered.isEmpty()) {
        const auto joined = [](const QSet<int> &set) {
            QList<int> sorted(set.begin(), set.end());
            std::sort(sorted.begin(), sorted.end());
            QStringList numbers;
            for (const int line : sorted) {
                numbers << QString::number(line);
            }
            return numbers.join(QLatin1Char(','));
        };
        e2eMark(QStringLiteral("{\"ev\":\"coverage_lines\",\"path\":%1,\"covered\":[%2],\"uncovered\":[%3]}")
                  .arg(e2eJson(path), joined(covered), joined(uncovered)));
    }
    QStringList lineNumbers;
    for (const int line : lines) {
        lineNumbers << QString::number(line);
    }
    e2eMark(QStringLiteral("{\"ev\":\"gutter_run_lines\",\"path\":%1,\"lines\":[%2]}")
              .arg(e2eJson(path), lineNumbers.join(QLatin1Char(','))));
}

void EditorTabs::pushRunMarkerStatuses(CodeEditor *editor, const QString &path)
{
    QHash<int, RunMarkerStatus> statuses;
    if (testService_ != nullptr && !path.isEmpty()) {
        for (const FfiMarkerStatus &status : testService_->markerStatuses(path, editor->toPlainText())) {
            if (status.state != FfiMarkerState::NeverRun) {
                statuses.insert(static_cast<int>(status.line),
                                RunMarkerStatus{ toGlyph(status.state), statusTooltip(status) });
            }
        }
    }
    editor->setRunMarkerStatuses(statuses);
}

void EditorTabs::refreshRunMarkerStatuses()
{
    // One pass over the tree names the files the run touched; every other
    // editor (and any without test markers) is spared a parse and a walk.
    QSet<QString> tested;
    if (testService_ != nullptr) {
        for (const QString &file : testService_->testedFiles()) {
            tested.insert(comparablePath(file));
        }
    }
    forEachEditor([this, &tested](QPlainTextEdit *plain) {
        auto *editor = qobject_cast<CodeEditor *>(plain);
        if (editor == nullptr) {
            return;
        }
        const QString path = docManager_->tabPath(editor->property("tabId").toULongLong());
        if (editor->runnable() && tested.contains(comparablePath(path))) {
            pushRunMarkerStatuses(editor, path);
        } else {
            editor->setRunMarkerStatuses({});
        }
    });
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

    if (testService_ != nullptr
        && !testService_->markerName(path, editor->toPlainText(), static_cast<quint32>(line)).isEmpty()) {
        showTestMarkerMenu(editor, path, line);
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
        e2eMarkMenuActions(&menu, "run_gutter_menu_action");
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
        e2eMarkMenuActions(&menu, "run_gutter_menu_action");
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

void EditorTabs::runContextAtCaret(CodeEditor *editor, bool debug)
{
    if (!editor) {
        return;
    }
    const QString path = docManager_->tabPath(editor->property("tabId").toULongLong());
    if (testService_ == nullptr || path.isEmpty()) {
        if (!debug) {
            requestRunFor(editor);
        } else if (auto *mainWindow = qobject_cast<QMainWindow *>(window_)) {
            mainWindow->statusBar()->showMessage(tr("There is no test in this file to debug."), 6000);
        }
        return;
    }
    const quint32 line = static_cast<quint32>(editor->textCursor().blockNumber());
    FfiResult result = testService_->runCaretContext(path, editor->toPlainText(), line, debug,
                                                     debugService_ != nullptr);
    if (result.code == kNotATestCode) {
        requestRunFor(editor);
        return;
    }
    if (result.code == 0 && debug) {
        result = debugService_->debugPhpTests();
    }
    auto *mainWindow = qobject_cast<QMainWindow *>(window_);
    if (result.code != 0 && mainWindow != nullptr) {
        mainWindow->statusBar()->showMessage(QString(result.message), 6000);
    }
}

void EditorTabs::showTestMarkerMenu(CodeEditor *editor, const QString &path, int line)
{
    const QString text = editor->toPlainText();
    const quint32 testLine = static_cast<quint32>(line);
    const QString name = testService_->markerName(path, text, testLine);
    QMenu menu(editor);
    QAction *run = menu.addAction(tr("Run '%1'").arg(name));
    QAction *debug = debugService_ != nullptr ? menu.addAction(tr("Debug '%1'").arg(name)) : nullptr;
    QAction *coverage = menu.addAction(tr("Run '%1' with Coverage").arg(name));
    // E3: each data set of the last run, runnable on its own.
    QHash<QAction *, QString> runDataSet;
    QHash<QAction *, QString> debugDataSet;
    const FfiDataSetChoices dataSets = testService_->markerDataSets(path, text, testLine);
    if (!dataSets.sets.empty()) {
        menu.addSeparator();
    }
    for (const FfiDataSet &set : dataSets.sets) {
        const QString id(set.id);
        const QString label(set.label);
        runDataSet.insert(menu.addAction(tr("Run '%1'").arg(label)), id);
        if (debugService_ != nullptr) {
            debugDataSet.insert(menu.addAction(tr("Debug '%1'").arg(label)), id);
        }
    }
    if (dataSets.hidden > 0) {
        menu.addAction(tr("… (%n more)", nullptr, static_cast<int>(dataSets.hidden)))->setEnabled(false);
    }
    e2eMarkMenuActions(&menu, "run_gutter_menu_action");
    QAction *chosen = menu.exec(QCursor::pos());
    FfiResult result;
    if (chosen == run) {
        result = testService_->runMarker(path, text, testLine);
    } else if (chosen == coverage) {
        result = testService_->runMarkerWithCoverage(path, text, testLine);
    } else if (debug != nullptr && chosen == debug) {
        result = testService_->prepareDebugMarker(path, text, testLine);
        if (result.code == 0) {
            result = debugService_->debugPhpTests();
        }
    } else if (runDataSet.contains(chosen)) {
        result = testService_->runDataSet(runDataSet.value(chosen));
    } else if (debugDataSet.contains(chosen)) {
        result = testService_->prepareDebugDataSet(debugDataSet.value(chosen));
        if (result.code == 0) {
            result = debugService_->debugPhpTests();
        }
    } else {
        return;
    }
    auto *mainWindow = qobject_cast<QMainWindow *>(window_);
    if (result.code != 0 && mainWindow != nullptr) {
        mainWindow->statusBar()->showMessage(QString(result.message), 6000);
    }
}

} // namespace ui_shell
