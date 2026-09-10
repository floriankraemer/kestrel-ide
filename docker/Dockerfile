# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# base: shared toolchain layer for all target-specific builder stages.
# Pinned Rust via rustup (no distro rustc) + common build tools.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS base

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    RUST_VERSION=1.98.1

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        build-essential \
        pkg-config \
        cmake \
        ninja-build \
        git \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "${RUST_VERSION}" \
        --component clippy,rustfmt,llvm-tools-preview \
    && rustc --version && cargo --version

WORKDIR /workspace

# ---------------------------------------------------------------------------
# linux-builder: base + Qt6 dev packages needed to build/test/run the
# workspace (incl. ui-shell's cxx-qt Qt6 Widgets bridge) natively on Linux.
# ---------------------------------------------------------------------------
FROM base AS linux-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        ccache \
        mold \
        qt6-base-dev \
        qt6-base-dev-tools \
        qt6-base-private-dev \
        qt6-l10n-tools \
        libqt6core5compat6-dev \
        libgl1-mesa-dev \
        libxcb1-dev \
        xvfb \
        xauth \
        x11-apps \
        imagemagick \
        xdotool \
        python3-debugpy \
    && rm -rf /var/lib/apt/lists/*

# `python3-debugpy` is the one debug adapter the E2E suite needs (D3-9).
# Debugging is the feature most likely to be broken by a change nobody
# noticed, and the only way to see it work is to run a real adapter — the
# same argument that put `xdotool` and `xvfb` in this list.

ENV QMAKE=/usr/bin/qmake6

# cc-rs recognises ccache as the leading token of CC/CXX and keeps it as the
# launcher. build.rs compiles the whole vendored docking system through cc,
# which has no object cache of its own, so without this every rerun of that
# build script recompiles all ~213 translation units from scratch.
# CCACHE_DIR is a plain path so the Makefile can mount a named volume there
# and the cache survives `docker run --rm`.
#
# RUSTC_WRAPPER=sccache covers the other half: cargo's own incremental
# compilation already skips unchanged crates in a warm `target/` (bind-mounted
# by the Makefile, so it survives `--rm` too), but a fresh worktree or a
# `cargo clean` still recompiles every crate from source with nothing to
# fall back on. sccache is a compilation cache keyed on source+flags, so it
# still hits in that cold case as long as some other checkout built the same
# crate/version/flags combination before. SCCACHE_DIR is a plain path for the
# same named-volume-survives-`--rm` reason as CCACHE_DIR above.
ENV CC="ccache gcc" \
    CXX="ccache g++" \
    CCACHE_DIR=/ccache \
    RUSTC_WRAPPER=sccache \
    SCCACHE_DIR=/sccache

# Pinned binary releases, not `cargo install`: the latter would compile both
# tools from source on every image rebuild, which is exactly the slow cold
# build these tools exist to avoid.
ARG NEXTEST_VERSION=0.9.143
RUN curl -fsSL -o /tmp/nextest.tar.gz \
      "https://get.nexte.st/${NEXTEST_VERSION}/linux" \
    && tar -xzf /tmp/nextest.tar.gz -C /usr/local/cargo/bin cargo-nextest \
    && rm /tmp/nextest.tar.gz \
    && cargo nextest --version

# cargo-llvm-cov drives the source-based coverage instrumentation rustc
# already ships; `llvm-tools-preview` in the base stage supplies the
# llvm-profdata/llvm-cov binaries it shells out to.
ARG LLVM_COV_VERSION=0.9.0
RUN curl -fsSL -o /tmp/llvm-cov.tar.gz \
      "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v${LLVM_COV_VERSION}/cargo-llvm-cov-x86_64-unknown-linux-gnu.tar.gz" \
    && tar -xzf /tmp/llvm-cov.tar.gz -C /usr/local/cargo/bin cargo-llvm-cov \
    && rm /tmp/llvm-cov.tar.gz \
    && cargo llvm-cov --version

ARG SCCACHE_VERSION=0.17.0
RUN curl -fsSL -o /tmp/sccache.tar.gz \
      "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    && tar -xzf /tmp/sccache.tar.gz -C /tmp \
    && mv "/tmp/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl/sccache" /usr/local/cargo/bin/sccache \
    && chmod +x /usr/local/cargo/bin/sccache \
    && rm -rf /tmp/sccache.tar.gz "/tmp/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl" \
    && sccache --version

# Every path a build writes to, made writable by whatever uid the Makefile
# runs the container as (`--user $(id -u):$(id -g)`, see `RUN_LINUX`).
#
# Without this the container runs as root, and root is the owner of every file
# it creates on the bind-mounted `/workspace` — so `target/` comes back
# `root:root` on the host and the developer who owns the checkout cannot
# delete it without `sudo`. That is not a theoretical tidiness point: removing
# a finished agent worktree failed with "Permission denied" until its `target/`
# was deleted from inside a container, and 25 GB of dead build output sat on a
# 98%-full disk in the meantime.
#
# `chmod 0777` on the mountpoints rather than `chown` to a fixed uid, and
# `--user` in the Makefile rather than a `USER` line here: the right uid is a
# property of the machine running the build (whoever owns the checkout), not
# of the image, and CI runs this image via `container:` as root with no bind
# mount to spoil. Only the directories themselves are chmodded — never a
# recursive `chown -R` over the crate registry, which would rewrite every file
# into a new layer for nothing.
#
# `/usr/local/cargo` itself is included because cargo takes its `.package-cache`
# lock there, and `git` because a git dependency is checked out under it; both
# sit outside the registry volume.
RUN mkdir -p /ccache /sccache /usr/local/cargo/registry /usr/local/cargo/git \
    && chmod 0777 /ccache /sccache \
        /usr/local/cargo /usr/local/cargo/registry /usr/local/cargo/git

# ---------------------------------------------------------------------------
# lsp-conformance: linux-builder plus real language servers, for the
# conformance suites (crates/lsp-core/tests/real_server_conformance.rs and
# crates/lsp-core/tests/csharp_conformance.rs).
#
# Deliberately NOT folded into linux-builder: that image is pulled by every CI
# job and every developer, and this adds ~40MB and a download for a suite that
# runs nightly. Keeping it a separate stage means the delta is all that is
# pulled, and linux-builder's layer cache is untouched.
#
# rust-analyzer and csharp-ls, pinned. Two servers now, not one: the C#
# language-servers work built client support for dynamic capability
# registration (client/registerCapability) and pulled configuration
# (workspace/configuration), and no real server had ever exercised either
# path — csharp-ls is the first that does. See
# docs/architecture/lsp-conformance.md for the full rationale. rust-analyzer
# remains for the two things it was originally added for: UTF-16 position
# encoding on non-ASCII text, and the shape of a real
# `textDocument/codeAction` reply.
#
# pyright and clangd still disagree with rust-analyzer in useful ways (Node
# runtime, different encoding history, compile_commands.json, heavier use of
# codeAction/resolve), and get added when a feature actually depends on that
# divergence — the same bar csharp-ls had to clear.
#
# Kept as ONE stage rather than split: the Makefile's `lsp-conformance`
# target has no `--target` parameter, both suites run from the same
# `lsp-conformance-ci` invocation, and this image is already only pulled
# nightly/on demand — the extra ~1GB from the .NET SDK is not a per-PR tax on
# anyone.
#
# Every version below is PINNED on purpose. An unpinned language server turns
# a conformance suite into a random number generator, and "upstream changed
# its shape" arriving as a red CI on an unrelated PR is exactly how a suite
# like this gets switched off. Bumping a pin is a deliberate commit.
# ---------------------------------------------------------------------------
FROM linux-builder AS lsp-conformance

ARG RUST_ANALYZER_VERSION=2025-08-11

RUN curl -fsSL -o /tmp/ra.gz \
      "https://github.com/rust-lang/rust-analyzer/releases/download/${RUST_ANALYZER_VERSION}/rust-analyzer-x86_64-unknown-linux-gnu.gz" \
    && gunzip -c /tmp/ra.gz > /usr/local/bin/rust-analyzer \
    && chmod +x /usr/local/bin/rust-analyzer \
    && rm /tmp/ra.gz \
    && rust-analyzer --version

# .NET SDK via Microsoft's own dotnet-install.sh, the same "download a pinned
# release directly" style as rust-analyzer above — no third-party apt repo.
# 10.0 (LTS) rather than 8.0: the pinned csharp-ls below ships a
# `tools/net10.0/any` global-tool payload, and an 8.0 SDK cannot select it —
# `dotnet tool install` fails with a misleading "DotnetToolSettings.xml was
# not found" rather than a clear TFM mismatch. Verified to exist at
# https://dotnet.microsoft.com/download/dotnet/10.0.
ARG DOTNET_SDK_VERSION=10.0.400
ENV DOTNET_ROOT=/usr/local/dotnet
# `dotnet tool install --global` always installs under $HOME/.dotnet/tools
# regardless of DOTNET_ROOT — this stage runs as root, so that is
# /root/.dotnet/tools.
ENV PATH="${DOTNET_ROOT}:/root/.dotnet/tools:${PATH}"
# dotnet-install.sh needs libicu for globalization; linux-builder is Debian
# bookworm and does not carry it.
RUN apt-get update && apt-get install -y --no-install-recommends libicu72 \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/dotnet-install.sh https://dot.net/v1/dotnet-install.sh \
    && chmod +x /tmp/dotnet-install.sh \
    && /tmp/dotnet-install.sh --version "${DOTNET_SDK_VERSION}" --install-dir "${DOTNET_ROOT}" \
    && rm /tmp/dotnet-install.sh \
    && dotnet --version

# csharp-ls: Roslyn-based LSP server, installed as a pinned global dotnet
# tool. Verified to exist on nuget.org (api.nuget.org/v3-flatcontainer/
# csharp-ls/index.json) before pinning.
ARG CSHARP_LS_VERSION=0.27.0
RUN dotnet tool install --global csharp-ls --version "${CSHARP_LS_VERSION}" \
    && csharp-ls --version

# ---------------------------------------------------------------------------
# linux-artifact-builder: linux-builder + baked-in source, for buildx
# artifact-extraction builds. Kept separate from linux-builder (used as the
# dev/test image via `docker run -v $(pwd):/workspace`) so that stage's
# layer cache isn't invalidated by every source change.
# .dockerignore keeps target/, dist/, .git/ out of the COPY.
# ---------------------------------------------------------------------------
FROM linux-builder AS linux-artifact-builder
COPY . /workspace
RUN cargo build --release -p app

# ---------------------------------------------------------------------------
# linux-artifact-bundle: the release binary plus the shared-library closure
# it needs, so the tarball runs on a machine with no Qt6 installed. Copies
# every ldd dependency except the ones that must come from the host: glibc
# and its companions (a bundled libc against a different loader breaks) and
# libdrm/libgbm, which the host's GPU driver stack owns. libEGL/libGLX are
# bundled — on Debian those are the vendor-neutral libglvnd dispatchers,
# which find the real driver through the host's vendor config at runtime —
# and so are the xcb libraries, which are protocol clients rather than
# drivers and are the ones most often missing on a minimal system. Qt loads its platform plugin
# dynamically (not via the import table), so libqxcb.so is copied in
# explicitly and fed through the same closure walk.
# ---------------------------------------------------------------------------
FROM linux-artifact-builder AS linux-artifact-bundle

RUN set -eux; \
    mkdir -p /out/bin /out/lib /out/plugins/platforms; \
    cp /workspace/target/release/app /out/bin/ide; \
    QT_PLUGINS=/usr/lib/x86_64-linux-gnu/qt6/plugins; \
    cp -L $QT_PLUGINS/platforms/libqxcb.so /out/plugins/platforms/; \
    cp -rL $QT_PLUGINS/imageformats /out/plugins/; \
    host_lib='^(ld-linux.*|libc|libm|libdl|libpthread|librt|libresolv|libnsl|libutil|libanl|libdrm|libgbm)\.'; \
    while :; do \
        before=$(ls /out/lib | wc -l); \
        for obj in /out/bin/ide $(find /out/lib /out/plugins -name '*.so*'); do \
            ldd "$obj" | awk '/=> \//{print $1 " " $3}'; \
        done | sort -u | while read -r soname path; do \
            if echo "$soname" | grep -Eq "$host_lib"; then continue; fi; \
            if [ ! -e "/out/lib/$soname" ]; then cp -L "$path" "/out/lib/$soname"; fi; \
        done; \
        if [ "$before" = "$(ls /out/lib | wc -l)" ]; then break; fi; \
    done; \
    printf '%s\n' \
        '#!/bin/sh' \
        '# Launcher: point the dynamic loader and Qt at the bundled copies.' \
        'here=$(dirname "$(readlink -f "$0")")' \
        'export LD_LIBRARY_PATH="$here/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"' \
        'export QT_PLUGIN_PATH="$here/plugins"' \
        'exec "$here/bin/ide" "$@"' \
        > /out/ide; \
    chmod +x /out/ide

# ---------------------------------------------------------------------------
# linux-artifact: final stage whose entire filesystem is just the bundle, so
# `docker buildx build --output type=local,dest=dist/` exports only that,
# landing at dist/ide-linux-x86_64/ with ./ide as the entry point.
# ---------------------------------------------------------------------------
FROM scratch AS linux-artifact
COPY --from=linux-artifact-bundle /out/ /ide-linux-x86_64/

# ---------------------------------------------------------------------------
# mxe-base: MXE's mingw-w64 + Qt6 cross toolchain, built from a pinned
# upstream commit into /opt/mxe.
#
# This replaces a hand-built, out-of-band `mxe-spike-snapshot:2` image that
# existed only on one developer's machine and was lost to a `docker prune`.
# Building it here costs several hours once, then lives in the layer cache.
#
# Only `qt6-qtbase` is built (Core/Gui/Widgets + the qwindows platform
# plugin) because that is all `crates/ui-shell/build.rs` asks for; the `qt6`
# meta-package would additionally compile multimedia, webview, declarative
# and friends for no gain.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS mxe-base

ENV DEBIAN_FRONTEND=noninteractive

# Bump deliberately. The bundled DLL list in windows-artifact-bundle pins ICU
# sonames (icudt78 etc.) that track MXE's icu4c version, so re-check that
# closure whenever this moves.
ARG MXE_COMMIT=fb9a1a22906e2b0ba37602c562b3335c91a73b13

# Package list per https://mxe.cc/#requirements (Debian/Ubuntu), plus
# python3-yaml, which mesa's meson build requires but the MXE list omits.
RUN apt-get update && apt-get install -y --no-install-recommends \
        autoconf \
        automake \
        autopoint \
        bash \
        bison \
        bzip2 \
        ca-certificates \
        flex \
        g++ \
        g++-multilib \
        gettext \
        git \
        gperf \
        intltool \
        libc6-dev-i386 \
        libclang-dev \
        libgdk-pixbuf2.0-dev \
        libgl-dev \
        libltdl-dev \
        libpcre2-dev \
        libssl-dev \
        libtool-bin \
        libxml-parser-perl \
        lzip \
        make \
        openssl \
        p7zip-full \
        patch \
        perl \
        python3 \
        python3-mako \
        python3-packaging \
        python3-pkg-resources \
        python3-setuptools \
        python3-yaml \
        python-is-python3 \
        ruby \
        sed \
        sqlite3 \
        unzip \
        wget \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN git clone https://github.com/mxe/mxe.git /opt/mxe \
    && git -C /opt/mxe checkout "${MXE_COMMIT}"

# MXE_QT6_ID defaults to `qt6`, so this installs into
# /opt/mxe/usr/x86_64-w64-mingw32.shared/qt6 — the prefix windows-builder and
# windows-artifact-bundle below reference.
# The cache mount holds MXE's downloaded source tarballs (~GBs). It keeps a
# retried build from re-downloading everything, and — since cache mounts are
# not part of the layer — keeps them out of the image for free.
RUN --mount=type=cache,target=/opt/mxe/pkg,sharing=locked \
    --mount=type=cache,target=/root/.ccache,sharing=locked \
    make -C /opt/mxe qt6-qtbase qt6-qttools \
        MXE_TARGETS=x86_64-w64-mingw32.shared \
        JOBS="$(nproc)"

# ---------------------------------------------------------------------------
# windows-builder: mxe-base plus rustup, for cross-compiling to
# x86_64-pc-windows-gnu.
# ---------------------------------------------------------------------------
FROM mxe-base AS windows-builder

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/opt/mxe/usr/bin:$PATH \
    RUST_VERSION=1.98.1

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "${RUST_VERSION}" \
    && rustc --version && cargo --version

RUN rustup target add x86_64-pc-windows-gnu

# rustc's windows-gnu target invokes plain-named `x86_64-w64-mingw32-dlltool`
# (and friends) regardless of the configured linker; MXE only ships the
# `.shared`-suffixed triplet, so alias the plain names it expects.
RUN for t in gcc g++ ar windres ranlib strip ld nm objcopy; do \
        ln -sf /opt/mxe/usr/bin/x86_64-w64-mingw32.shared-$t /usr/local/bin/x86_64-w64-mingw32-$t; \
    done \
    # dlltool defaults to invoking plain `as` (the host assembler) instead of
    # its own cross prefix, producing host relocations; force it to the
    # correct cross assembler via -S.
    && printf '%s\n' '#!/bin/sh' \
        'exec /opt/mxe/usr/bin/x86_64-w64-mingw32.shared-dlltool -S /opt/mxe/usr/bin/x86_64-w64-mingw32.shared-as "$@"' \
        > /usr/local/bin/x86_64-w64-mingw32-dlltool \
    && chmod +x /usr/local/bin/x86_64-w64-mingw32-dlltool \
    # cxx-qt injects --whole-archive "call-initializer" shim archives after
    # the rlibs that define the symbols they reference (libui_shell.rlib
    # defines cxx_qt_init_crate_ui_shell, consumed by a later --whole-archive
    # shim); MXE's ld.bfd is a strict single-pass linker over the archive
    # list and won't revisit libui_shell.rlib once past it. Tried swapping
    # in lld (Debian's ld.lld, name-compatible drop-in for this cross gcc,
    # which resolves ordering via -fuse-ld or by replacing the resolved `ld`
    # binary directly) — that fixed the ordering issue but lld's COFF driver
    # then hard-errors on the `-exclude-symbols:` .drectve directives rustc
    # emits for the windows-gnu target, which is GNU-ld-specific syntax lld
    # doesn't implement. So instead: keep ld.bfd, and wrap the whole static
    # section (from -Bstatic to -Bdynamic) in --start-group/--end-group so
    # bfd resolves back-references within it in a single group.
    && printf '%s\n' '#!/bin/bash' \
        'out=()' \
        'for a in "$@"; do' \
        '  if [ "$a" = "-Wl,-Bstatic" ]; then out+=("$a" "-Wl,--start-group");' \
        '  elif [ "$a" = "-Wl,-Bdynamic" ]; then out+=("-Wl,--end-group" "$a");' \
        '  else out+=("$a"); fi' \
        'done' \
        'exec x86_64-w64-mingw32.shared-gcc "${out[@]}"' \
        > /usr/local/bin/x86_64-w64-mingw32-windows-gnu-cc \
    && chmod +x /usr/local/bin/x86_64-w64-mingw32-windows-gnu-cc \
    # rustc's windows-gnu target unconditionally links `-l:libpthread.a`
    # (posix-threads mingw layout); MXE's `.shared` build only ships the
    # winpthreads shared-lib import archive under that pthread alias
    # (libpthread.dll.a), so alias it to the static-looking name the
    # linker asks for by exact filename.
    && ln -sf libpthread.dll.a /opt/mxe/usr/x86_64-w64-mingw32.shared/lib/libpthread.a

ENV MXE_TARGET=x86_64-w64-mingw32.shared \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32.shared-gcc \
    CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32.shared-g++ \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32.shared-ar \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-windows-gnu-cc \
    QMAKE=/opt/mxe/usr/x86_64-w64-mingw32.shared/qt6/bin/qmake \
    CMAKE_PREFIX_PATH=/opt/mxe/usr/x86_64-w64-mingw32.shared/qt6/lib/cmake \
    PKG_CONFIG_ALLOW_CROSS=1

WORKDIR /workspace

# ---------------------------------------------------------------------------
# windows-artifact-builder: windows-builder + baked-in source (same
# separation rationale as linux-artifact-builder).
# ---------------------------------------------------------------------------
FROM windows-builder AS windows-artifact-builder
COPY . /workspace
RUN cargo build --release --target x86_64-pc-windows-gnu -p app

# ---------------------------------------------------------------------------
# windows-artifact: final stage whose entire filesystem is the release .exe
# plus its Qt6/mingw runtime DLL closure (dynamic Qt6 link, not static), so
# `docker buildx build --output type=local,dest=dist/` exports a
# self-contained dist/windows/ folder. DLL set determined by walking the PE
# import table (objdump -p) of the exe and each bundled DLL transitively —
# see docker/Dockerfile git history / task notes for the closure derivation.
# The qwindows.dll platform plugin isn't in the exe's import table (Qt loads
# it dynamically at runtime via QPluginLoader) but is required for any Qt
# Widgets app to start on Windows, so it's included under platforms/, which
# is Qt's mandated lookup path next to the executable.
# ---------------------------------------------------------------------------
FROM windows-artifact-builder AS windows-artifact-bundle
RUN mkdir -p /out/platforms \
    && cp /workspace/target/x86_64-pc-windows-gnu/release/app.exe /out/ide-windows-x86_64.exe \
    && MINGW_BIN=/opt/mxe/usr/x86_64-w64-mingw32.shared/bin \
    && QT_BIN=/opt/mxe/usr/x86_64-w64-mingw32.shared/qt6/bin \
    && cp \
        $QT_BIN/Qt6Core.dll \
        $QT_BIN/Qt6Gui.dll \
        $QT_BIN/Qt6Widgets.dll \
        $MINGW_BIN/libstdc++-6.dll \
        $MINGW_BIN/libwinpthread-1.dll \
        $MINGW_BIN/libgcc_s_seh-1.dll \
        $MINGW_BIN/icudt78.dll \
        $MINGW_BIN/icuin78.dll \
        $MINGW_BIN/icuuc78.dll \
        $MINGW_BIN/libpcre2-16-0.dll \
        $MINGW_BIN/libpcre2-8-0.dll \
        $MINGW_BIN/libzstd.dll \
        $MINGW_BIN/zlib1.dll \
        $MINGW_BIN/libfreetype-6.dll \
        $MINGW_BIN/libharfbuzz-0.dll \
        $MINGW_BIN/libpng16-16.dll \
        $MINGW_BIN/libbrotlidec.dll \
        $MINGW_BIN/libbrotlicommon.dll \
        $MINGW_BIN/libbz2.dll \
        $MINGW_BIN/libglib-2.0-0.dll \
        $MINGW_BIN/libintl-8.dll \
        $MINGW_BIN/libiconv-2.dll \
        /out/ \
    && cp /opt/mxe/usr/x86_64-w64-mingw32.shared/qt6/plugins/platforms/qwindows.dll /out/platforms/

FROM scratch AS windows-artifact
COPY --from=windows-artifact-bundle /out/ /windows/
