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
# wasm32-unknown-unknown: `cargo rustc --crate-type cdylib` links a core module, whose `generate!`
# embeds the world it was written against, and `wasm-tools component new` (the version
# WASM_TOOLS_VERSION pins, as CI does) turns it into the component (RD-1110-08: cargo-component,
# unmaintained since 2025, did the same two steps). Signing uses the release key; keep it out of the
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
#   scripts/build-plugins.sh --list-packageable # names the plugins the bundle ships
#   scripts/build-plugins.sh --list-examples    # names the example plugins, built but never bundled
#   scripts/build-plugins.sh --list-stale       # names the components not built from these sources
#   scripts/build-plugins.sh --list-missing     # names the components that were never built here
#   scripts/build-plugins.sh --list-unbumped    # names the plugins changed under a signed version
#   scripts/build-plugins.sh --source-hash ddownload  # the source hash a stamp records
#   scripts/build-plugins.sh --completed-manifest ddownload  # the manifest a package carries
#   scripts/build-plugins.sh --cache-key        # deps=… and sources=… for CI's component cache
#
# Staleness by content, not by file time (RD-120-58). Every component this script builds gets a
# stamp beside it, `rd_plugin_<name>.wasm.src-sha256`: the hash of the sources it was built from
# and the hash of the component itself, and the dependency hash (`deps_hash`: registry packages,
# root Cargo.toml, compiler). A component is current when all three still match. File
# times said "stale" after every checkout and rebase — in 5 of 8 branches on 2026-09-24, for
# plugins nobody had touched. A bare `cargo rustc --crate-type cdylib` writes no stamp (and leaves
# a core module, not a component), so the file it leaves no longer matches the old one and counts
# as stale: a stamp never vouches for a build it did not see; a bare `cargo build` of a plugin
# links no `.wasm` at all since the crates are `rlib` only (RD-1120-11). `crates/rd-plugin-host/src/artifact.rs` applies the same definition.
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
# from here writes where the queries read. A CARGO_TARGET_DIR that is set still wins. A worktree
# with a target of its own (RD-140-06) builds there; `worktree.sh new --own-target` links its
# wasm32-unknown-unknown/ to the main checkout's, so the components stay the shared ones.
# shellcheck source=lib/lanes.sh
source "$ROOT/scripts/lib/lanes.sh"
MAIN_ROOT="$(rd_main_root "$ROOT")"
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    CARGO_TARGET_DIR="$(rd_target_dir "$ROOT")"
    export CARGO_TARGET_DIR
fi
TARGET_DIR="$CARGO_TARGET_DIR"
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
examples_only=0
components_only=0
hash_only=0
completed_only=0
cache_key_only=0
selected=()
for argument in "$@"; do
    case "$argument" in
        --development) development=1 ;;
        --components-only) components_only=1 ;;
        --list-packageable) list_only=1 ;;
        --list-examples) examples_only=1 ;;
        --list-stale) stale_only=1 ;;
        --list-missing) missing_only=1 ;;
        --list-unbumped) unbumped_only=1 ;;
        --source-hash) hash_only=1 ;;
        --completed-manifest) completed_only=1 ;;
        --cache-key) cache_key_only=1 ;;
        -*) echo "unknown argument: $argument" >&2; exit 2 ;;
        *) selected+=("$argument") ;;
    esac
done

# manifest_version, completed_manifest, package_drift and list_unbumped: the manifest a package
# carries and the comparison --list-unbumped and the packaging share. Functions only; what they
# read (PACKAGES, component_path) is looked up when they run.
# shellcheck source=lib/plugin-drift.sh
source "$ROOT/scripts/lib/plugin-drift.sh"

# The manifest `plugin package` writes into the package, plugins/metadata.toml's shared fields
# filled in (RD-1280-18) -- what crates/rd-plugin-host's test compares its own completion with.
if [[ "$completed_only" -eq 1 ]]; then
    for name in "${selected[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || { echo "no plugin named $name" >&2; exit 2; }
        completed_manifest "$name"
    done
    exit 0
fi

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
    local minimum
    # The completed manifest: a plugin that states no `min_app_version` of its own has
    # plugins/metadata.toml's.
    minimum="$(completed_manifest "$1" | sed -n 's/^min_app_version = "\(.*\)"/\1/p' | head -1)"
    [[ -z "$minimum" ]] && return 0
    [[ "$(printf '%s\n%s\n' "$minimum" "$APP_VERSION" | sort -V | head -1)" == "$minimum" ]]
}

# Whether plugin $1 is an example (RD-150-20). `plugins/example-*` are the components the
# contract tests drive (oauth, stream-transform and transfer contract tests, rd-plugin-host's
# artifact and foreign-address tests); they are built, tested and checked like every other plugin,
# so they stay current, but no installation needs them, so a signed build never packages them and
# the bundle does not carry them. What they taught authors lives in sdk/templates (RD-160-04).
example() { [[ "$1" == example-* ]]; }

# The plugins this build can actually package, for the release scripts and CI, so the rule
# above lives in exactly one place: --list-packageable the bundle, --list-examples the examples,
# which CI packages unsigned to check them.
if [[ "$list_only" -eq 1 || "$examples_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        if example "$name"; then
            [[ "$examples_only" -eq 1 ]] || continue
        else
            [[ "$list_only" -eq 1 ]] || continue
        fi
        packageable "$name" && echo "$name"
    done
    exit 0
fi

# The component paths, the source and dependency hashes, the stamp and the staleness rule.
# shellcheck source=lib/plugin-stamp.sh
source "$ROOT/scripts/lib/plugin-stamp.sh"

if [[ "$hash_only" -eq 1 ]]; then
    for name in "${selected[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || { echo "no plugin named $name" >&2; exit 2; }
        source_hash "$name"
    done
    exit 0
fi

# The key of the component cache in CI and the release (cache_key, lib/plugin-stamp.sh).
if [[ "$cache_key_only" -eq 1 ]]; then
    cache_key
    exit 0
fi

if [[ "$stale_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        stale "$name" && echo "$name"
    done
    exit 0
fi

if [[ "$missing_only" -eq 1 ]]; then
    for manifest in plugins/*/manifest.toml; do
        name="$(basename "$(dirname "$manifest")")"
        missing "$name" && echo "$name"
    done
    exit 0
fi


# --list-unbumped (list_unbumped, lib/plugin-drift.sh).
if [[ "$unbumped_only" -eq 1 ]]; then
    list_unbumped "${selected[@]}"
    exit 0
fi

# Everything above this line is a query over existing files. Everything below it compiles.
rd_take_lock "$@"

# The component bytes depend on the wasm-tools that encodes them, and the signed packages are
# compared by those bytes (RD-120-47), so a build here uses the version CI uses or none.
installed_wasm_tools="$(wasm_tools_version)"
[[ "$installed_wasm_tools" == "$WASM_TOOLS_VERSION" ]] || {
    echo "wasm-tools $WASM_TOOLS_VERSION is required, found ${installed_wasm_tools:-none}" >&2
    echo "   cargo install wasm-tools --version $WASM_TOOLS_VERSION --locked" >&2
    exit 1
}

# The component build: make_component, wasm_rustflags, build_components, shared_libraries.
# shellcheck source=lib/plugin-build.sh
source "$ROOT/scripts/lib/plugin-build.sh"

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
        # The development set keeps the examples: it is what a plugin author runs.
        if [[ "$development" -eq 0 ]] && example "$name"; then
            continue
        fi
        selected+=("$name")
    done
fi

refused=()
# The packager is built once here and called directly below. `cargo run` per plugin rebuilt it
# every time: `build_components` touched the shared plugin libraries, and rdownloader linked the
# native fallbacks that depended on them, so each package paid a full release link of the host
# (2026-09-24: 12 packages in 43 minutes).
#
# It is `rd-pack`, not the service (RD-150-20): the same `plugin package` command, without
# rd-api, the queue or the web assets. And it is built in `release-test`, not `release`:
# `plugin package` compiles every component with Wasmtime to validate it, which a debug build of
# Cranelift makes slow, while `release`'s single code unit and thin LTO buy a tool nothing and
# cost the build the most. The owner's test packages use the same profile, so the plugin host
# and Wasmtime are compiled once for both.
PACKAGER_PROFILE="release-test"
echo "==> building the packager (rd-pack, $PACKAGER_PROFILE)"
CARGO_BUILD_JOBS="$JOBS" cargo build --quiet --profile "$PACKAGER_PROFILE" -j "$JOBS" -p rd-pack
PACKAGER="$TARGET_DIR/$PACKAGER_PROFILE/rd-pack"
[[ -x "$PACKAGER" ]] || { echo "!! $PACKAGER was not produced" >&2; exit 1; }

buildable=()
for name in "${selected[@]}"; do
    [[ -f "plugins/$name/manifest.toml" ]] || { echo "!! $name has no manifest.toml — skipped" >&2; continue; }
    if [[ "$development" -eq 0 ]] && example "$name"; then
        echo "!! $name is an example and never signed into the bundle — skipped (--development packages it)" >&2
        continue
    fi
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
    [[ -f plugins/metadata.toml ]] && arguments+=(--defaults plugins/metadata.toml)
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

# A signed example package from before RD-150-20 would still ship with the bundle, and the count
# check of the packaging scripts would stop on it; dist/plugins holds the bundle and nothing else.
if [[ "$development" -eq 0 ]]; then
    for existing in "$OUT"/example-*.rdplug; do
        [[ -f "$existing" ]] || continue
        rm -f "$existing"
        echo "    removed ${existing##*/}: examples are not bundled"
    done
fi

echo "==> done — $(ls -1 "$OUT"/*.rdplug 2>/dev/null | wc -l) package(s) in $OUT"

if [[ ${#refused[@]} -gt 0 ]]; then
    echo "!! not packaged — changed, but a signed package of the same version exists:" >&2
    printf '     %s\n' "${refused[@]}" >&2
    echo "   Raise each one's version in the same commit as the change (RD-120-47)." >&2
    exit 1
fi
