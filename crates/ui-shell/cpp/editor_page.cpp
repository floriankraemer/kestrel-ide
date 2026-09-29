#include "editor_page.h"

#include "code_editor.h"
#include "editor_tabs.h"

#include <QApplication>
#include <QCheckBox>
#include <QColor>
#include <QColorDialog>
#include <QFont>
#include <QFormLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QObject>
#include <QPalette>
#include <QPushButton>
#include <QSpinBox>
#include <QString>
#include <QVBoxLayout>
#include <QWidget>

#include <memory>

namespace ui_shell {

EditorPage buildEditorPage(QWidget *parent, AppSettings *appSettings, EditorTabs *editorTabs)
{
    const FfiEditorFont originalFont = appSettings->editorFont();
    const FfiEditorColors originalColors = appSettings->editorColors();

    auto *editorPage = new QWidget(parent);
    auto *editorForm = new QFormLayout(editorPage);
    auto *fontFamilyEdit = new QLineEdit(originalFont.family, editorPage);
    auto *fontSizeSpin = new QSpinBox(editorPage);
    fontSizeSpin->setRange(6, 72);
    fontSizeSpin->setValue(static_cast<int>(originalFont.size));
    editorForm->addRow(QObject::tr("Font family:"), fontFamilyEdit);
    editorForm->addRow(QObject::tr("Font size:"), fontSizeSpin);

    auto applyFontLive = [editorTabs, fontFamilyEdit, fontSizeSpin]() {
        editorTabs->setEditorFont(QFont(fontFamilyEdit->text(), fontSizeSpin->value()));
    };
    QObject::connect(fontFamilyEdit, &QLineEdit::textChanged, editorPage, applyFontLive);
    QObject::connect(fontSizeSpin, &QSpinBox::valueChanged, editorPage, applyFontLive);

    // Boxed so the color-picker lambdas (which need to both read and update
    // the chosen value across separate clicks) share one instance rather
    // than each capturing a stale copy.
    auto backgroundColor = std::make_shared<QString>(originalColors.background);
    auto foregroundColor = std::make_shared<QString>(originalColors.foreground);
    auto currentLineColor = std::make_shared<QString>(originalColors.current_line);
    auto applyColorsLive = [editorTabs, backgroundColor, foregroundColor, currentLineColor]() {
        editorTabs->setEditorColors(*backgroundColor, *foregroundColor, *currentLineColor);
    };

    auto *backgroundButton = new QPushButton(QObject::tr("Background Color..."), editorPage);
    QObject::connect(backgroundButton, &QPushButton::clicked, editorPage,
                      [parent, backgroundColor, applyColorsLive]() {
                          const QColor initial = backgroundColor->isEmpty()
                            ? QColor(Qt::white)
                            : QColor(*backgroundColor);
                          const QColor chosen = QColorDialog::getColor(
                            initial, parent, QObject::tr("Background Color"));
                          if (chosen.isValid()) {
                              *backgroundColor = chosen.name();
                              applyColorsLive();
                          }
                      });
    editorForm->addRow(backgroundButton);

    auto *foregroundButton = new QPushButton(QObject::tr("Text Color..."), editorPage);
    QObject::connect(foregroundButton, &QPushButton::clicked, editorPage,
                      [parent, foregroundColor, applyColorsLive]() {
                          const QColor initial = foregroundColor->isEmpty()
                            ? QColor(Qt::black)
                            : QColor(*foregroundColor);
                          const QColor chosen = QColorDialog::getColor(
                            initial, parent, QObject::tr("Text Color"));
                          if (chosen.isValid()) {
                              *foregroundColor = chosen.name();
                              applyColorsLive();
                          }
                      });
    editorForm->addRow(foregroundButton);

    auto *currentLineButton = new QPushButton(QObject::tr("Current Line Color..."), editorPage);
    QObject::connect(currentLineButton, &QPushButton::clicked, editorPage,
                      [parent, currentLineColor, applyColorsLive]() {
                          // Empty means "derived from the theme", which has no
                          // single hex to seed the picker with — the editor
                          // background is the closest starting point.
                          const QColor initial = currentLineColor->isEmpty()
                            ? qApp->palette().color(QPalette::Base)
                            : QColor(*currentLineColor);
                          const QColor chosen = QColorDialog::getColor(
                            initial, parent, QObject::tr("Current Line Color"));
                          if (chosen.isValid()) {
                              *currentLineColor = chosen.name();
                              applyColorsLive();
                          }
                      });
    editorForm->addRow(currentLineButton);

    // JetBrains-style "show whitespace characters": a master toggle plus
    // three sub-toggles that only mean anything when it is on, following
    // the same enable/disable-together shape mcp_page.cpp's port spin box
    // uses for mcpEnabledCheck. Indented under the master by nesting them
    // in their own layout rather than a QGroupBox, matching this page's
    // otherwise flat QFormLayout instead of introducing a second container
    // style.
    const FfiWhitespaceOptions originalWhitespace = appSettings->whitespaceOptions();
    auto *whitespaceCheck = new QCheckBox(QObject::tr("Show whitespace characters"), editorPage);
    whitespaceCheck->setChecked(originalWhitespace.enabled);
    editorForm->addRow(whitespaceCheck);

    auto *whitespaceSubLayout = new QVBoxLayout;
    whitespaceSubLayout->setContentsMargins(20, 0, 0, 0);
    auto *leadingCheck = new QCheckBox(QObject::tr("Leading"), editorPage);
    leadingCheck->setChecked(originalWhitespace.leading);
    auto *innerCheck = new QCheckBox(QObject::tr("Inner"), editorPage);
    innerCheck->setChecked(originalWhitespace.inner);
    auto *trailingCheck = new QCheckBox(QObject::tr("Trailing"), editorPage);
    trailingCheck->setChecked(originalWhitespace.trailing);
    for (QCheckBox *sub : {leadingCheck, innerCheck, trailingCheck}) {
        sub->setEnabled(whitespaceCheck->isChecked());
        whitespaceSubLayout->addWidget(sub);
    }
    editorForm->addRow(whitespaceSubLayout);
    QObject::connect(whitespaceCheck, &QCheckBox::toggled, editorPage,
                      [leadingCheck, innerCheck, trailingCheck](bool enabled) {
                          leadingCheck->setEnabled(enabled);
                          innerCheck->setEnabled(enabled);
                          trailingCheck->setEnabled(enabled);
                      });

    auto *eolCheck = new QCheckBox(QObject::tr("Show line endings"), editorPage);
    eolCheck->setChecked(originalWhitespace.eol_markers);
    editorForm->addRow(eolCheck);

    auto whitespaceOptionsFrom = [whitespaceCheck, leadingCheck, innerCheck, trailingCheck,
                                  eolCheck]() {
        return WhitespaceOptions{
          whitespaceCheck->isChecked(), leadingCheck->isChecked(), innerCheck->isChecked(),
          trailingCheck->isChecked(), eolCheck->isChecked()};
    };
    auto applyWhitespaceLive = [editorTabs, whitespaceOptionsFrom]() {
        editorTabs->setWhitespaceOptions(whitespaceOptionsFrom());
    };
    QObject::connect(whitespaceCheck, &QCheckBox::toggled, editorPage, applyWhitespaceLive);
    QObject::connect(leadingCheck, &QCheckBox::toggled, editorPage, applyWhitespaceLive);
    QObject::connect(innerCheck, &QCheckBox::toggled, editorPage, applyWhitespaceLive);
    QObject::connect(trailingCheck, &QCheckBox::toggled, editorPage, applyWhitespaceLive);
    QObject::connect(eolCheck, &QCheckBox::toggled, editorPage, applyWhitespaceLive);

    // Editor minimap (issue #199): a master toggle plus five overlay
    // sub-toggles, the same master/sub construction the whitespace group
    // above uses. `editorMinimapEnabled` is this page's first object name —
    // the E2E flow needs a stable target for the dialog-rects click.
    const FfiMinimapOptions originalMinimap = appSettings->minimapOptions();
    auto *minimapCheck = new QCheckBox(QObject::tr("Show minimap"), editorPage);
    minimapCheck->setObjectName(QStringLiteral("editorMinimapEnabled"));
    minimapCheck->setChecked(originalMinimap.enabled);
    editorForm->addRow(minimapCheck);

    auto *minimapSubLayout = new QVBoxLayout;
    minimapSubLayout->setContentsMargins(20, 0, 0, 0);
    auto *minimapSearchCheck = new QCheckBox(QObject::tr("Find matches"), editorPage);
    minimapSearchCheck->setChecked(originalMinimap.search_matches);
    auto *minimapDiagnosticsCheck = new QCheckBox(QObject::tr("Errors and warnings"), editorPage);
    minimapDiagnosticsCheck->setChecked(originalMinimap.diagnostics);
    auto *minimapVcsCheck = new QCheckBox(QObject::tr("VCS changes"), editorPage);
    minimapVcsCheck->setChecked(originalMinimap.vcs_changes);
    auto *minimapBreakpointsCheck = new QCheckBox(QObject::tr("Breakpoints"), editorPage);
    minimapBreakpointsCheck->setChecked(originalMinimap.breakpoints);
    auto *minimapCaretCheck = new QCheckBox(QObject::tr("Current line"), editorPage);
    minimapCaretCheck->setChecked(originalMinimap.caret_line);
    for (QCheckBox *sub : {minimapSearchCheck, minimapDiagnosticsCheck, minimapVcsCheck,
                           minimapBreakpointsCheck, minimapCaretCheck}) {
        sub->setEnabled(minimapCheck->isChecked());
        minimapSubLayout->addWidget(sub);
    }
    editorForm->addRow(minimapSubLayout);
    QObject::connect(
      minimapCheck, &QCheckBox::toggled, editorPage,
      [minimapSearchCheck, minimapDiagnosticsCheck, minimapVcsCheck, minimapBreakpointsCheck,
       minimapCaretCheck](bool enabled) {
          minimapSearchCheck->setEnabled(enabled);
          minimapDiagnosticsCheck->setEnabled(enabled);
          minimapVcsCheck->setEnabled(enabled);
          minimapBreakpointsCheck->setEnabled(enabled);
          minimapCaretCheck->setEnabled(enabled);
      });

    auto minimapOptionsFrom = [minimapCheck, minimapSearchCheck, minimapDiagnosticsCheck,
                               minimapVcsCheck, minimapBreakpointsCheck, minimapCaretCheck]() {
        return MinimapOptions{
          minimapCheck->isChecked(),          minimapSearchCheck->isChecked(),
          minimapDiagnosticsCheck->isChecked(), minimapVcsCheck->isChecked(),
          minimapBreakpointsCheck->isChecked(), minimapCaretCheck->isChecked()};
    };
    auto applyMinimapLive = [editorTabs, minimapOptionsFrom]() {
        editorTabs->setMinimapOptions(minimapOptionsFrom());
    };
    QObject::connect(minimapCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);
    QObject::connect(minimapSearchCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);
    QObject::connect(minimapDiagnosticsCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);
    QObject::connect(minimapVcsCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);
    QObject::connect(minimapBreakpointsCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);
    QObject::connect(minimapCaretCheck, &QCheckBox::toggled, editorPage, applyMinimapLive);

    // Hover card (H6): what a dwell shows and how long it waits. The delay is
    // meaningful only while some hover is on.
    const FfiHoverOptions originalHover = appSettings->hoverOptions();
    auto *hoverBox = new QGroupBox(QObject::tr("Hover"), editorPage);
    auto *hoverLayout = new QVBoxLayout(hoverBox);
    auto *hoverDocsCheck =
      new QCheckBox(QObject::tr("Show quick documentation on mouse hover"), hoverBox);
    hoverDocsCheck->setObjectName(QStringLiteral("editorHoverDocs"));
    hoverDocsCheck->setChecked(originalHover.docs_on_hover);
    hoverLayout->addWidget(hoverDocsCheck);

    auto *hoverDelayLayout = new QHBoxLayout;
    hoverDelayLayout->setContentsMargins(20, 0, 0, 0);
    auto *hoverDelaySpin = new QSpinBox(hoverBox);
    hoverDelaySpin->setObjectName(QStringLiteral("editorHoverDelay"));
    hoverDelaySpin->setRange(100, 3000);
    hoverDelaySpin->setSingleStep(100);
    hoverDelaySpin->setValue(static_cast<int>(originalHover.delay_ms));
    hoverDelayLayout->addWidget(new QLabel(QObject::tr("Delay"), hoverBox));
    hoverDelayLayout->addWidget(hoverDelaySpin);
    hoverDelayLayout->addWidget(new QLabel(QObject::tr("ms"), hoverBox));
    hoverDelayLayout->addStretch();
    hoverLayout->addLayout(hoverDelayLayout);

    auto *hoverProblemsCheck =
      new QCheckBox(QObject::tr("Show problems and quick fixes on mouse hover"), hoverBox);
    hoverProblemsCheck->setObjectName(QStringLiteral("editorHoverProblems"));
    hoverProblemsCheck->setChecked(originalHover.problems_on_hover);
    hoverLayout->addWidget(hoverProblemsCheck);
    editorForm->addRow(hoverBox);

    auto hoverOptionsFrom = [hoverDocsCheck, hoverDelaySpin, hoverProblemsCheck]() {
        return FfiHoverOptions{hoverDocsCheck->isChecked(),
                               static_cast<quint32>(hoverDelaySpin->value()),
                               hoverProblemsCheck->isChecked()};
    };
    auto applyHoverLive = [editorTabs, hoverDelaySpin, hoverDocsCheck, hoverProblemsCheck,
                           hoverOptionsFrom]() {
        hoverDelaySpin->setEnabled(hoverDocsCheck->isChecked() || hoverProblemsCheck->isChecked());
        editorTabs->setHoverOptions(hoverOptionsFrom());
    };
    hoverDelaySpin->setEnabled(originalHover.docs_on_hover || originalHover.problems_on_hover);
    QObject::connect(hoverDocsCheck, &QCheckBox::toggled, editorPage, applyHoverLive);
    QObject::connect(hoverProblemsCheck, &QCheckBox::toggled, editorPage, applyHoverLive);
    QObject::connect(hoverDelaySpin, &QSpinBox::valueChanged, editorPage, applyHoverLive);

    return EditorPage{
      editorPage,
      [appSettings, fontFamilyEdit, fontSizeSpin, backgroundColor, foregroundColor,
       currentLineColor, whitespaceOptionsFrom, minimapOptionsFrom, hoverOptionsFrom]() {
          appSettings->saveEditorFont(fontFamilyEdit->text(),
                                       static_cast<quint32>(fontSizeSpin->value()));
          appSettings->saveEditorColors(*backgroundColor, *foregroundColor, *currentLineColor);
          const WhitespaceOptions options = whitespaceOptionsFrom();
          appSettings->saveWhitespaceOptions(FfiWhitespaceOptions{
            options.enabled, options.leading, options.inner, options.trailing,
            options.eolMarkers});
          const MinimapOptions minimap = minimapOptionsFrom();
          appSettings->saveMinimapOptions(FfiMinimapOptions{
            minimap.enabled, minimap.searchMatches, minimap.diagnostics, minimap.vcsChanges,
            minimap.breakpoints, minimap.caretLine});
          appSettings->saveHoverOptions(hoverOptionsFrom());
      },
      [editorTabs, originalFont, originalColors, originalWhitespace, originalMinimap,
       originalHover]() {
          editorTabs->setEditorFont(
            QFont(originalFont.family, static_cast<int>(originalFont.size)));
          editorTabs->setEditorColors(originalColors.background, originalColors.foreground,
                                       originalColors.current_line);
          editorTabs->setWhitespaceOptions(WhitespaceOptions{
            originalWhitespace.enabled, originalWhitespace.leading, originalWhitespace.inner,
            originalWhitespace.trailing, originalWhitespace.eol_markers});
          editorTabs->setMinimapOptions(MinimapOptions{
            originalMinimap.enabled, originalMinimap.search_matches, originalMinimap.diagnostics,
            originalMinimap.vcs_changes, originalMinimap.breakpoints,
            originalMinimap.caret_line});
          editorTabs->setHoverOptions(originalHover);
      },
    };
}

} // namespace ui_shell
