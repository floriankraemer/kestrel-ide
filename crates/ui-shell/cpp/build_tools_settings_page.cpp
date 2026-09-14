#include "build_tools_settings_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QLabel>
#include <QLineEdit>
#include <QStringList>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

QComboBox *autoReloadCombo(QWidget *parent)
{
    auto *combo = new QComboBox(parent);
    combo->addItem(QObject::tr("External changes"), QStringLiteral("external"));
    combo->addItem(QObject::tr("Any changes"), QStringLiteral("any"));
    combo->addItem(QObject::tr("Never (show a banner)"), QStringLiteral("none"));
    return combo;
}

void selectData(QComboBox *combo, const QString &data)
{
    const int index = combo->findData(data);
    combo->setCurrentIndex(index >= 0 ? index : 0);
}

} // namespace

QWidget *buildBuildToolsSettingsPage(QWidget *parent, BuildToolsEditor *editor)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);

    auto *problemsLabel = new QLabel(page);
    problemsLabel->setWordWrap(true);
    problemsLabel->setStyleSheet(QStringLiteral("color: #c0392b;"));
    problemsLabel->setVisible(false);
    layout->addWidget(problemsLabel);

    const FfiBuildToolsFields fields = editor->fields();

    auto *gradleBox = new QGroupBox(QObject::tr("Gradle"), page);
    auto *gradleForm = new QFormLayout(gradleBox);
    auto *gradleDistribution = new QComboBox(gradleBox);
    gradleDistribution->addItem(QObject::tr("Project wrapper"), QStringLiteral("wrapper"));
    gradleDistribution->addItem(QObject::tr("Local installation"), QStringLiteral("gradle-home"));
    selectData(gradleDistribution, fields.gradleDistribution);
    auto *gradleHome = new QLineEdit(fields.gradleHome, gradleBox);
    auto *gradleJavaHome = new QLineEdit(fields.gradleJavaHome, gradleBox);
    auto *gradleOffline = new QCheckBox(gradleBox);
    gradleOffline->setChecked(fields.gradleOffline);
    auto *gradleAutoReload = autoReloadCombo(gradleBox);
    selectData(gradleAutoReload, fields.gradleAutoReload);
    auto *gradleDownloadSources = new QCheckBox(gradleBox);
    gradleDownloadSources->setChecked(fields.gradleDownloadSources);
    auto *gradleJvmArgs = new QLineEdit(fields.gradleJvmArgs, gradleBox);
    gradleForm->addRow(QObject::tr("Distribution:"), gradleDistribution);
    gradleForm->addRow(QObject::tr("Gradle home:"), gradleHome);
    gradleForm->addRow(QObject::tr("JDK home:"), gradleJavaHome);
    gradleForm->addRow(QObject::tr("Offline:"), gradleOffline);
    gradleForm->addRow(QObject::tr("Reload:"), gradleAutoReload);
    gradleForm->addRow(QObject::tr("Download sources:"), gradleDownloadSources);
    gradleForm->addRow(QObject::tr("JVM arguments:"), gradleJvmArgs);
    layout->addWidget(gradleBox);

    auto *mavenBox = new QGroupBox(QObject::tr("Maven"), page);
    auto *mavenForm = new QFormLayout(mavenBox);
    auto *mavenHome = new QLineEdit(fields.mavenHome, mavenBox);
    auto *mavenUserSettings = new QLineEdit(fields.mavenUserSettingsFile, mavenBox);
    auto *mavenLocalRepository = new QLineEdit(fields.mavenLocalRepository, mavenBox);
    auto *mavenOffline = new QCheckBox(mavenBox);
    mavenOffline->setChecked(fields.mavenOffline);
    auto *mavenSkipTests = new QCheckBox(mavenBox);
    mavenSkipTests->setChecked(fields.mavenSkipTests);
    auto *mavenThreads = new QLineEdit(fields.mavenThreads, mavenBox);
    auto *mavenAlwaysUpdate = new QCheckBox(mavenBox);
    mavenAlwaysUpdate->setChecked(fields.mavenAlwaysUpdateSnapshots);
    auto *mavenAutoReload = autoReloadCombo(mavenBox);
    selectData(mavenAutoReload, fields.mavenAutoReload);
    mavenForm->addRow(QObject::tr("Maven home:"), mavenHome);
    mavenForm->addRow(QObject::tr("User settings file:"), mavenUserSettings);
    mavenForm->addRow(QObject::tr("Local repository:"), mavenLocalRepository);
    mavenForm->addRow(QObject::tr("Offline:"), mavenOffline);
    mavenForm->addRow(QObject::tr("Skip tests:"), mavenSkipTests);
    mavenForm->addRow(QObject::tr("Threads:"), mavenThreads);
    mavenForm->addRow(QObject::tr("Always update snapshots:"), mavenAlwaysUpdate);
    mavenForm->addRow(QObject::tr("Reload:"), mavenAutoReload);
    layout->addWidget(mavenBox);
    layout->addStretch(1);

    const auto refreshProblems = [editor, problemsLabel]() {
        const ::rust::Vec<FfiBuildToolsProblem> problems = editor->problems();
        if (problems.empty()) {
            problemsLabel->setVisible(false);
            return;
        }
        QStringList sentences;
        for (const FfiBuildToolsProblem &problem : problems) {
            sentences << QString(problem.sentence);
        }
        problemsLabel->setText(sentences.join(QStringLiteral("\n")));
        problemsLabel->setVisible(true);
    };

    QObject::connect(gradleDistribution, &QComboBox::currentIndexChanged, editor,
                     [editor, gradleDistribution, refreshProblems](int) {
                         editor->setGradleDistribution(gradleDistribution->currentData().toString());
                         refreshProblems();
                     });
    QObject::connect(gradleHome, &QLineEdit::textChanged, editor,
                     [editor, refreshProblems](const QString &text) {
                         editor->setGradleHome(text);
                         refreshProblems();
                     });
    QObject::connect(gradleJavaHome, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setGradleJavaHome(text); });
    QObject::connect(gradleOffline, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setGradleOffline(on); });
    QObject::connect(gradleAutoReload, &QComboBox::currentIndexChanged, editor,
                     [editor, gradleAutoReload](int) {
                         editor->setGradleAutoReload(gradleAutoReload->currentData().toString());
                     });
    QObject::connect(gradleDownloadSources, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setGradleDownloadSources(on); });
    QObject::connect(gradleJvmArgs, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setGradleJvmArgs(text); });

    QObject::connect(mavenHome, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setMavenHome(text); });
    QObject::connect(mavenUserSettings, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setMavenUserSettingsFile(text); });
    QObject::connect(mavenLocalRepository, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setMavenLocalRepository(text); });
    QObject::connect(mavenOffline, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setMavenOffline(on); });
    QObject::connect(mavenSkipTests, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setMavenSkipTests(on); });
    QObject::connect(mavenThreads, &QLineEdit::textChanged, editor,
                     [editor, refreshProblems](const QString &text) {
                         editor->setMavenThreads(text);
                         refreshProblems();
                     });
    QObject::connect(mavenAlwaysUpdate, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setMavenAlwaysUpdateSnapshots(on); });
    QObject::connect(mavenAutoReload, &QComboBox::currentIndexChanged, editor,
                     [editor, mavenAutoReload](int) {
                         editor->setMavenAutoReload(mavenAutoReload->currentData().toString());
                     });

    refreshProblems();
    return page;
}

} // namespace ui_shell
