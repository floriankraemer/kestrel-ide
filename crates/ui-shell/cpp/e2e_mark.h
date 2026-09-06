#pragma once

#include <QString>

// The view's E2E marker stream.
//
// `e2eMark` appends one JSON line to the file named by the `IDE_E2E_EVENTS`
// environment variable and flushes it. With the variable unset — every
// normal run — the first call resolves to a null stream and every call after
// it returns immediately, so marks cost nothing in production.
//
// This does not violate the humble-view rule (CLAUDE.md): a mark contains no
// `if` encoding a business decision, and nothing downstream of the view reads
// it. It is the view reporting what it finished doing, the same category of
// statement as painting. It is also the only channel that can observe
// signal wiring, widget lifetime, focus routing and index-identity mapping —
// the bug classes `cpp/` has no other net for.
void e2eMark(const char *json);
void e2eMark(const QString &json);

// Reports every action in a menu — label, enabled state and screen rect —
// once the menu is actually laid out.
//
// A popup menu is the one widget an E2E flow cannot locate any other way:
// it is a separate toplevel with no model behind it, and counting `Down`
// presses to reach an entry silently re-targets itself the day someone adds
// an entry above it. Safe to call on any menu; free when `IDE_E2E_EVENTS` is
// unset, like every other mark.
void e2eMarkMenuActions(class QMenu *menu, const char *event);

// A JSON string literal — quoted and escaped — for embedding in a mark.
// Paths can contain quotes and backslashes, so no call site may interpolate
// one raw. Deliberately not a JSON library: this is the only JSON the view
// ever writes.
QString e2eJson(const QString &value);
