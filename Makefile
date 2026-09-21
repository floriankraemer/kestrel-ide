DOCKER ?= docker
DOCKERFILE := docker/Dockerfile
LINUX_IMAGE := ide-linux-builder
LSP_IMAGE := ide-lsp-conformance
JVM_IMAGE := ide-linux-jvm
# Named volumes, not bind mounts: the crate registry and the ccache object
# store must outlive `--rm`, and neither belongs in the source tree. Without
# them every container start re-downloads the registry and recompiles every
# C++ translation unit from scratch.
DOCKER_MOUNTS = -v "$(CURDIR)":/workspace -w /workspace \
	-v ide-cargo-registry:/usr/local/cargo/registry \
	-v ide-ccache:/ccache \
	-v ide-sccache:/sccache

# Run as the developer who owns the checkout, not as root. `/workspace` is a
# bind mount, so a root container leaves `target/` owned by `root:root` on the
# host and `rm -rf` (or `git worktree remove`) then fails with "Permission
# denied" until the tree is deleted from inside another container. The image
# chmods the cache mountpoints 0777 for exactly this, so any uid can write
# them; see `docker/Dockerfile`'s linux-builder stage.
#
# HOME is redirected because the image's `/root` is not writable by this uid,
# and tools that expect a home (git config, sccache's server) fail without one.
# CI is unaffected: it runs the `*-ci` inner targets inside a `container:`,
# never through RUN_LINUX.
#
# `--init` gives the container a PID 1 that reaps: without it `xvfb-run` in the
# E2E target never returns after its child exits and the run hangs forever.
DOCKER_USER = --user $(shell id -u):$(shell id -g) -e HOME=/tmp
RUN_LINUX = $(DOCKER) run --rm --init $(DOCKER_USER) $(DOCKER_MOUNTS) $(LINUX_IMAGE)

# Same non-root/--init treatment as RUN_LINUX. Safe now that the linux-jvm
# stage redirects GRADLE_USER_HOME/Maven's local repo to a fixed,
# world-writable /opt/jvm-cache rather than $HOME (root's own $HOME during
# the image's fixture-prewarm build step) — a bind-mounted fixture's stray
# build/.gradle/target directories never end up root-owned on the host.
RUN_JVM = $(DOCKER) run --rm --init $(DOCKER_USER) $(DOCKER_MOUNTS) $(JVM_IMAGE)

.PHONY: help all test lint coverage coverage-ci e2e e2e-ci e2e-repeat build build-linux build-windows linux-image shell clean \
	lsp-image lsp-conformance lsp-conformance-ci linux-jvm-image test-jvm jvm-ci test-db db-ci

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

all: test build ## Run tests, then build all targets

linux-image: ## Build the linux-builder Docker image
	$(DOCKER) build --target linux-builder -t $(LINUX_IMAGE) -f $(DOCKERFILE) .

test: linux-image ## Run cargo nextest + doctests in Docker
	$(RUN_LINUX) cargo nextest run --workspace
	$(RUN_LINUX) cargo test --doc --workspace

# The conformance suite runs against a REAL language server, so it is opt-in:
# it needs its own image, takes minutes, and can go red because upstream
# changed rather than because we did. Nightly and on demand, never per-PR.
lsp-image: ## Build the lsp-conformance image (linux-builder + rust-analyzer + .NET SDK/csharp-ls)
	$(DOCKER) build --target lsp-conformance -t $(LSP_IMAGE) -f $(DOCKERFILE) .

lsp-conformance: lsp-image ## Check the LSP client against real rust-analyzer + csharp-ls servers
	$(DOCKER) run --rm -e CONFORMANCE_BLESS $(DOCKER_MOUNTS) $(LSP_IMAGE) \
		$(MAKE) lsp-conformance-ci

# Inner target: the command line itself, with no Docker wrapper, so CI (which
# is already inside the lsp-conformance image) and `make lsp-conformance` run
# exactly the same thing. Both suites, rust-analyzer's and csharp-ls's, run
# from this one invocation — they are separate #[ignore]d test binaries, not
# a separate target, per the plan's "beside rust-analyzer".
lsp-conformance-ci: ## Inner half of `lsp-conformance` — run inside the image
	cargo test -p lsp-core --test real_server_conformance -- --ignored --nocapture
	cargo test -p lsp-core --test csharp_conformance -- --ignored --nocapture

# jvm-build-core's `jvm-integration`-feature tests spawn a real gradle/mvn
# binary against fixture projects — same "own image, nightly/on demand,
# never per-PR" reasoning as lsp-conformance above (jvm-build-tools plan A2).
linux-jvm-image: ## Build the linux-jvm image (linux-builder + Temurin 21 + Gradle + Maven)
	$(DOCKER) build --target linux-jvm -t $(JVM_IMAGE) -f $(DOCKERFILE) .

test-jvm: linux-jvm-image ## Run the real-toolchain integration tests and Gradle/Maven E2E flows
	$(RUN_JVM) $(MAKE) jvm-ci

# Inner target: the command line itself, with no Docker wrapper, mirroring
# `lsp-conformance-ci`'s split. `e2e_build_tools` (E2, ADR-0057 §6) rides
# along here rather than `e2e-ci`'s per-PR budget — it is gated
# `IDE_E2E_JVM=1` at runtime (`app`'s own test binary has no
# `jvm-integration` feature to gate it at compile time the way the two
# `nextest run` lines above do), so it is a silent no-op skip everywhere
# except here and `make test-jvm`.
jvm-ci: ## Inner half of `test-jvm` — run inside the image
	cargo nextest run -p jvm-build-core --features jvm-integration
	cargo nextest run -p test-core --features jvm-integration
	cargo build -p app
	IDE_E2E_JVM=1 $(E2E_XVFB) cargo test -p app --test e2e_build_tools -- --ignored --test-threads=1 --nocapture

# Database Tools' real-server suite (docs/architecture/db-integration.md).
# F1 lands PostgreSQL only, in `linux-builder` itself (no `linux-db` image
# stage yet — that lands in F8.6 alongside the ODBC client-tools install;
# for now the compose service is reached over `--network host`, the same
# loopback-only posture `docker/db-compose.yml` publishes it under).
test-db: linux-image ## Bring up docker/db-compose.yml and run the real-Postgres integration tests
	docker compose -f docker/db-compose.yml up -d --wait
	$(DOCKER) run --rm --init $(DOCKER_USER) $(DOCKER_MOUNTS) --network host \
		-e IDE_DB_POSTGRES_URL=postgres://ide:ide@127.0.0.1:55432/ide_test \
		$(LINUX_IMAGE) $(MAKE) db-ci; \
		status=$$?; \
		docker compose -f docker/db-compose.yml down -v; \
		exit $$status

db-ci: ## Inner half of `test-db` — run inside the image
	cargo nextest run -p db-drivers --features db-integration

lint: linux-image ## Run clippy + rustfmt + file-size checks in Docker
	$(RUN_LINUX) cargo clippy --workspace --all-targets -- -D warnings
	$(RUN_LINUX) cargo fmt --all -- --check
	$(RUN_LINUX) scripts/check-file-size.sh
	# aws-lc-rs must never enter the tree (R2, database-tools-plan.md §13/§7): every rustls-using crate is audited to keep the `ring` provider only.
	$(RUN_LINUX) sh -c '! cargo tree --workspace --all-features -i aws-lc-rs >/dev/null 2>&1'

# Coverage measures the Qt-free crates only. `ui-shell` is a humble view and
# `app` is a main(); both are untested by design (CLAUDE.md), and folding
# them in would produce a total nobody can act on. `e2e` drives the built
# binary from the outside, so it has no source of its own worth measuring.
COVERAGE_EXCLUDES = --exclude ui-shell --exclude app --exclude e2e

coverage: linux-image ## Coverage for the Qt-free crates; writes target/coverage/lcov.info
	$(RUN_LINUX) $(MAKE) coverage-ci

# Inner target: the command line itself, with no Docker wrapper, so CI (which
# is already inside the linux-builder image) and `make coverage` run exactly
# the same thing.
coverage-ci: ## Inner half of `coverage` — run inside the builder image
	mkdir -p target/coverage
	cargo llvm-cov --workspace $(COVERAGE_EXCLUDES) \
		--lcov --output-path target/coverage/lcov.info
	python3 scripts/coverage-report.py --lcov target/coverage/lcov.info

# One X server with N app instances makes xdotool's window targeting
# ambiguous, and ambiguous input is the first source of E2E flake — hence
# --test-threads=1. xvfb, xauth, x11-apps, imagemagick and xdotool are already
# in linux-builder, so no image change is needed.
E2E_XVFB = xvfb-run -a --server-args="-screen 0 1600x1200x24"

e2e: linux-image ## Run the E2E flows under Xvfb (ignored by `make test`)
	$(RUN_LINUX) $(MAKE) e2e-ci

# Inner target: the command line itself, with no Docker wrapper, so CI (which
# is already inside the linux-builder image) and `make e2e` run exactly the
# same thing.
e2e-ci: ## Inner half of `e2e` — run inside the builder image
	cargo build -p app
	cargo build --bin stub_server -p lsp-core
	cargo build --bin stub_analyzer -p analysis-core
	cargo build --bin stub_engine -p container-core
	$(E2E_XVFB) cargo test -p app --test e2e --test e2e_run --test e2e_panes --test e2e_preview --test e2e_vcs --test e2e_minimap --test e2e_about --test e2e_diff --test e2e_analysis --test e2e_edit --test e2e_editor_popups --test e2e_containers -- --ignored --test-threads=1 --nocapture

# Burn-in: `make e2e-repeat TEST=e2e_open_project_edit_save N=20`. A flake is
# a P1 bug in the product or the harness, so this exists to find one before
# it is discovered by somebody re-running CI.
N ?= 20
e2e-repeat: linux-image ## Repeat one E2E flow N times: make e2e-repeat TEST=<name> N=20
	@test -n "$(TEST)" || { echo "usage: make e2e-repeat TEST=<name> [N=20]"; exit 2; }
	$(RUN_LINUX) sh -c 'cargo build -p app && cargo build --bin stub_server -p lsp-core && \
		cargo build --bin stub_analyzer -p analysis-core && \
		cargo build --bin stub_engine -p container-core && \
		for i in $$(seq 1 $(N)); do \
		echo "--- run $$i/$(N) ---"; \
		$(E2E_XVFB) cargo test -p app --test e2e --test e2e_run --test e2e_panes --test e2e_preview --test e2e_vcs --test e2e_minimap --test e2e_about --test e2e_diff --test e2e_analysis --test e2e_edit --test e2e_editor_popups --test e2e_containers -- --ignored --exact \
			--test-threads=1 --nocapture $(TEST) || exit 1; \
	done'

build: build-linux build-windows ## Build Linux and Windows artifacts

build-linux: ## Export dist/ide-linux-x86_64/ (binary + bundled Qt runtime)
	$(DOCKER) buildx build --target linux-artifact -f $(DOCKERFILE) \
		--output type=local,dest=dist/ .

# First run builds the MXE mingw-w64 + Qt6 cross toolchain (mxe-base stage)
# from source: several hours. Cached as a layer afterwards.
build-windows: ## Export dist/windows/ (first run builds the MXE toolchain, hours)
	$(DOCKER) buildx build --target windows-artifact -f $(DOCKERFILE) \
		--output type=local,dest=dist/ .

shell: linux-image ## Open a bash shell in the builder image
	$(DOCKER) run --rm -it $(DOCKER_MOUNTS) $(LINUX_IMAGE) bash

clean: ## cargo clean + remove dist/
	$(RUN_LINUX) cargo clean
	rm -rf dist
