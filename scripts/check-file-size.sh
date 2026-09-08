#!/bin/sh
# File-size gate: no source file under crates/ may grow without bound.
#
# Files are discovered with `find`, not `git ls-files`, on purpose: this repo
# is worked on in git worktrees, where `.git` is a *file* pointing outside the
# Docker mount (the Makefile's RUN_LINUX mounts only $(CURDIR) at /workspace),
# so any git invocation inside the container fails. Do not "improve" this back
# to `git ls-files` — it breaks only in worktree sessions, which is the worst
# way for it to break.
set -eu

RS_MAX=1500
CPP_MAX=1200

# Permanently exempt — no cap at all.
exempt() {
	case "$1" in
	# cxx-qt permits exactly one `#[cxx_qt::bridge] mod ffi` per crate, so the
	# bridge module cannot be split across files. ~3057 lines on arrival.
	crates/ui-shell/src/bridge/ffi.rs) return 0 ;;
	esac
	return 1
}

# Grandfathered files: ratcheted baselines. A listed file may shrink, never
# grow. Lower or delete the entry in the same commit that shrinks the file.
#
# Measure these against the tip of main at the moment the change MERGES, not
# when the branch was cut. The first version of this gate was measured on a
# main that two open pull requests then landed on top of, so it turned main
# red the moment it merged — the numbers were correct when written and stale
# by the time they were enforced.
baseline() {
	case "$1" in
	crates/index-core/src/lib.rs) echo 3995 ;;         # ratcheted down: replace_in_files/preview_replacements moved to replace_preview.rs (F3-15)
	# Raised from 2572 by 10 lines for `Scope::from_id` (C9) — the inverse of
	# `Scope::id`, needed to rebuild a `Scope` from the raw id a
	# `FfiHighlightSpan` carries back across the seam when overlaying
	# semantic-token spans onto tree-sitter ones (`ui-shell`'s
	# `overlay_semantic_tokens`). No split planned otherwise.
	crates/syntax-core/src/lib.rs) echo 2582 ;;
	crates/mcp-server/src/lib.rs) echo 1836 ;;         # no split planned; ratcheted so it cannot grow
	crates/ai-chat-core/src/context.rs) echo 1608 ;;   # no split planned; ratcheted so it cannot grow
	# Raised from 1553 by 16 lines for the ResourceOp error variant, its FFI
	# code and the file_ops module declaration — the parts that must live
	# beside AppError. The operation itself, its 12 tests and its Display
	# impl went to app-core/src/file_ops.rs rather than in here.
	# Raised from 1569 by 1 line for the tree_sort module declaration; the
	# two sort-order methods themselves live in app-core/src/tree_sort.rs.
	# Raised from 1570 by 2 lines for the preview module declaration and its
	# doc comment; PreviewService itself lives in app-core/src/preview.rs.
	# Raised from 1572 by 22 lines for TabKind::Diff/TabContent::Diff and the
	# diff_tab module declaration — the enum variants and match arms that
	# must live beside TabKind/TabContent themselves (F3-14). DiffContent,
	# AppSession's open_diff_tab/diff_labels/diff_texts/diff_hunks and their
	# tests all went to app-core/src/diff_tab.rs rather than in here.
	# Raised from 1594 by 1 line for the project_open module declaration
	# (ADR-0037); install_opened_project/install_rebuilt_tree and their tests
	# live in app-core/src/project_open.rs rather than in here.
	# Raised from 1595 by 2 lines for the text_search module declaration and
	# its doc comment (issue #233); matches_query itself lives in
	# app-core/src/text_search.rs rather than in here.
	crates/app-core/src/lib.rs) echo 1597 ;;           # no split planned; ratcheted so it cannot grow
	# 1442 -> 2052 across the C1-C12 csharp-ls chain: registerCapability
	# (C4), didChangeWatchedFiles (C5), workspace/configuration (C6),
	# completionItem/resolve (C7), semantic tokens (C9), code lens (C10)
	# and call/type hierarchy (C11) each added a request method and a
	# dispatch arm here. A split into per-feature request modules is a
	# real follow-up (tracked separately), not attempted in this chain.
	crates/lsp-core/src/manager.rs) echo 2052 ;;
	# 1363 -> 2328 over the same chain: one integration test module per
	# stub_server mode added by C4/C6/C7/C9/C10/C11. Splitting by feature
	# is the obvious fix; deferred as a follow-up alongside manager.rs.
	crates/lsp-core/tests/stub_server_session.rs) echo 2328 ;;
	# 1371 -> 1686: the bridge-side call sites for the same feature chain
	# (C5 watched files, C7 completion resolve, C9-C11 FFI methods).
	crates/ui-shell/src/bridge/language/mod.rs) echo 1686 ;;
	# Raised from the 1500 ceiling by 5 lines for minimapOptions/
	# saveMinimapOptions (issue #199) — the same load/mutate/save pair
	# whitespaceOptions/saveWhitespaceOptions just above them already uses.
	# No split planned.
	crates/ui-shell/src/bridge/settings.rs) echo 1505 ;;
	# Raised from the 1200 ceiling by 11 lines for the PHP tooling plan's
	# B9: constructing AnalysisEditor/AnalysisService alongside the other
	# per-window settings-page editors and services, two new
	# SettingsContext fields, wiring AnalysisService into buildStatusBar,
	# and the one-line call to buildAnalysisMenu (its own translation
	# unit, like buildBuildMenu/buildRunMenu). No split planned.
	# Raised from 1211 by 6 lines for the PHP tooling plan's D5/D6:
	# constructing TestService alongside the other per-window services,
	# threading it through buildCentralWidget to build the Tests dock, and
	# the one-line call to buildTestsMenu (its own translation unit, like
	# buildBuildMenu/buildAnalysisMenu). No split planned.
	# Raised from 1217 by 3 lines for the PHP tooling plan's E2: threading
	# `analysisService` through `buildCentralWidget` into
	# `wireDiagnosticsService`/`ProblemsPanel` too, so an analyzer's rows
	# reach the editor's squiggles and the Problems dock the same way a
	# build's or a language server's already did (ADR-0046's third-source
	# gap this closes). No split planned.
	# Raised from 1220 by 8 lines for the Git history UI revamp
	# (commit-detail dock, repo-wide Commit Log panel, File History's
	# buildFileHistoryDock retrofit): each new dock is one
	# `buildXDock(dockManager, docks, bottomArea, ...)` call plus its
	# `#include`, following the existing
	# buildCommitLogDock/buildCommitDetailDock/buildFileHistoryDock
	# pattern — the construction logic itself already lives in each
	# panel's own .cpp. Further splitting main_window.cpp itself was
	# already investigated and ruled out (see the extraction-wall note
	# from the main_window.cpp split down to 1175 lines); no further
	# split planned here.
	# Raised from 1228 by 2 lines for i18n startup (ADR-0049): one
	# #include and one installUiTranslators(appSettings, app) call in
	# run_app() — the translator-load logic itself lives in the new
	# i18n_startup.cpp, following the same split-out-a-.cpp pattern as
	# status_bar.cpp/navigate_menu.cpp/ai_menu.cpp above it.
	crates/ui-shell/cpp/main_window.cpp) echo 1230 ;;
	# Raised from 1446 by 91 lines for issue #164's regression test: a
	# second-commit git fixture, opening File History via Find Action, and
	# driving the fixed context-menu interaction end to end. Ratcheted down
	# from 1537 when the run flows moved to e2e_run.rs (R1-9), then up by 13
	# for the comment explaining why the file-history flow matches on a path
	# suffix (D3-9). Raised again for issue #143's regression test: switches
	# the Settings dialog to project scope, edits the tab width, and checks
	# only that field reached .ide/settings.toml — the flow that found and
	# proves the fix for the origin-badge bug #143 walked into. No split
	# planned; this suite is already one flow per test.
	# Ratcheted down: the split-pane flows moved to their own binary,
	# crates/app/tests/e2e_panes.rs, to make room for the tab-drag flow.
	# Raised from 1362 by 35 lines for e2e_ui_locale_setting_takes_effect_on_relaunch
	# (ADR-0049): the ui_locale_active marker on a cold launch and after a
	# relaunch seeded with ui_locale = "de".
	crates/app/tests/e2e.rs) echo 1397 ;;
	esac
}

# The loop runs in a subshell (it is the right-hand side of a pipe), so its
# `failed` cannot escape — the subshell exits with it instead, and that exit
# status is the pipeline's, which is what the `if` below tests.
if find crates -type f \( -name '*.rs' -o -name '*.cpp' -o -name '*.h' \) -not -path '*/target/*' | sort | {
	failed=0
	while IFS= read -r file; do
		exempt "$file" && continue

		case "$file" in
		*.rs) max=$RS_MAX ;;
		*) max=$CPP_MAX ;;
		esac

		lines=$(wc -l <"$file")
		base=$(baseline "$file")

		if [ -n "$base" ]; then
			[ "$lines" -le "$base" ] && continue
			echo "FAIL $file: $lines lines, grandfathered baseline $base."
			echo "     A baselined file may only shrink toward the $max-line ceiling."
		else
			[ "$lines" -le "$max" ] && continue
			echo "FAIL $file: $lines lines, ceiling $max."
			echo "     Split it into focused modules, or justify a baseline in this script."
		fi
		failed=1
	done
	[ "$failed" -eq 0 ]
}; then
	echo "file-size gate: ok"
else
	echo "file-size gate failed"
	exit 1
fi
