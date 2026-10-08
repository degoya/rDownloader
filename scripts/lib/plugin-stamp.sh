# shellcheck shell=bash
# shellcheck disable=SC2154  # TARGET_DIR and TARGET are build-plugins.sh's, which sources this file
#
# The component stamps of scripts/build-plugins.sh (RD-120-58): where a plugin's component and
# its stamp lie, the source hash and the dependency hash a stamp records, writing a stamp after a
# build, the staleness rule that reads one, the missing component and the cache key (PIPE-02). crates/rd-plugin-host/src/artifact.rs implements
# the same definition; `build-plugins.sh --source-hash` is what its test compares.
#
# Expects from scripts/build-plugins.sh, which sources it: TARGET_DIR and TARGET, and the
# working directory at the checkout root.

# The wasm-tools that turns each core module into its component (RD-1110-08): the one CI installs
# (`tool: wasm-tools@…` in .github/workflows/, held to this by scripts/tests/workflow-shape.sh)
# and the one build-plugins.sh insists on, because it decides the component's bytes.
# shellcheck disable=SC2034  # read by build-plugins.sh, which sources this file
WASM_TOOLS_VERSION="1.261.0"

# The version of the wasm-tools on PATH (`wasm-tools 1.261.0 (…)` -> `1.261.0`), empty without one.
wasm_tools_version() {
    command -v wasm-tools > /dev/null || return 0
    wasm-tools --version | awk 'NR == 1 { print $2 }'
}

# The component of plugin $1, and the stamp that records what it was built from.
component_path() { printf '%s\n' "$TARGET_DIR/$TARGET/release/rd_plugin_${1//-/_}.wasm"; }
stamp_path() { printf '%s.src-sha256\n' "$(component_path "$1")"; }

# The SHA-256 of each file named on the command line, in `sha256sum` format. macOS has no
# sha256sum; its shasum prints the same format.
sha256_files() {
    if command -v sha256sum > /dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

# Every file the component of plugin $1 is built from, relative to the checkout, one per line,
# sorted bytewise: the plugin's own crate, the shared plugin libraries it depends on
# (transitively), and the WIT contract. Path dependencies outside plugins/ are deliberately not
# followed; see the module documentation in crates/rd-plugin-host/src/artifact.rs, which walks
# the same set and must stay in step with this function.
source_files() {
    local name="$1"
    local directories=("plugins/$name" "crates/rd-plugin-api/wit")
    local pending=("plugins/$name") current dependency
    while [[ ${#pending[@]} -gt 0 ]]; do
        current="${pending[0]}"
        pending=("${pending[@]:1}")
        [[ -f "$current/Cargo.toml" ]] || continue
        while read -r dependency; do
            dependency="plugins/$dependency"
            [[ -d "$dependency" ]] || continue
            printf '%s\n' "${directories[@]}" | grep -qxF "$dependency" && continue
            directories+=("$dependency")
            pending+=("$dependency")
        done < <(sed -n 's|.*path = "\.\./\([^/"]*\)".*|\1|p' "$current/Cargo.toml")
    done
    find "${directories[@]}" -type f \
        \( -name '*.rs' -o -name '*.wit' -o -name Cargo.toml -o -name manifest.toml \) \
        | LC_ALL=C sort
}

# The source hash of plugin $1: the SHA-256 of the `sha256sum` listing of its sorted source
# files (`<hex>  <relative path>` per line). Paths and contents only — no file time and no
# checkout location enters it, so every worktree of the same commit gets the same answer.
source_hash() {
    local files
    mapfile -t files < <(source_files "$1")
    sha256_files "${files[@]}" | sha256_files - | cut -c1-64
}

# What every component depends on besides its own sources: the registry packages in Cargo.lock,
# the root Cargo.toml (workspace dependencies, features, the release profile) without the
# workspace version, .cargo/config.toml, the compiler cargo resolves here (`rustc -vV`) and the
# wasm-tools that encodes the component (since 1.11; cargo-component's version was in no hash). The
# component cache's `deps` key, and the third field of a stamp: until 1.10 a stamp held the
# sources alone, so after the move to Rust 1.99 the components the shared target/ still had from
# 1.98.1 passed as current, --list-unbumped compared those, and only signing the release found 39
# plugins whose components had changed under a signed version. Computed once per run.
deps_hash_value=""
deps_hash() {
    if [[ -z "$deps_hash_value" ]]; then
        deps_hash_value="$({
            awk 'BEGIN { RS = "" } /\nsource = / { print; print "" }' Cargo.lock
            sed '/^\[workspace\.package\]/,/^\[/{/^version = /d;}' Cargo.toml
            [[ ! -f .cargo/config.toml ]] || cat .cargo/config.toml
            rustc -vV
            printf 'wasm-tools %s\n' "$(wasm_tools_version)"
        } | sha256_files - | cut -c1-64)"
    fi
    printf '%s\n' "$deps_hash_value"
}

# Writes the stamp for plugin $1, whose component was just built from this checkout: the source
# hash, the component's own hash and the dependency hash. crates/rd-plugin-host/src/artifact.rs
# reads the first two.
write_stamp() {
    local component; component="$(component_path "$1")"
    local stamp; stamp="$(stamp_path "$1")"
    printf '%s %s %s\n' "$(source_hash "$1")" "$(sha256_files "$component" | cut -c1-64)" \
        "$(deps_hash)" > "$stamp.tmp"
    mv "$stamp.tmp" "$stamp"
}

# Whether the built component of plugin $1 is not the one its current sources produce.
#
# `target/` is shared and `cargo test` never builds components, so a merge — or another
# worktree's build — leaves a component next to sources it was not built from, and the contract
# tests then run that guest code against the current expectations. The tests say so themselves
# (`rd_plugin_host::artifact`, the authority on the rule); this is the same question asked in a
# second, cheaper place, so `scripts/check.sh` can answer it before a run that would fail.
#
# Stale unless the stamp exists, describes exactly these component bytes, and records exactly
# the current source hash and dependency hash. A stamp for other bytes means the component was
# rebuilt without one — by a bare `cargo build`, possibly in another checkout — and
# vouches for nothing; a stamp without the dependency hash predates it and is stale once.
stale() {
    local name="$1" component stamp recorded_sources recorded_component recorded_deps
    component="$(component_path "$name")"
    stamp="$(stamp_path "$name")"
    # A component that is not there is `missing`'s question, asked separately below;
    # conflating the two is what left the quiet case uncovered until RD-108-16.
    [[ -f "$component" ]] || return 1
    [[ -f "$stamp" ]] || return 0
    read -r recorded_sources recorded_component recorded_deps < "$stamp" || return 0
    [[ "$recorded_component" == "$(sha256_files "$component" | cut -c1-64)" ]] || return 0
    [[ "$recorded_deps" == "$(deps_hash)" ]] || return 0
    [[ "$recorded_sources" != "$(source_hash "$name")" ]]
}

# Whether the plugin in $1 has no built component in this checkout at all.
#
# The case the staleness check cannot see, and the one that used to pass quietly: the contract
# tests read their components from target/, which is per checkout and which `cargo test` never
# fills. Since RD-108-16 they fail on it rather than returning early, and this is the same
# question asked before the run instead of forty minutes into it.
missing() {
    [[ ! -f "$(component_path "$1")" ]]
}

# The key of the component cache in CI and the release (RD-150-10), as two `name=value` lines
# for $GITHUB_OUTPUT. `sources` covers every plugin's source hash, so an exact hit is a set of
# components built from exactly these sources. `deps` covers what a stamp deliberately leaves
# out and a component still depends on: the registry packages in Cargo.lock, the root Cargo.toml
# (workspace dependencies, features, the release profile), .cargo/config.toml and the compiler
# cargo resolves here (`rustc -vV`, RUSTUP_TOOLCHAIN or rust-toolchain.toml's channel), so a
# toolchain change is a miss even where the key's literal version was not moved. The workspace
# version is taken out of both files first — every release moves it, and no component reads it.
# The workflow restores by `deps` alone when `sources` misses, and build-plugins.sh's staleness
# check then rebuilds exactly the plugins whose stamps no longer match.
cache_key() {
    local manifest name
    printf 'deps=%s\n' "$(deps_hash)"
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        printf '%s %s\n' "$name" "$(source_hash "$name")"
    done | sha256_files - | cut -c1-64 | sed 's/^/sources=/'
}
