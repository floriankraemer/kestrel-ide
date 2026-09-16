# JVM integration: checking `jvm-build-core` against a real Gradle/Maven

Every unit test in `jvm-build-core` parses a fixture file — a checked-in JSON model, an effective-pom XML document, a verbose dependency-tree text dump — and asserts against it.
That proves the parsers are right about a snapshot of what Gradle or Maven once printed.
It does not prove the init script actually runs on a real Gradle, or that a real `mvn dependency:tree -Dverbose` still looks the way the fixture says it does.

This suite closes that gap, the same way `docs/architecture/lsp-conformance.md` closes it for the LSP client.

## Running it

```sh
make test-jvm
```

It builds the `linux-jvm` Docker stage (`linux-builder` plus pinned Temurin 21, Gradle and Maven binary distributions) and runs, in order (`jvm-ci`, the Makefile's inner half of `test-jvm`): `jvm-build-core`'s and `test-core`'s `jvm-integration`-feature tests, then builds `app` and runs the Gradle/Maven E2E flows (`e2e_build_tools.rs`, the jvm-build-tools plan's E2) under Xvfb.
The two `jvm-integration`-feature test runs are gated behind that Cargo feature, not `#[ignore]`, so `cargo test --workspace`/`make test` never builds or runs them — the feature simply is not enabled there; the E2E flows are gated instead at runtime on `IDE_E2E_JVM=1`, which only `make test-jvm`/`make jvm-ci` and the nightly `jvm-integration` CI job set, for the same "no JDK/Gradle/Maven outside this image" reason.

## Why it is not a per-PR gate

Neither a JDK, Gradle, nor Maven exists in `linux-builder`, and installing all three there would slow every PR's `make test`/`make lint` for work most PRs never touch — exactly the `lsp-conformance` stage's own reasoning for staying a separate image.
Nightly and on demand.

## What it verifies

Fixture projects live under `crates/jvm-build-core/tests/fixtures/`: `gradle-single`, `gradle-multi-kts`, `gradle-catalog`, `maven-single`, `maven-multi`.
`gradle_integration.rs` and `maven_integration.rs`'s `#[cfg(feature = "jvm-integration")]` tests run the real, system (wrapper-less) `gradle`/`mvn` binaries this image ships against those fixtures — the init script's model output and TeamCity test stream, Maven's effective-pom and pinned verbose dependency-tree text — and assert against the *shape* of what comes back, the same "the report is executable" property `lsp-conformance.md` describes for the LSP suite.

The `linux-jvm` stage's fixture-prewarm step copies the five fixtures to a throwaway path at image-build time and runs the exact commands the integration tests run for real (`ideModel`, the `-Pide.teamcity=true` test task, `help:effective-pom`, the pinned `dependency:tree`), so the cache is warm with precisely the artifacts those tests need before `make test-jvm` ever runs one.

That cache is **not** `~/.gradle`/`~/.m2`: `GRADLE_USER_HOME` and Maven's `-Dmaven.repo.local` are redirected to a fixed, world-readable-and-writable `/opt/jvm-cache` rather than left `$HOME`-relative.
Two things made that necessary, both real bugs a plain `~/.gradle` cache had: `make test-jvm` runs `RUN_JVM` under the *developer's own uid* (the same non-root `--user`/`--init` treatment `RUN_LINUX` already gives every other gate, so a bind-mounted fixture's stray `build`/`.gradle` never ends up root-owned on the host), which cannot read a cache root wrote into its own `$HOME` during the image build; and a Gradle daemon `chmod 700`s its own registry directory on startup, which a non-root uid cannot do to a directory root still owns even when the mode bits already say `rwx` for everyone — the fix is `docker/Dockerfile` deleting the daemon registry (and Gradle's `.tmp` scratch space) right after prewarming, so the *first* daemon a real run starts creates and owns that one directory itself, while the dependency caches around it stay prewarmed.
The nightly GitHub Actions job runs this same image inside a `container:` with its own (possibly different, possibly ephemeral) `HOME` — the fixed path is exactly what makes the prewarm still apply there rather than only working by accident locally.

`gradle_integration.rs`/`maven_integration.rs`'s own `SyncOptions` still set `offline: false`: the fixtures' JUnit 5 resolution needs a first online pass the same way a real project's first sync would, and nothing here asserts the prewarm made every single artifact those tests touch already cache-resolvable — `--offline` is `jvm_build_core::gradle::sync`/`maven::sync`'s own option or an IDE user's, not something this suite forces on itself. The image build has network access to populate the cache; the nightly job's container step does too, so an occasional cache-miss re-download is a slower run, not a failure.

**Status:** `make test-jvm` passes inside `linux-jvm` — however many unit and integration tests `cargo nextest` currently collects for `jvm-build-core`/`test-core`'s `jvm-integration` feature, plus both Gradle/Maven E2E flows — verified by building the image and running it directly (as the non-root `RUN_JVM` invocation, the same path CI takes). A hardcoded count drifts the moment a test is added; run `make test-jvm` itself for today's number rather than trusting one written down here.

## A caveat in the published image's tag

`.github/workflows/builder-image.yml` tags the published `linux-jvm` image by `hashFiles('docker/Dockerfile')` alone.
A change to a fixture project or to `ide-model.init.gradle` (which the Dockerfile's `COPY` reaches into, but does not itself name) does not change that hash, so the nightly job can reuse an already-published image whose prewarm step ran against an *older* copy of either.
Harmless today: the nightly `jvm-integration` job checks out the current commit and the tests read the fixtures from that checkout, not from anything baked into the image, so a stale prewarm only ever costs a slower (cache-miss, online-fallback) run, never a wrong answer.
Worth revisiting if the prewarm step ever starts mattering for correctness rather than only speed.

## Why every version is pinned

An unpinned JDK, Gradle or Maven turns this suite into a random number generator: "upstream changed its output shape" arriving as a red build on an unrelated PR is exactly how a suite like this stops being trusted.
Bumping a pin is a deliberate commit, the same rule `lsp-conformance.md` states for `rust-analyzer`/`csharp-ls`.
