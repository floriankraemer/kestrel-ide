#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QCheckBox;
class QComboBox;
class QLineEdit;
class QListWidget;
class QTableWidget;
class QToolButton;

namespace ui_shell {

// C5 (ADR-0056): the run-config dialog's container-kind page — Image,
// Containerfile, or Compose, switched by `setKind()`. Structured editing
// only: every field crosses the seam as its own typed `FfiContainerOptions`
// field, never JSON (see that struct's own doc comment in ffi.rs) —
// port/mount/env/build-arg/scale tables, an ordered compose-files list, and
// a `composeServices`-fed services picker, all editable with Add/Remove.
//
// Humble view: this widget only collects what the user typed into
// `options()`/reads `setOptions()` back; `RunConfigEditor::commandPreview`
// (Rust) is what turns that into the command line shown below it, and
// `container_core::run_config::validate_*` (also Rust) is what decides
// whether it is savable — this class encodes no rule of its own besides
// which rows are visible for which kind, which is display, not a decision.
class ContainerOptionsPage : public QWidget
{
    Q_OBJECT
public:
    explicit ContainerOptionsPage(ContainerService *containerService, RunConfigEditor *editor,
                                  QWidget *parent);

    // `""` (a plain process — the page shows nothing), `"container-image"`,
    // `"containerfile"`, or `"compose"`.
    void setKind(const QString &kind);

    void setOptions(const FfiContainerOptions &options);
    FfiContainerOptions options() const;

    // Re-reads `ContainerService::connections()` — called once up front and
    // whenever the Settings > Containers page could have changed the list.
    void refreshConnections();

signals:
    // Any field changed — the dialog's live "Command preview" listens here.
    void changed();

private:
    void buildUi();
    void requestServices();
    void onComposeServicesReady(const QString &services);
    void applyGroupVisibility();

    ContainerService *containerService_;
    RunConfigEditor *editor_;
    QString kind_;
    // Services the connection actually reports (from `composeServicesReady`),
    // kept so `options()` can preserve a checked service the picker has not
    // refreshed for yet (e.g. a slow connection) rather than silently
    // dropping it.
    QStringList knownServices_;
    QStringList selectedServices_;

    QComboBox *serverCombo_ = nullptr;

    // Image / Containerfile (shared).
    QLineEdit *imageEdit_ = nullptr;
    QLineEdit *containerNameEdit_ = nullptr;
    QCheckBox *publishAllPortsCheck_ = nullptr;
    QLineEdit *entrypointEdit_ = nullptr;
    QLineEdit *commandEdit_ = nullptr;
    QLineEdit *runOptionsEdit_ = nullptr;
    QCheckBox *runAttachCheck_ = nullptr;
    QComboBox *pullPolicyCombo_ = nullptr;
    QTableWidget *portsTable_ = nullptr;
    QTableWidget *mountsTable_ = nullptr;
    QTableWidget *envTable_ = nullptr;

    // Containerfile-only.
    QLineEdit *dockerfileEdit_ = nullptr;
    QLineEdit *contextDirEdit_ = nullptr;
    QLineEdit *imageTagEdit_ = nullptr;
    QTableWidget *buildArgsTable_ = nullptr;
    QLineEdit *buildOptionsEdit_ = nullptr;
    QCheckBox *runBuiltImageCheck_ = nullptr;

    // Compose-only.
    QListWidget *composeFilesList_ = nullptr;
    QListWidget *servicesList_ = nullptr;
    QLineEdit *projectNameEdit_ = nullptr;
    QLineEdit *profilesEdit_ = nullptr;
    QLineEdit *envFilesEdit_ = nullptr;
    QCheckBox *compatibilityCheck_ = nullptr;
    QCheckBox *removeOrphansOnDownCheck_ = nullptr;
    QCheckBox *removeVolumesOnDownCheck_ = nullptr;
    QComboBox *removeImagesOnDownCombo_ = nullptr;
    QLineEdit *sigkillTimeoutEdit_ = nullptr;
    QLineEdit *exitCodeFromEdit_ = nullptr;
    QTableWidget *scaleTable_ = nullptr;
    QCheckBox *alwaysRecreateDepsCheck_ = nullptr;
    QCheckBox *renewAnonVolumesCheck_ = nullptr;
    QCheckBox *removeOrphansCheck_ = nullptr;
    QCheckBox *noLogPrefixCheck_ = nullptr;
    QComboBox *startCombo_ = nullptr;
    QComboBox *composeAttachCombo_ = nullptr;
    QComboBox *recreateCombo_ = nullptr;
    QComboBox *buildCombo_ = nullptr;
    QCheckBox *abortOnExitCheck_ = nullptr;

    // "Modify options ▾": one checkable action per optional group, each
    // paired with the QGroupBox it shows/hides.
    QToolButton *modifyOptionsButton_ = nullptr;
    QWidget *portsGroup_ = nullptr;
    QWidget *mountsGroup_ = nullptr;
    QWidget *envGroup_ = nullptr;
    QWidget *buildArgsGroup_ = nullptr;
    QWidget *scaleGroup_ = nullptr;
    QWidget *advancedComposeGroup_ = nullptr;

    // The kind-specific containers stacked to show only the current kind's
    // rows (image/containerfile share most of theirs; compose is disjoint).
    QWidget *imageContainerfileSection_ = nullptr;
    QWidget *containerfileOnlySection_ = nullptr;
    QWidget *composeSection_ = nullptr;
};

} // namespace ui_shell
