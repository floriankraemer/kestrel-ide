DOCKER ?= docker
DOCKERFILE := docker/Dockerfile
LINUX_IMAGE := ide-linux-builder
LSP_IMAGE := ide-lsp-conformance
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

.PHONY: help all test lint coverage coverage-ci e2e e2e-ci e2e-repeat build build-linux build-windows linux-image shell clean \
	lsp-image lsp-conformance lsp-conformance-ci

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

lint: linux-image ## Run clippy + rustfmt + file-size checks in Docker
	$(RUN_LINUX) cargo clippy --workspace --all-targets -- -D warnings
	$(RUN_LINUX) cargo fmt --all -- --check
	$(RUN_LINUX) scripts/check-file-size.sh

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
	$(E2E_XVFB) cargo test -p app --test e2e --test e2e_run --test e2e_panes --test e2e_preview --test e2e_vcs --test e2e_minimap --test e2e_about --test e2e_diff --test e2e_analysis -- --ignored --test-threads=1 --nocapture

# Burn-in: `make e2e-repeat TEST=e2e_open_project_edit_save N=20`. A flake is
# a P1 bug in the product or the harness, so this exists to find one before
# it is discovered by somebody re-running CI.
N ?= 20
e2e-repeat: linux-image ## Repeat one E2E flow N times: make e2e-repeat TEST=<name> N=20
	@test -n "$(TEST)" || { echo "usage: make e2e-repeat TEST=<name> [N=20]"; exit 2; }
	$(RUN_LINUX) sh -c 'cargo build -p app && cargo build --bin stub_server -p lsp-core && \
		cargo build --bin stub_analyzer -p analysis-core && \
		for i in $$(seq 1 $(N)); do \
		echo "--- run $$i/$(N) ---"; \
		$(E2E_XVFB) cargo test -p app --test e2e --test e2e_run --test e2e_panes --test e2e_preview --test e2e_vcs --test e2e_minimap --test e2e_about --test e2e_diff --test e2e_analysis -- --ignored --exact \
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
