#!/bin/sh
# Test-layout gate: each crate's integration tests link into one binary, not
# one per file (see the "one integration-test binary per crate" plan). A
# crate with more than one top-level `tests/*.rs` file is paying a full
# static link of its dependency tree per extra file.
#
# Discovered with `find`, not `git ls-files`, on purpose: this repo is worked
# on in git worktrees, where `.git` is a *file* pointing outside the Docker
# mount (the Makefile's RUN_LINUX mounts only $(CURDIR) at /workspace), so
# any git invocation inside the container fails. Do not "improve" this back
# to `git ls-files` — it breaks only in worktree sessions, which is the worst
# way for it to break.
set -eu

# Exempt, with the reason kept here rather than left to a commit message.
exempt() {
	case "$1" in
	# vcs-core: `wsl_git_e2e.rs` calls `std::env::set_var("PATH", ...)` behind
	# a lock that only protects its own binary. Merged into another file's
	# binary, `timings.rs` would run `git` in the same process, and CI runs
	# `cargo test` threaded (ci.yml), so that would be a real race. See the
	# "one integration-test binary per crate" plan.
	crates/vcs-core) return 0 ;;
	esac
	return 1
}

if find crates -mindepth 2 -maxdepth 2 -type d -name tests | sort | {
	failed=0
	while IFS= read -r dir; do
		crate=$(dirname "$dir")
		exempt "$crate" && continue

		count=$(find "$dir" -maxdepth 1 -type f -name '*.rs' | wc -l)
		[ "$count" -le 1 ] && continue

		echo "FAIL $dir: $count top-level *.rs files."
		echo "     Add each as a module of tests/<target>/main.rs instead of its own top-level file."
		failed=1
	done
	[ "$failed" -eq 0 ]
}; then
	echo "test-layout gate: ok"
else
	echo "test-layout gate failed"
	exit 1
fi
