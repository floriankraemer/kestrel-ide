# Contributing

Thanks for taking the time to contribute.
This document covers the practical rules for getting a change merged.

## Ground rules

- Read `CLAUDE.md` and `docs/architecture/layering.md` before structural work.
Both are the authority on how this codebase is organized and why.
- Read `docs/architecture/overview.md` and `docs/architecture/project-structure.md` for orientation before your first PR.
- Follow the layering rules: Qt-free crates must never depend on cxx-qt/Qt, and `cpp/` stays a humble view with no business logic.
- Every new rule or behavior needs unit tests in the Qt-free crate it lives in.
- Structural changes (new crate, moved responsibility, new dependency) require updating `docs/architecture/layering.md` and adding or updating an ADR.

## AI-assisted contributions

AI-assisted and AI-generated contributions are welcome.
The tool you used to write the code is not the point — the result is.

That means:

- You are responsible for every line you submit, regardless of how it was produced.
Review it yourself before opening the PR; don't submit output you haven't read and understood.
- AI-assisted PRs go through the exact same quality gates as everything else.
There is no separate, lighter bar.
- If AI-generated tests are passing but don't actually exercise the behavior they claim to, that's a bug in the PR, not an exception to it.
- Do not paste secrets, credentials, or proprietary third-party code into a PR because a tool suggested it.

## Quality gates

**Only green PRs are accepted.**
A PR that fails CI, fails review, or has known-flaky tests will not be merged, no matter who or what authored it.

Before opening a PR, run the same checks CI runs:

```sh
make lint    # clippy -D warnings + rustfmt --check
make test    # cargo nextest run --workspace + doctests
```

Both commands run inside the project's Docker builder image — see "Development environment" below.
Do not skip this step locally and hope CI catches it; fix issues before pushing.

Requirements for merge:

- `make lint` passes with zero warnings.
- `make test` passes for the full workspace.
- New behavior has unit tests in the correct (Qt-free) crate.
- Patch coverage (the share of lines your PR adds that a test executes) is at least 80%, as reported by CI.
- No `cargo tree -p <qt-free-crate> -e normal | grep -i qt` hits for `editor-core`, `project-model`, `app-core`, or any other Qt-free crate.
- Commit messages follow Conventional Commits (see below).

If a check is failing for a reason unrelated to your change, say so in the PR description — don't silently work around it or disable it.

## Development environment

Always build, test, and run the app inside the Docker builder image — never against the bare host toolchain.

```sh
make linux-image     # build/refresh the builder image
make test             # full workspace test suite
make lint             # full workspace lint
```

For faster iteration on a single crate:

```sh
make shell
cargo check -p <crate>
cargo test -p <crate>
```

Run the full `make test` / `make lint` gate before opening or updating a PR.

## Commit messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): short description

optional body

optional footer(s)
```

Common types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`. Breaking changes use `!` after the type/scope or a `BREAKING CHANGE:` footer.

## Opening a pull request

1. Branch off `main`, one branch per task.
2. Keep the PR scoped to one logical change; split unrelated work into separate PRs.
3. Fill in the pull request template completely, especially the test plan.
4. Link the issue it resolves, if any.
5. Make sure `make lint` and `make test` are green before requesting review.

## Reporting bugs and requesting features

Use the issue templates under **New Issue**. Fill in every section — a reproducible bug report or a well-scoped feature request gets triaged and picked up far faster than a one-line description.
