# JVM integration: checking `jvm-build-core` against a real Gradle/Maven

Every unit test in `jvm-build-core` parses a fixture file — a checked-in JSON model, an effective-pom XML document, a verbose dependency-tree text dump — and asserts against it.
That proves the parsers are right about a snapshot of what Gradle or Maven once printed.
It does not prove the init script actually runs on a real Gradle, or that a real `mvn dependency:tree -Dverbose` still looks the way the fixture says it does.

This suite closes that gap, the same way `docs/architecture/lsp-conformance.md` closes it for the LSP client.

## Running it

```sh
make test-jvm
```

It builds the `linux-jvm` Docker stage (`linux-builder` plus pinned Temurin 21, Gradle and Maven binary distributions) and runs `jvm-build-core`'s `jvm-integration`-feature tests: `cargo nextest run -p jvm-build-core --features jvm-integration`.
Those tests are gated behind the `jvm-integration` Cargo feature, not `#[ignore]`, so `cargo test --workspace`/`make test` never builds or runs them — the feature simply is not enabled there.

## Why it is not a per-PR gate

Neither a JDK, Gradle, nor Maven exists in `linux-builder`, and installing all three there would slow every PR's `make test`/`make lint` for work most PRs never touch — exactly the `lsp-conformance` stage's own reasoning for staying a separate image.
Nightly and on demand.

## What it verifies

Fixture projects live under `crates/jvm-build-core/tests/fixtures/`: `gradle-single`, `gradle-multi-kts`, `gradle-catalog`, `maven-single`, `maven-multi`.
`gradle_integration.rs` and `maven_integration.rs`'s `#[cfg(feature = "jvm-integration")]` tests run the real, system (wrapper-less) `gradle`/`mvn` binaries this image ships against those fixtures — the init script's model output and TeamCity test stream, Maven's effective-pom and pinned verbose dependency-tree text — and assert against the *shape* of what comes back, the same "the report is executable" property `lsp-conformance.md` describes for the LSP suite.

The `linux-jvm` stage's fixture-prewarm step copies the five fixtures to a throwaway path at image-build time and runs the exact commands the integration tests run for real (`ideModel`, the `-Pide.teamcity=true` test task, `help:effective-pom`, the pinned `dependency:tree`), so `~/.gradle`/`~/.m2` are warm with precisely the artifacts those tests need before `make test-jvm` ever runs one.

**Status:** 66 tests pass inside `linux-jvm` (59 unit + 4 Gradle + 3 Maven integration), verified by building the image and running `make jvm-ci` directly.

## Why every version is pinned

An unpinned JDK, Gradle or Maven turns this suite into a random number generator: "upstream changed its output shape" arriving as a red build on an unrelated PR is exactly how a suite like this stops being trusted.
Bumping a pin is a deliberate commit, the same rule `lsp-conformance.md` states for `rust-analyzer`/`csharp-ls`.
