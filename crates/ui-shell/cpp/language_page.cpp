#include "language_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QComboBox>
#include <QFormLayout>
#include <QLabel>
#include <QLocale>
#include <QWidget>

#include <algorithm>

namespace ui_shell {

namespace {

// Qt's own nativeLanguageName() comes back lower-case ("deutsch",
// "español"); every other combo in Settings shows a capitalised label, so
// this matches that convention rather than surfacing Qt's raw casing.
QString capitalizedNativeName(const QLocale &locale)
{
    QString name = locale.nativeLanguageName();
    if (!name.isEmpty()) {
        name[0] = name[0].toUpper();
    }
    return name;
}

} // namespace

LanguagePage buildLanguagePage(QWidget *parent, AppSettings *appSettings)
{
    const QString originalLocale = appSettings->uiLocale();

    auto *page = new QWidget(parent);
    auto *form = new QFormLayout(page);

    // "en" first (it is the tr() source text and the default), then the
    // shipped translations in the order translations/README.md lists them.
    auto *localeCombo = new QComboBox(page);
    localeCombo->addItem(QStringLiteral("English"), QStringLiteral("en"));
    for (const char *code : {"de", "es", "fr"}) {
        localeCombo->addItem(capitalizedNativeName(QLocale(QString::fromLatin1(code))),
                              QString::fromLatin1(code));
    }
    localeCombo->setCurrentIndex(std::max(0, localeCombo->findData(originalLocale)));
    form->addRow(QObject::tr("Language:"), localeCombo);

    auto *notice = new QLabel(QObject::tr("Restart the app for a language change to take effect."), page);
    notice->setEnabled(false);
    notice->setWordWrap(true);
    form->addRow(QString(), notice);

    return LanguagePage{page, [appSettings, localeCombo]() {
                             appSettings->saveUiLocale(localeCombo->currentData().toString());
                         }};
}

} // namespace ui_shell
