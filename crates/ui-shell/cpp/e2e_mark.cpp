#include "e2e_mark.h"

#include <QByteArray>
#include <QAction>
#include <QMenu>
#include <QPoint>
#include <QRect>
#include <QTimer>

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <optional>

namespace {

// Opened once. Re-opening per call would cost a syscall on paths the view
// takes thousands of times, and appending from two handles interleaves.
std::FILE *markStream()
{
    static std::FILE *const stream = []() -> std::FILE * {
        const char *path = std::getenv("IDE_E2E_EVENTS");
        if (path == nullptr || *path == '\0') {
            return nullptr;
        }
        return std::fopen(path, "ae");
    }();
    return stream;
}

// Mutable, unlike `markStream()`'s stream handle: `e2eMarkStartupBegin()`
// sets this once, from `run_app()`, well before any marker that wants
// `e2eElapsedMs()` can fire.
std::optional<std::chrono::steady_clock::time_point> &startupBegin()
{
    static std::optional<std::chrono::steady_clock::time_point> begin;
    return begin;
}

} // namespace

void e2eMarkStartupBegin()
{
    startupBegin() = std::chrono::steady_clock::now();
}

qint64 e2eElapsedMs()
{
    const auto &begin = startupBegin();
    if (!begin.has_value()) {
        return 0;
    }
    return std::chrono::duration_cast<std::chrono::milliseconds>(
             std::chrono::steady_clock::now() - *begin)
      .count();
}

void e2eMark(const char *json)
{
    std::FILE *stream = markStream();
    if (stream == nullptr) {
        return;
    }
    std::fputs(json, stream);
    std::fputc('\n', stream);
    // Flushed per line: a test reads this file while the app is still
    // running, and a crash must not swallow the marks that explain it.
    std::fflush(stream);
}

void e2eMark(const QString &json)
{
    if (markStream() == nullptr) {
        return;
    }
    e2eMark(json.toUtf8().constData());
}

QString e2eJson(const QString &value)
{
    QString out;
    out.reserve(value.size() + 2);
    out += QLatin1Char('"');
    for (const QChar character : value) {
        const char16_t code = character.unicode();
        switch (code) {
        case u'"':
            out += QLatin1String("\\\"");
            break;
        case u'\\':
            out += QLatin1String("\\\\");
            break;
        case u'\n':
            out += QLatin1String("\\n");
            break;
        case u'\r':
            out += QLatin1String("\\r");
            break;
        case u'\t':
            out += QLatin1String("\\t");
            break;
        default:
            if (code < 0x20) {
                out += QString::asprintf("\\u%04x", code);
            } else {
                out += character;
            }
            break;
        }
    }
    out += QLatin1Char('"');
    return out;
}

void e2eMarkMenuActions(QMenu *menu, const char *event)
{
    // `aboutToShow` fires before the menu is laid out, so its action
    // geometry is still empty; one turn of the event loop later it is on
    // screen with real rects.
    const QString name = QString::fromUtf8(event);
    QObject::connect(menu, &QMenu::aboutToShow, menu, [menu, name]() {
        QTimer::singleShot(0, menu, [menu, name]() {
            for (QAction *action : menu->actions()) {
                if (action->isSeparator()) {
                    continue;
                }
                const QRect rect = menu->actionGeometry(action);
                const QPoint origin = rect.isEmpty() ? QPoint() : menu->mapToGlobal(rect.topLeft());
                e2eMark(QStringLiteral("{\"ev\":%1,\"label\":%2,\"enabled\":%3,"
                                        "\"rect\":[%4,%5,%6,%7]}")
                          .arg(e2eJson(name), e2eJson(action->text()),
                                action->isEnabled() ? "true" : "false")
                          .arg(origin.x())
                          .arg(origin.y())
                          .arg(rect.width())
                          .arg(rect.height()));
            }
        });
    });
}
