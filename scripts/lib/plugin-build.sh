# shellcheck shell=bash
# shellcheck disable=SC2154  # ROOT, JOBS and TARGET are build-plugins.sh's
#
# The component build of scripts/build-plugins.sh, split from it (RD-1280-08): make_component,
# wasm_rustflags, build_components and shared_libraries, unchanged. Sourced, never run; expects
# ROOT, JOBS and TARGET set, lib/plugin-stamp.sh sourced (component_path, stamp_path,
# source_files, write_stamp), and the working directory at the checkout root.

# Turns the core module cargo linked for plugin $1 into its component, at the same path.
#
# Written beside it and renamed over it: cargo's file in release/ is a hard link to the one in
# deps/, and writing through it would leave a component where cargo keeps its core module. The
# rename leaves deps/ alone, and the next `cargo rustc` links release/ to the core module again,
# fresh crate or not — which is why every call here follows a `cargo rustc` of the same plugin.
# No adapter and no `--world`: nothing imports WASI, and the world is the one `generate!`
# embedded.
make_component() {
    local module; module="$(component_path "$1")"
    wasm-tools component new "$module" -o "$module.component" \
        || { echo "!! rd-plugin-$1: wasm-tools could not make a component of $module" >&2; exit 1; }
    mv "$module.component" "$module"
}

# The rustflags of every wasm32 build here (RD-1120-10, PL-21): the checkout, the cargo home and
# the toolchain mapped to fixed names. A panic location embeds the source path, so without the
# map a component carried `/home/<user>/.cargo/registry/src/…` and two checkouts of the same
# commit built different bytes. The same disjoint prefixes as the native release targets
# (scripts/release-build-env.sh), set per target so the native builds of the shared target/ keep
# their flags; flags the caller already set for wasm32 come first and stay.
wasm_rustflags() {
    local cargo_home="${CARGO_HOME:-$HOME/.cargo}" rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
    local flags="${CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS:-}"
    flags+="${flags:+ }--remap-path-prefix=$ROOT=/rdownloader"
    flags+=" --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$rustup_home=/rustup"
    printf '%s\n' "$flags"
}

# Builds the components of the plugins named as arguments from THIS checkout, and stamps them.
#
# The plugins' own sources and their shared plugin libraries are touched first. Worktrees share
# one target/, and cargo fingerprints a workspace crate by the file times of its sources rather
# than their contents, so without the touch a build here can find another checkout's newer
# fingerprint, call the crate fresh and leave that checkout's component in place — which the
# stamp would then describe with this checkout's source hash. The touch makes cargo compile from
# here; it costs the staleness check nothing, because that reads contents now. The WIT is left
# alone: rd-plugin-api's host bindings are generated from it, and touching it would rebuild
# half the workspace on the next test run.
#
# The plugin crates are `rlib` only (RD-1120-11): as `["rlib", "cdylib"]` every native build that
# named one — a test run, a `cargo build` of the workspace — linked a shared object nobody loads,
# 72 of them with 551 MiB in target/debug/deps and the link time on every run. The component is
# asked for here alone, with `cargo rustc --crate-type cdylib`, which takes one package per call.
# So the shared plugin libraries the selection links, and with them every registry crate, are
# built first in ONE call with every `-p` (RD-130-17: one call per plugin kept a single core busy
# on small crates, 72 times over); what is left per plugin is its own leaf crate. The plugins
# declare no features of their own, so building one alone unifies nothing it would not get in
# company — a signed build's same-version comparison would notice if that ever changed. A plugin
# that does not build stops the run with its name.
build_components() {
    local name component library libraries=() started="$SECONDS"
    [[ $# -gt 0 ]] || return 0
    export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS
    CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="$(wasm_rustflags)"
    for name in "$@"; do
        rm -f "$(stamp_path "$name")"
        source_files "$name" | grep -v '^crates/' | tr '\n' '\0' | xargs -0 -r touch
    done
    while read -r library; do
        libraries+=(-p "$library")
    done < <(shared_libraries "$@")
    if [[ ${#libraries[@]} -gt 0 ]]; then
        echo "--> the $(( ${#libraries[@]} / 2 )) shared plugin libraries first, in one cargo call"
        CARGO_BUILD_JOBS="$JOBS" cargo build --release --target "$TARGET" -j "$JOBS" "${libraries[@]}" \
            || { echo "!! the shared plugin libraries do not build" >&2; exit 1; }
    fi
    for name in "$@"; do
        echo "--> rd-plugin-$name"
        CARGO_BUILD_JOBS="$JOBS" cargo rustc --release --target "$TARGET" -j "$JOBS" \
            -p "rd-plugin-$name" --lib --crate-type cdylib \
            || { echo "!! rd-plugin-$name does not build" >&2; exit 1; }
    done
    for name in "$@"; do
        component="$(component_path "$name")"
        [[ -f "$component" ]] || { echo "!! $component was not produced" >&2; exit 1; }
        make_component "$name"
        # The same guard CI runs, here so a forbidden import is caught before the push rather
        # than after it.
        "$ROOT/scripts/check-plugin-imports.sh" "$component"
        write_stamp "$name"
    done
    echo "--> $# component(s) built in $(( SECONDS - started )) s"
}

# The package names of the shared plugin libraries (the plugins/ crates without a manifest) the
# plugins named as arguments link, each once.
shared_libraries() {
    local name directory
    for name in "$@"; do
        source_files "$name" | sed -n 's|^\(plugins/[^/]*\)/Cargo\.toml$|\1|p'
    done | LC_ALL=C sort -u | while read -r directory; do
        [[ -f "$directory/manifest.toml" ]] && continue
        sed -n 's/^name = "\(.*\)"/\1/p' "$directory/Cargo.toml" | head -1
    done
}
