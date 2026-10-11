#!/bin/sh
# i18n gate (ADR-0049): the committed ui-shell .ts files must match what
# `lupdate` extracts from the C++ sources, hold no unfinished message, and
# every context lupdate names must resolve to the same context at run time.
#
# Needs `lupdate` (Qt6 l10n tools, present in linux-builder). Like
# check-test-layout.sh it avoids git so it works inside a worktree mount.
set -eu
export LC_ALL=C

if [ -z "${LUPDATE:-}" ]; then
	for candidate in lupdate lupdate-qt6 lupdate6 /usr/lib/qt6/bin/lupdate; do
		if command -v "$candidate" >/dev/null 2>&1; then LUPDATE=$candidate; break; fi
	done
fi
[ -n "${LUPDATE:-}" ] || { echo "check-translations: lupdate not found (set LUPDATE=...)"; exit 1; }

SRC=crates/ui-shell/cpp
TS_DIR=crates/ui-shell/translations
LOCALES="de es fr"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failed=0

# --- 1. Context check -------------------------------------------------------
# lupdate names a message's context after the lexically enclosing class; Qt
# resolves tr() through the nearest class with Q_OBJECT, or the one named in
# Q_DECLARE_TR_FUNCTIONS. A class with neither (e.g. a plain QWidget subclass)
# therefore translates in its base class's context and its .ts entries never
# match. `QObject` is the context of free-function QObject::tr().
# Classes are tracked by the last `class|struct Name` line seen before the
# macro, which holds for this codebase's one-class-per-block headers.
find "$SRC" -type f \( -name '*.h' -o -name '*.cpp' \) | sort | xargs awk '
	FNR == 1 { cls = "" }
	/^[ \t]*(class|struct)[ \t]+[A-Za-z_][A-Za-z_0-9]*/ && !/;[ \t]*$/ {
		line = $0; sub(/^[ \t]*(class|struct)[ \t]+/, "", line); sub(/[^A-Za-z_0-9].*/, "", line); cls = line
	}
	/^[ \t]*Q_OBJECT[ \t]*$/ && cls != "" { print cls }
	/^[ \t]*Q_DECLARE_TR_FUNCTIONS\(/ { line = $0; sub(/.*Q_DECLARE_TR_FUNCTIONS\(/, "", line); sub(/\).*/, "", line); print line }
' | sed 's/.*:://' | sort -u >"$tmp/resolved"
# Explicit QCoreApplication::translate("Ctx", ...) / QT_TRANSLATE_NOOP("Ctx", ...)
# name their context themselves.
grep -rhoE '(translate|QT_TRANSLATE_NOOP)\("[A-Za-z_:0-9]+"' "$SRC" | sed 's/.*("//; s/"$//' >>"$tmp/resolved" || true
echo QObject >>"$tmp/resolved"
sort -u -o "$tmp/resolved" "$tmp/resolved"

for loc in $LOCALES; do
	cp "$TS_DIR/ide_$loc.ts" "$tmp/ide_$loc.ts"
done
# shellcheck disable=SC2046
"$LUPDATE" -no-obsolete -locations none "$SRC" -extensions cpp,h \
	-ts $(for loc in $LOCALES; do printf '%s ' "$tmp/ide_$loc.ts"; done) >"$tmp/lupdate.log" 2>&1 \
	|| { cat "$tmp/lupdate.log"; exit 1; }

sed -n 's|^[ \t]*<name>\(.*\)</name>$|\1|p' "$tmp/ide_de.ts" | sort -u >"$tmp/contexts_all"
# <name> also matches nothing else at top level: context names only (message
# <source> lines use <source>).
sed 's/.*:://' "$tmp/contexts_all" | sort -u | comm -23 - "$tmp/resolved" >"$tmp/bad_ctx"
if [ -s "$tmp/bad_ctx" ]; then
	while IFS= read -r c; do
		echo "FAIL context '$c': tr() is called in a class with neither Q_OBJECT nor Q_DECLARE_TR_FUNCTIONS($c);"
		echo "     at run time it resolves to a base class and its translations never match."
	done <"$tmp/bad_ctx"
	failed=1
fi

# --- 2. Staleness and unfinished entries -----------------------------------
for loc in $LOCALES; do
	committed="$TS_DIR/ide_$loc.ts"
	fresh="$tmp/ide_$loc.ts"
	if ! diff -q "$committed" "$fresh" >/dev/null; then
		echo "FAIL $committed is stale or has new/vanished messages vs the sources."
		echo "     Run: make update-translations   then translate the new entries."
		failed=1
	fi
	n=$(grep -c 'type="unfinished"' "$fresh" || true)
	if [ "$n" -gt 0 ]; then
		echo "FAIL $committed: $n unfinished message(s)."
		failed=1
	fi
done

if [ "$failed" -eq 0 ]; then
	echo "translations gate: ok"
else
	echo "translations gate failed"
	exit 1
fi
