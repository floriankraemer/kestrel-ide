#include "php_settings_page.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>
#include <QWidget>

#include <memory>

namespace ui_shell {

namespace {

QString probeSummary(const FfiPhpProbe &probe)
{
    if (!probe.ok) {
        return QString(probe.error);
    }
    QStringList parts{QObject::tr("PHP %1").arg(QString(probe.version))};
    if (probe.xdebug) {
        const QString modes = QString(probe.xdebug_modes);
        parts << (modes.isEmpty() ? QObject::tr("Xdebug (off)")
                                  : QObject::tr("Xdebug (%1)").arg(modes));
    } else {
        parts << QObject::tr("no Xdebug");
    }
    if (probe.pcov) {
        parts << QObject::tr("PCOV");
    }
    return parts.join(QStringLiteral(" · "));
}

} // namespace

QWidget *buildPhpSettingsPage(QWidget *parent, PhpSettingsEditor *editor,
                              RunConfigEditor *runConfigEditor)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);
    auto *formLayout = new QFormLayout();
    layout->addLayout(formLayout);

    const FfiPhpForm form = editor->form();

    auto *interpreterEdit = new QLineEdit(QString(form.interpreter), page);
    interpreterEdit->setPlaceholderText(QObject::tr("php (from PATH)"));
    auto *detectButton = new QPushButton(QObject::tr("Detect"), page);
    auto *interpreterRow = new QHBoxLayout();
    interpreterRow->addWidget(interpreterEdit, 1);
    interpreterRow->addWidget(detectButton);
    formLayout->addRow(QObject::tr("Interpreter:"), interpreterRow);

    auto *probeLabel = new QLabel(page);
    probeLabel->setTextInteractionFlags(Qt::TextSelectableByMouse);
    formLayout->addRow(QString(), probeLabel);

    auto *targetCombo = new QComboBox(page);
    targetCombo->addItem(QObject::tr("This machine"), QString());
    for (const FfiContainerTarget &target : runConfigEditor->containerTargets()) {
        targetCombo->addItem(QString(target.name), QString(target.id));
    }
    const int targetIndex = targetCombo->findData(QString(form.container_target));
    targetCombo->setCurrentIndex(targetIndex >= 0 ? targetIndex : 0);
    formLayout->addRow(QObject::tr("Run in container:"), targetCombo);

    auto *modeCombo = new QComboBox(page);
    modeCombo->addItem(QObject::tr("Target default"), QString());
    modeCombo->addItem(QObject::tr("Exec in a running container"), QStringLiteral("exec"));
    modeCombo->addItem(QObject::tr("Run a new container"), QStringLiteral("run"));
    const int modeIndex = modeCombo->findData(QString(form.container_mode));
    modeCombo->setCurrentIndex(modeIndex >= 0 ? modeIndex : 0);
    formLayout->addRow(QObject::tr("Container mode:"), modeCombo);

    auto *levelEdit = new QLineEdit(QString(form.language_level), page);
    levelEdit->setPlaceholderText(QObject::tr("From composer.json, else the interpreter"));
    formLayout->addRow(QObject::tr("Language level:"), levelEdit);

    auto *includeEdit = new QPlainTextEdit(QString(form.include_paths), page);
    includeEdit->setPlaceholderText(QObject::tr("One path per line"));
    includeEdit->setMaximumHeight(80);
    formLayout->addRow(QObject::tr("Include paths:"), includeEdit);

    auto *stubsEdit = new QLineEdit(QString(form.stubs), page);
    stubsEdit->setPlaceholderText(QObject::tr("Server default; for example: redis, mongodb"));
    formLayout->addRow(QObject::tr("Stubs:"), stubsEdit);

    auto *licenceEdit = new QLineEdit(page);
    licenceEdit->setEchoMode(QLineEdit::Password);
    auto *removeLicenceButton = new QPushButton(QObject::tr("Remove"), page);
    auto *licenceRow = new QHBoxLayout();
    licenceRow->addWidget(licenceEdit, 1);
    licenceRow->addWidget(removeLicenceButton);
    formLayout->addRow(QObject::tr("Intelephense licence key:"), licenceRow);
    auto *licenceHint = new QLabel(page);
    licenceHint->setEnabled(false);
    formLayout->addRow(QString(), licenceHint);
    const auto refreshLicenceHint = [=]() {
        const bool has = editor->hasLicenceKey();
        licenceEdit->setPlaceholderText(has ? QObject::tr("A key is stored; type to replace it")
                                            : QObject::tr("Paste the key to unlock premium features"));
        licenceHint->setText(QObject::tr("Kept in the OS keychain, never in a settings file."));
        removeLicenceButton->setEnabled(has);
    };
    refreshLicenceHint();

    auto *serversBox = new QGroupBox(QObject::tr("Language servers"), page);
    auto *serversLayout = new QFormLayout(serversBox);
    auto *intelephenseOn = new QCheckBox(QObject::tr("Intelephense"), serversBox);
    intelephenseOn->setChecked(form.intelephense_enabled);
    auto *intelephenseDiagnostics = new QCheckBox(QObject::tr("Show its diagnostics"), serversBox);
    intelephenseDiagnostics->setChecked(form.intelephense_diagnostics);
    auto *phpactorOn = new QCheckBox(QObject::tr("Phpactor"), serversBox);
    phpactorOn->setChecked(form.phpactor_enabled);
    auto *phpactorDiagnostics = new QCheckBox(QObject::tr("Show its diagnostics"), serversBox);
    phpactorDiagnostics->setChecked(form.phpactor_diagnostics);
    serversLayout->addRow(intelephenseOn, intelephenseDiagnostics);
    serversLayout->addRow(phpactorOn, phpactorDiagnostics);
    layout->addWidget(serversBox);

    auto *errorLabel = new QLabel(page);
    errorLabel->setWordWrap(true);
    layout->addWidget(errorLabel);
    layout->addStretch(1);

    // One push of the whole form on any change: the Rust side validates and
    // either takes it or refuses it with a message shown under the form.
    const auto push = [=]() {
        FfiPhpForm next{};
        next.interpreter = interpreterEdit->text();
        next.language_level = levelEdit->text();
        next.include_paths = includeEdit->toPlainText();
        next.stubs = stubsEdit->text();
        next.container_target = targetCombo->currentData().toString();
        next.container_mode = modeCombo->currentData().toString();
        next.intelephense_enabled = intelephenseOn->isChecked();
        next.intelephense_diagnostics = intelephenseDiagnostics->isChecked();
        next.phpactor_enabled = phpactorOn->isChecked();
        next.phpactor_diagnostics = phpactorDiagnostics->isChecked();
        const FfiResult result = editor->setForm(next);
        errorLabel->setText(result.code == 0 ? QString() : QString(result.message));
        modeCombo->setEnabled(!next.container_target.isEmpty());
    };
    push();

    for (QLineEdit *edit : {interpreterEdit, levelEdit, stubsEdit}) {
        QObject::connect(edit, &QLineEdit::textChanged, page, push);
    }
    QObject::connect(includeEdit, &QPlainTextEdit::textChanged, page, push);
    for (QComboBox *combo : {targetCombo, modeCombo}) {
        QObject::connect(combo, &QComboBox::currentIndexChanged, page, push);
    }
    for (QCheckBox *check :
         {intelephenseOn, intelephenseDiagnostics, phpactorOn, phpactorDiagnostics}) {
        QObject::connect(check, &QCheckBox::toggled, page, push);
    }

    QObject::connect(licenceEdit, &QLineEdit::textEdited, page, [=](const QString &key) {
        editor->setLicenceKey(key);
        refreshLicenceHint();
    });
    QObject::connect(removeLicenceButton, &QPushButton::clicked, page, [=]() {
        licenceEdit->clear();
        editor->removeLicenceKey();
        refreshLicenceHint();
    });

    QObject::connect(detectButton, &QPushButton::clicked, page, [=]() {
        probeLabel->setText(QObject::tr("Detecting…"));
        editor->probeInterpreter(interpreterEdit->text());
    });
    QObject::connect(editor, &PhpSettingsEditor::probeFinished, page,
                     [=](const FfiPhpProbe &probe) { probeLabel->setText(probeSummary(probe)); });

    return page;
}

} // namespace ui_shell
