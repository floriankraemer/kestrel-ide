#!/bin/sh
# Re-extracts every tr() string from crates/ui-shell/cpp into the three .ts
# files (translations/README.md). New strings arrive as type="unfinished";
# strings no longer in the sources are dropped.
set -eu

if [ -z "${LUPDATE:-}" ]; then
	for candidate in lupdate lupdate-qt6 lupdate6 /usr/lib/qt6/bin/lupdate; do
		if command -v "$candidate" >/dev/null 2>&1; then LUPDATE=$candidate; break; fi
	done
fi
[ -n "${LUPDATE:-}" ] || { echo "update-translations: lupdate not found (set LUPDATE=...)"; exit 1; }

T=crates/ui-shell/translations
exec "$LUPDATE" -no-obsolete -locations none crates/ui-shell/cpp -extensions cpp,h \
	-ts "$T/ide_de.ts" "$T/ide_es.ts" "$T/ide_fr.ts"
