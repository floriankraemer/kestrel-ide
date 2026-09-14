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

## What it verifies (once the plan's A4/A5 land)

The plan's A4 and A5 tasks add fixture projects under `crates/jvm-build-core/tests/fixtures/`: `gradle-single`, `gradle-multi-kts`, `gradle-catalog`, `maven-single`, `maven-multi`.
The `jvm-integration` tests run the real, system (wrapper-less) `gradle`/`mvn` binaries this image ships against those fixtures — the init script's model output, the TeamCity test stream, Maven's effective-pom and dependency-tree text — and compare the *shape* of what comes back against what the unit-test fixtures assume, the same "the report is executable" property `lsp-conformance.md` describes for the LSP suite.

The `linux-jvm` stage's fixture-prewarm step (building the fixture projects once at image-build time so their JUnit 5 dependency resolution is cached for `--offline` use) is added in the same change that adds the fixtures — there is nothing to prewarm before then.

**Status as of this PR (P0+A1-A7):** the `linux-jvm` image and `make test-jvm`/`jvm-ci` targets exist and the image builds; `jvm-build-core` itself has no `jvm-integration`-feature tests yet, because the Gradle/Maven providers and their fixtures (A4/A5) are a later PR.
`make test-jvm` at this point in the plan runs an (empty) `cargo nextest run -p jvm-build-core --features jvm-integration` successfully rather than nothing.

## Why every version is pinned

An unpinned JDK, Gradle or Maven turns this suite into a random number generator: "upstream changed its output shape" arriving as a red build on an unrelated PR is exactly how a suite like this stops being trusted.
Bumping a pin is a deliberate commit, the same rule `lsp-conformance.md` states for `rust-analyzer`/`csharp-ls`.
