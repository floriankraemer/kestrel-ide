#include "appearance_page.h"

#include "icon_cache.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QComboBox>
#include <QFormLayout>
#include <QSpinBox>
#include <QWidget>

#include <algorithm>

namespace ui_shell {

void applyUiFontScales(const FfiUiFontScales &scales, const UiFontTargets &targets)
{
    applyUiFontScale(static_cast<int>(scales.ui));
    applyWidgetFontScale(targets.menuBar, static_cast<int>(scales.menu));
    applyWidgetFontScale(targets.projectTree, static_cast<int>(scales.project_tree));
    // Fixed at build time from the status bar's font metrics, so a scaled
    // status bar would otherwise leave the indexing bar at the old height.
    if (targets.indexBar != nullptr) {
        targets.indexBar->setFixedHeight(targets.indexBar->fontMetrics().height());
    }
}

AppearancePage buildAppearancePage(QWidget *parent,
                                   AppSettings *appSettings,
                                   const UiFontTargets &targets,
                                   AppearanceHooks hooks)
{
    IconProvider *iconProvider = sharedIconProvider();
    ThemeProvider *themeProvider = sharedThemeProvider();
    const QString originalTheme = appSettings->themeName();
    const QString originalIconTheme = appSettings->iconThemeId();
    const FfiUiFontScales originalScales = appSettings->uiFontScales();

    auto *page = new QWidget(parent);
    auto *form = new QFormLayout(page);

    auto *themeCombo = new QComboBox(page);
    for (const FfiColorThemeChoice &theme : themeProvider->colorThemes()) {
        themeCombo->addItem(theme.label, theme.id);
    }
    // core-themes is the built-in plugin behind these entries; disabling it
    // entirely is the only way this combo ends up empty (T6's fallback chain
    // keeps a theme selected otherwise), so this guard is defensive, not the
    // normal path.
    themeCombo->setEnabled(themeCombo->count() > 0);
    // findData() of an unknown persisted name (or of an empty combo) yields
    // -1; falling back to 0 lands on Dark when it exists, and is a documented
    // no-op on setCurrentIndex when the combo is empty.
    themeCombo->setCurrentIndex(std::max(0, themeCombo->findData(originalTheme)));
    form->addRow(QObject::tr("Theme:"), themeCombo);

    // The icon themes the loaded plugins offer, in the order the registry
    // hands them over — the same combo shape as the colour theme above, and
    // for the same reason: the id is data on the item, the label is what the
    // user reads, and neither is composed here.
    auto *iconThemeCombo = new QComboBox(page);
    for (const FfiIconTheme &theme : iconProvider->iconThemes()) {
        iconThemeCombo->addItem(theme.label, theme.id);
    }
    // No plugin offers one: the row still exists, saying so, rather than
    // vanishing and leaving the user wondering where icons come from.
    iconThemeCombo->setEnabled(iconThemeCombo->count() > 0);
    if (iconThemeCombo->count() == 0) {
        iconThemeCombo->addItem(QObject::tr("No icon theme is installed"), QString());
    }
    iconThemeCombo->setCurrentIndex(std::max(0, iconThemeCombo->findData(originalIconTheme)));
    form->addRow(QObject::tr("Icon theme:"), iconThemeCombo);

    // Percentages of the platform's own UI font rather than absolute point
    // sizes, so the same settings.toml stays sane on a machine whose default
    // UI font is a different size. The editor keeps its own absolute font
    // (Settings > Editor) — it is content, not chrome.
    auto makeScaleSpin = [page](quint32 value) {
        auto *spin = new QSpinBox(page);
        spin->setRange(50, 300);
        spin->setSingleStep(10);
        spin->setSuffix(QStringLiteral(" %"));
        spin->setValue(static_cast<int>(value));
        return spin;
    };
    QSpinBox *uiScaleSpin = makeScaleSpin(originalScales.ui);
    QSpinBox *treeScaleSpin = makeScaleSpin(originalScales.project_tree);
    QSpinBox *menuScaleSpin = makeScaleSpin(originalScales.menu);
    form->addRow(QObject::tr("Interface font scale:"), uiScaleSpin);
    form->addRow(QObject::tr("Project tree font scale:"), treeScaleSpin);
    form->addRow(QObject::tr("Menu font scale:"), menuScaleSpin);

    auto chosenScales = [uiScaleSpin, treeScaleSpin, menuScaleSpin]() {
        return FfiUiFontScales{static_cast<quint32>(uiScaleSpin->value()),
                               static_cast<quint32>(treeScaleSpin->value()),
                               static_cast<quint32>(menuScaleSpin->value())};
    };

    auto applyScalesLive = [chosenScales, targets, hooks]() {
        applyUiFontScales(chosenScales(), targets);
        hooks.relayout();
    };
    for (QSpinBox *spin : {uiScaleSpin, treeScaleSpin, menuScaleSpin}) {
        QObject::connect(spin, &QSpinBox::valueChanged, page, applyScalesLive);
    }

    // Applying a colour theme is two changes, not one: the chrome, and the
    // icon art the pack ships a light variant of.
    auto applyThemeLive = [iconProvider, hooks](const QString &name) {
        applyTheme(name);
        iconProvider->applyColorTheme(name);
        hooks.refreshEditors();
        hooks.refreshIcons();
    };

    QObject::connect(themeCombo, &QComboBox::currentIndexChanged, page,
                     [themeCombo, applyThemeLive]() {
                         applyThemeLive(themeCombo->currentData().toString());
                     });

    QObject::connect(iconThemeCombo, &QComboBox::currentIndexChanged, page,
                     [iconThemeCombo, iconProvider, hooks]() {
                         iconProvider->applyIconTheme(iconThemeCombo->currentData().toString());
                         hooks.refreshIcons();
                     });

    auto commit = [appSettings, themeCombo, iconThemeCombo, chosenScales]() {
        appSettings->saveTheme(themeCombo->currentData().toString());
        appSettings->saveIconTheme(iconThemeCombo->currentData().toString());
        const FfiUiFontScales scales = chosenScales();
        appSettings->saveUiFontScales(scales.ui, scales.project_tree, scales.menu);
    };

    auto revert = [iconProvider, targets, hooks, applyThemeLive, originalTheme, originalIconTheme,
                    originalScales]() {
        iconProvider->applyIconTheme(originalIconTheme);
        applyThemeLive(originalTheme);
        applyUiFontScales(originalScales, targets);
    };

    return AppearancePage{page, commit, revert};
}

} // namespace ui_shell
