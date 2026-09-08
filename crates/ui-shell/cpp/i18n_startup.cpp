#include "i18n_startup.h"

#include "e2e_mark.h"

#include <QApplication>
#include <QLibraryInfo>
#include <QLocale>
#include <QTranslator>

// Q_INIT_RESOURCE expands to a call to an `extern` function that is not
// given C linkage, so it is looked up in whatever namespace it is written
// in rather than resolving to the global symbol `rcc --name translations`
// actually emits (an undefined-symbol link error, not a compile error) —
// Qt's own docs call this out: the macro must run at true global scope.
// Kept in this file rather than main_window.cpp purely so that file's
// startup sequence reads as one call, not a namespace-scope carve-out.
static void initTranslationResources()
{
    Q_INIT_RESOURCE(translations);
}

namespace ui_shell {

void installUiTranslators(AppSettings *appSettings, QApplication &app)
{
    initTranslationResources();

    const QString locale = appSettings->uiLocale();
    if (locale != QStringLiteral("en")) {
        // Qt's own shipped translations for standard dialog buttons (OK,
        // Cancel, ...) — degrades silently to English if this Qt install
        // was not built with its translations, same as the app translator
        // below degrades if the locale has no .qm.
        auto *qtTranslator = new QTranslator(&app);
        if (qtTranslator->load(QLocale(locale), QStringLiteral("qtbase"), QStringLiteral("_"),
                                QLibraryInfo::path(QLibraryInfo::TranslationsPath))) {
            app.installTranslator(qtTranslator);
        }
        auto *appTranslator = new QTranslator(&app);
        if (appTranslator->load(QStringLiteral(":/i18n/ide_%1.qm").arg(locale))) {
            app.installTranslator(appTranslator);
        }
    }
    e2eMark(QStringLiteral("{\"ev\":\"ui_locale_active\",\"locale\":\"%1\"}").arg(locale));
}

} // namespace ui_shell
