#!/usr/bin/env bash
#
# Builds, signs and packages the bundled WebAssembly plugins into dist/plugins.
#
# Each plugin ends up as exactly one `<name>-<version>.rdplug`: the file name carries the
# version, so a version bump would otherwise leave the previous package next to the new one and
# the count check in package-linux.sh/package-windows.sh would abort on the surplus. The
# superseded package is therefore removed once the new one is written (RD-108-21).
#
# Plugins are built deliberately WITHOUT WASI — scripts/check-plugin-imports.sh rejects any
# import outside `rdownloader:plugin`, here and in CI — so the target is
# wasm32-unknown-unknown via cargo-component. Signing uses the release key; keep it out of the
# repository. The packaged version comes from each plugin's own manifest.toml, which is NOT the
# workspace version: re-packaging must not silently move a plugin to a new version, because
# installed jobs pin the version they were resolved with.
#
# Usage:
#   scripts/build-plugins.sh                    # all plugins that have a manifest
#   scripts/build-plugins.sh ddownload katfile  # only these
#   scripts/build-plugins.sh --development      # unsigned, into dist/dev-plugins
#   scripts/build-plugins.sh --components-only  # build (no signing) what is stale or missing
#   scripts/build-plugins.sh --components-only ddownload  # build (no signing) exactly these
#   scripts/build-plugins.sh --list-stale       # names the components not built from these sources
#   scripts/build-plugins.sh --list-missing     # names the components that were never built here
#   scripts/build-plugins.sh --list-unbumped    # names the plugins changed under a signed version
#   scripts/build-plugins.sh --source-hash ddownload  # the source hash a stamp records
#
# Staleness by content, not by file time (RD-120-58). Every component this script builds gets a
# stamp beside it, `rd_plugin_<name>.wasm.src-sha256`: the hash of the sources it was built from
# and the hash of the component itself. A component is current when both still match. File
# times said "stale" after every checkout and rebase — in 5 of 8 branches on 2026-09-24, for
# plugins nobody had touched. A bare `cargo component build` writes no stamp, so the component
# it leaves no longer matches the old one and counts as stale: a stamp never vouches for a
# build it did not see. `crates/rd-plugin-host/src/artifact.rs` applies the same definition.
#
# Same version, same content (RD-120-47). An installation only takes a bundled package whose
# version is *newer* than the one it has, so a plugin that changed and kept its version is never
# installed: on 2026-09-23 six plugins ran their old code on the owner's instance that way, and a
# fixed bug came back word for word. Every test runs against freshly built code, so nothing but
# the version number could have told. A signed build therefore refuses to package a plugin whose
# `<name>-<version>.rdplug` already exists with a different manifest, component or locale file,
# and names the version to raise; the same content at the same version is a plain rebuild and
# passes. `--list-unbumped` asks the same question without building or signing, for check.sh.
#
set -euo pipefail

KEY="${RDOWNLOADER_PLUGIN_KEY:-$HOME/.config/rdownloader/rdownloader-plugin.key}"
TARGET="wasm32-unknown-unknown"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Where cargo actually writes. Hard-wiring ./target made --list-missing report every plugin as
# missing whenever CARGO_TARGET_DIR pointed elsewhere — which is the normal state in a feature
# worktree — and check.sh then refused to start a run whose components were all built.
#
# In a linked worktree the shared target lives in the main checkout, so that is the default there
# too (RD-120-40). Without it both --list queries looked into the worktree's own, empty target/
# and named every plugin as missing -- the same trap that made worktree.sh's merge gate report a
# green branch as never verified on 2026-09-22, fixed there the same way. Exported, so a build
# from here writes where the queries read. A CARGO_TARGET_DIR that is set still wins.
common="$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir 2> /dev/null || true)"
MAIN_ROOT="$ROOT"
[[ -n "$common" && "$common" != "$ROOT/.git" ]] && MAIN_ROOT="$(dirname "$common")"
if [[ -z "${CARGO_TARGET_DIR:-}" && "$MAIN_ROOT" != "$ROOT" ]]; then
    export CARGO_TARGET_DIR="$MAIN_ROOT/target"
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# The signed packages a plugin's content is compared against (RD-120-47): the ones in the main
# checkout, where the coordinator signs and the packaging scripts collect — a feature worktree's
# own dist/ is empty. RD_PLUGIN_PACKAGES points elsewhere, which the tests use. A directory that
# does not exist, as in a fresh clone, simply holds nothing to compare against.
PACKAGES="${RD_PLUGIN_PACKAGES:-$MAIN_ROOT/dist/plugins}"
# Sourced before the `cd`; taken further down, after the --list-* queries have had their say,
# because those only stat files and must stay answerable while a build is running.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
cd "$ROOT"

development=0
list_only=0
stale_only=0
missing_only=0
unbumped_only=0
components_only=0
hash_only=0
selected=()
for argument in "$@"; do
    case "$argument" in
        --development) development=1 ;;
        --components-only) components_only=1 ;;
        --list-packageable) list_only=1 ;;
        --list-stale) stale_only=1 ;;
        --list-missing) missing_only=1 ;;
        --list-unbumped) unbumped_only=1 ;;
        --source-hash) hash_only=1 ;;
        -*) echo "unknown argument: $argument" >&2; exit 2 ;;
        *) selected+=("$argument") ;;
    esac
done

APP_VERSION="$("$ROOT/scripts/set-version.sh")"

# Whether this application build is new enough to package the plugin in $1.
#
# A plugin may declare `min_app_version`, and `plugin package` verifies it — a package that
# demands a newer core than the one packaging it is refused outright. During a development
# cycle that is the normal state for a plugin written against the release being prepared:
# the workspace version only moves in the `chore(release)` commit, so such a plugin cannot be
# packaged until then. Skipping it is therefore correct rather than a workaround, and it stops
# being skipped by itself at the release bump — nothing to remember and nothing to undo.
packageable() {
    local manifest="plugins/$1/manifest.toml"
    local minimum
    minimum="$(sed -n 's/^min_app_version = "\(.*\)"/\1/p' "$manifest" | head -1)"
    [[ -z "$minimum" ]] && return 0
    [[ "$(printf '%s\n%s\n' "$minimum" "$APP_VERSION" | sort -V | head -1)" == "$minimum" ]]
}

# The plugins this build can actually package, for the release scripts and CI, so the rule
# above lives in exactly one place.
if [[ "$list_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        packageable "$name" && echo "$name"
    done
    exit 0
fi

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
    # `src/bindings.rs` is what cargo-component writes into a plugin crate at every build
    # (gitignored). It is generated from the WIT, which is in the set already, and a checkout that
    # never built the plugin does not have it — counting it would make every fresh worktree
    # disagree with the stamp.
    find "${directories[@]}" -type f \
        \( -name '*.rs' -o -name '*.wit' -o -name Cargo.toml -o -name manifest.toml \) \
        ! -regex 'plugins/[^/]*/src/bindings\.rs' \
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

# Writes the stamp for plugin $1, whose component was just built from this checkout.
write_stamp() {
    local component; component="$(component_path "$1")"
    local stamp; stamp="$(stamp_path "$1")"
    printf '%s %s\n' "$(source_hash "$1")" "$(sha256_files "$component" | cut -c1-64)" > "$stamp.tmp"
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
# the current source hash. A stamp for other bytes means the component was rebuilt without one —
# by a bare `cargo component build`, possibly in another checkout — and vouches for nothing.
stale() {
    local name="$1" component stamp recorded_sources recorded_component
    component="$(component_path "$name")"
    stamp="$(stamp_path "$name")"
    # A component that is not there is `missing`'s question, asked separately below;
    # conflating the two is what left the quiet case uncovered until RD-108-16.
    [[ -f "$component" ]] || return 1
    [[ -f "$stamp" ]] || return 0
    read -r recorded_sources recorded_component < "$stamp" || return 0
    [[ "$recorded_component" == "$(sha256_files "$component" | cut -c1-64)" ]] || return 0
    [[ "$recorded_sources" != "$(source_hash "$name")" ]]
}

if [[ "$hash_only" -eq 1 ]]; then
    for name in "${selected[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || { echo "no plugin named $name" >&2; exit 2; }
        source_hash "$name"
    done
    exit 0
fi

if [[ "$stale_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        stale "$name" && echo "$name"
    done
    exit 0
fi

# Whether the plugin in $1 has no built component in this checkout at all.
#
# The case the staleness check cannot see, and the one that used to pass quietly: the contract
# tests read their components from target/, which is per checkout and which `cargo test` never
# fills. Since RD-108-16 they fail on it rather than returning early, and this is the same
# question asked before the run instead of forty minutes into it.
missing() {
    [[ ! -f "$(component_path "$1")" ]]
}

if [[ "$missing_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        missing "$name" && echo "$name"
    done
    exit 0
fi

# The version plugin $1 declares in its own manifest.
manifest_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' "plugins/$1/manifest.toml" | head -1
}

# Which member of the signed package $1 differs from what plugin $2 would be packaged from, with
# component $3: `manifest.toml`, `component.wasm` or `locales`, printed; nothing when they agree.
#
# Byte for byte, member by member. The packager stores all three verbatim (see
# crates/rd-plugin-host/src/packager.rs), so no re-signing is needed and none happens; comparing
# the archive itself would fail on nothing but the signature. The locales count because they
# ship inside the same package and an installation that keeps it keeps its texts too.
package_drift() {
    local package="$1" directory="plugins/$2" component="$3" packaged present language
    if ! unzip -p "$package" manifest.toml 2> /dev/null | cmp -s - "$directory/manifest.toml"; then
        echo manifest.toml
        return
    fi
    if ! unzip -p "$package" component.wasm 2> /dev/null | cmp -s - "$component"; then
        echo component.wasm
        return
    fi
    packaged="$(unzip -Z1 "$package" 2> /dev/null | sed -n 's|^locales/\(.*\)\.json$|\1|p' | sort || true)"
    present=""
    if [[ -d "$directory/locales" ]]; then
        present="$(find "$directory/locales" -maxdepth 1 -type f -name '*.json' -printf '%f\n' \
            | sed 's/\.json$//' | sort)"
    fi
    if [[ "$packaged" != "$present" ]]; then
        echo locales
        return
    fi
    while read -r language; do
        [[ -n "$language" ]] || continue
        if ! unzip -p "$package" "locales/$language.json" 2> /dev/null \
            | cmp -s - "$directory/locales/$language.json"; then
            echo locales
            return
        fi
    done <<< "$packaged"
}

# The plugins whose built component, manifest or locales differ from a signed package that
# already carries their current version: `<name> <version> <member> <package>`, one per line.
# Without names every plugin; with names only those, which is how check.sh keeps it to the
# change set. A plugin with no package of its version, or no built component, has nothing to
# compare and is not named — the latter is --list-missing's question.
if [[ "$unbumped_only" -eq 1 ]]; then
    if [[ ${#selected[@]} -eq 0 ]]; then
        for manifest in plugins/*/manifest.toml; do
            selected+=("$(basename "$(dirname "$manifest")")")
        done
    fi
    for name in "${selected[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || continue
        version="$(manifest_version "$name")"
        package="$PACKAGES/$name-$version.rdplug"
        component="$(component_path "$name")"
        [[ -n "$version" && -f "$package" && -f "$component" ]] || continue
        member="$(package_drift "$package" "$name" "$component")"
        [[ -z "$member" ]] || echo "$name $version $member $package"
    done
    exit 0
fi

# Everything above this line is a query over existing files. Everything below it compiles.
rd_take_lock "$@"

command -v cargo-component > /dev/null || {
    echo "cargo-component is not installed (cargo install cargo-component)" >&2
    exit 1
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
# ONE cargo call with every `-p` (RD-130-17). One call per plugin kept a single core busy on
# crates that are small, 72 times over; one call lets `-j` fill up across plugins, and its
# memory still depends on `-j`, not on how many plugins are named. The plugins declare no
# features of their own, so building them together unifies nothing a plugin built alone would
# not get — a signed build's same-version comparison would notice if that ever changed.
# When the combined call fails, cargo's error names a crate but not always the plugin that
# pulled it in (a shared library breaks all of them), so the plugins are then built one at a
# time up to the first that fails, which is named, and the run fails.
build_components() {
    local name component packages=()
    [[ $# -gt 0 ]] || return 0
    for name in "$@"; do
        rm -f "$(stamp_path "$name")"
        source_files "$name" | grep -v '^crates/' | tr '\n' '\0' | xargs -0 -r touch
        packages+=(-p "rd-plugin-$name")
    done
    echo "--> $# component(s) in one cargo call"
    if ! CARGO_BUILD_JOBS="$JOBS" cargo component build --release --target "$TARGET" -j "$JOBS" "${packages[@]}"; then
        echo "!! the combined build failed; building one plugin at a time to name the one that fails" >&2
        for name in "$@"; do
            echo "--> rd-plugin-$name"
            CARGO_BUILD_JOBS="$JOBS" cargo component build --release --target "$TARGET" -j "$JOBS" -p "rd-plugin-$name" \
                || { echo "!! rd-plugin-$name does not build" >&2; exit 1; }
        done
        echo "!! every plugin built on its own, but not together — nothing was stamped" >&2
        exit 1
    fi
    for name in "$@"; do
        component="$(component_path "$name")"
        [[ -f "$component" ]] || { echo "!! $component was not produced" >&2; exit 1; }
        # The same guard CI runs, here so a forbidden import is caught before the push rather
        # than after it.
        "$ROOT/scripts/check-plugin-imports.sh" "$component"
        write_stamp "$name"
    done
}

# --components-only: what the contract tests need, and nothing a release needs — no signing, no
# package. Without names it builds every component that is missing or stale, so after a
# checkout or a merge it is the one command that makes the component checks of check.sh pass.
if [[ "$components_only" -eq 1 ]]; then
    if [[ ${#selected[@]} -eq 0 ]]; then
        for manifest in plugins/*/manifest.toml; do
            name="$(basename "$(dirname "$manifest")")"
            if missing "$name" || stale "$name"; then selected+=("$name"); fi
        done
    fi
    echo "==> building ${#selected[@]} component(s) for $TARGET, unsigned (jobs: $JOBS)"
    for name in "${selected[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || { echo "!! no plugin named $name" >&2; exit 2; }
    done
    build_components "${selected[@]}"
    echo "==> done — ${#selected[@]} component(s) built and stamped"
    exit 0
fi

OUT="$ROOT/dist/plugins"
[[ "$development" -eq 1 ]] && OUT="$ROOT/dist/dev-plugins"
mkdir -p "$OUT"

# Drops the packages of plugin $1 that the freshly written $2 replaces.
#
# The file name carries the plugin's own version, so a bump writes a second file instead of
# overwriting the first, and the count check of the packaging scripts — which exists to catch a
# missing plugin — then reports one too many and stops the release build. Keeping the version in
# the name is worth that cleanup: it is what the released artifact set and the release workflow
# publish, and it names the version a package carries without opening the archive.
#
# The match is anchored on `<name>-<digit>` rather than a plain `$name-*` glob, so `realdebrid`
# never claims `realdebrid-torrents`. Pruning happens after packaging succeeded: a failed build
# must leave the previous package in place.
prune_superseded() {
    local name="$1" keep="$2" existing remainder
    for existing in "$OUT/$name"-*.rdplug; do
        [[ -f "$existing" && "$existing" != "$keep" ]] || continue
        remainder="${existing##*/}"
        remainder="${remainder#"$name-"}"
        [[ "$remainder" == [0-9]* ]] || continue
        rm -f "$existing"
        echo "    removed superseded ${existing##*/}"
    done
}

if [[ "$development" -eq 0 && ! -f "$KEY" ]]; then
    echo "signing key not found at $KEY" >&2
    echo "set RDOWNLOADER_PLUGIN_KEY, or pass --development for an unsigned build" >&2
    exit 1
fi

# Only directories carrying a manifest are plugins; the others (common, guest, xfs-common) are
# shared libraries the plugins depend on and have nothing to package.
if [[ ${#selected[@]} -eq 0 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        if ! packageable "$name"; then
            echo "    skipping $name: it needs a newer application version than $APP_VERSION" >&2
            continue
        fi
        selected+=("$name")
    done
fi

refused=()
# The packager is the host binary, built once here and called directly below. `cargo run` per
# plugin rebuilt it every time: `build_components` touches the shared plugin libraries, and
# rdownloader links the native fallbacks that depend on them, so each package paid a full
# release link of the host (2026-09-24: 12 packages in 43 minutes).
echo "==> building the packager (rdownloader, release)"
CARGO_BUILD_JOBS="$JOBS" cargo build --quiet --release -j "$JOBS" -p rdownloader
PACKAGER="$TARGET_DIR/release/rdownloader"
[[ -x "$PACKAGER" ]] || { echo "!! $PACKAGER was not produced" >&2; exit 1; }

buildable=()
for name in "${selected[@]}"; do
    [[ -f "plugins/$name/manifest.toml" ]] || { echo "!! $name has no manifest.toml — skipped" >&2; continue; }
    buildable+=("$name")
done
echo "==> building ${#buildable[@]} plugin(s) for $TARGET (jobs: $JOBS)"
build_components "${buildable[@]}"

for name in "${buildable[@]}"; do
    directory="plugins/$name"
    component="$(component_path "$name")"

    version="$(manifest_version "$name")"
    output="$OUT/$name-${version:?no version in $directory/manifest.toml}.rdplug"

    # Same version, same content (RD-120-47). Checked against the package about to be replaced
    # and against the signed set of the main checkout, which differ in a feature worktree. Not
    # for --development: unsigned packages are never what an installation is shipped. A refused
    # plugin is left unpackaged and the sweep goes on, so one forgotten bump does not leave the
    # rest of a routine re-sign undone; the run still fails at the end.
    if [[ "$development" -eq 0 ]]; then
        drift=""
        for reference in "$output" "$PACKAGES/${output##*/}"; do
            [[ -f "$reference" ]] || continue
            member="$(package_drift "$reference" "$name" "$component")"
            if [[ -n "$member" ]]; then
                drift="$member differs from the signed ${reference}"
                break
            fi
        done
        if [[ -n "$drift" ]]; then
            echo "!! $name $version: $drift" >&2
            echo "   raise \`version\` in $directory/manifest.toml — an installation only takes a newer one" >&2
            refused+=("$name $version")
            continue
        fi
    fi

    arguments=(plugin package --manifest "$directory/manifest.toml" --component "$component" --output "$output")
    [[ -d "$directory/locales" ]] && arguments+=(--locales "$directory/locales")
    if [[ "$development" -eq 1 ]]; then
        arguments+=(--development)
    else
        arguments+=(--key "$KEY")
    fi

    "$PACKAGER" "${arguments[@]}"
    prune_superseded "$name" "$output"
    echo "    $output"
done

echo "==> done — $(ls -1 "$OUT"/*.rdplug 2>/dev/null | wc -l) package(s) in $OUT"

if [[ ${#refused[@]} -gt 0 ]]; then
    echo "!! not packaged — changed, but a signed package of the same version exists:" >&2
    printf '     %s\n' "${refused[@]}" >&2
    echo "   Raise each one's version in the same commit as the change (RD-120-47)." >&2
    exit 1
fi
