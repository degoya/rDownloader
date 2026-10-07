#!/usr/bin/env bash
#
# The plugin steps of ci.yml's `components` job and release.yml's `plugins` job (RD-1101-07 moved
# them out of the workflows as they were). CI only: each runs from the checkout root after
# scripts/build-plugins.sh --components-only has built the components into
# target/wasm32-unknown-unknown/release/.
#
#   scripts/ci-plugins.sh list [--examples]        # step outputs names, packageable (examples)
#   scripts/ci-plugins.sh imports [--annotate] <plugin>...
#   scripts/ci-plugins.sh package-dev <plugin>...  # unsigned packages, verified (ci.yml)
#   scripts/ci-plugins.sh templates                # the SDK templates' WIT copies are current
#   scripts/ci-plugins.sh scaffolds                # every template scaffolds, builds and packages
#   scripts/ci-plugins.sh conformance <plugin>...  # conformance of the unsigned packages
#   scripts/ci-plugins.sh package-signed <plugin>... # signed packages and their notes (release.yml)
#
# Reads GITHUB_OUTPUT and GITHUB_WORKSPACE as the runner sets them; `package-signed` reads
# RDOWNLOADER_PLUGIN_SIGNING_KEY through the packager.
set -euo pipefail

component() {
    echo "target/wasm32-unknown-unknown/release/rd_plugin_${1//-/_}.wasm"
}

# Every plugins/ directory with a manifest.toml is a plugin and built here; xfs-common has none
# because it is a shared library. Deriving the list means adding a plugin is one directory, not
# five identical loops to keep in step.
#
# Building proves a plugin compiles; packaging additionally checks its `min_app_version` against
# this build, and a plugin written for the release being prepared cannot be packaged until the
# version bump. scripts/build-plugins.sh owns that rule so it lives in one place and clears itself
# at the bump. The examples (`plugins/example-*`) are built but not bundled (RD-150-20); ci.yml
# packages and checks them all the same (--examples), so they stay current.
list() {
    local names packageable examples
    names=$(find plugins -mindepth 2 -maxdepth 2 -name manifest.toml -printf '%h\n' \
        | xargs -n1 basename | sort | tr '\n' ' ')
    echo "names=${names}" >> "$GITHUB_OUTPUT"
    echo "building: ${names}"
    packageable=$(scripts/build-plugins.sh --list-packageable | sort | tr '\n' ' ')
    if [[ "${1:-}" == --examples ]]; then
        examples=$(scripts/build-plugins.sh --list-examples | sort | tr '\n' ' ')
        echo "packageable=${packageable}" >> "$GITHUB_OUTPUT"
        echo "examples=${examples}" >> "$GITHUB_OUTPUT"
        echo "packaging: ${packageable}"
        echo "examples: ${examples}"
    else
        echo "packageable=${packageable}" >> "$GITHUB_OUTPUT"
        echo "packaging: ${packageable}"
    fi
}

# Reads each component's import section with wasm-tools instead of scanning the bytes. The byte
# scan this replaced could not tell an import from a string, so a plugin whose data section merely
# contained the text "wasi:" was rejected as importing it. --annotate adds ci.yml's error line.
imports() {
    local annotate=0 plugin components=()
    if [[ "${1:-}" == --annotate ]]; then annotate=1; shift; fi
    for plugin in "$@"; do
        components+=("$(component "${plugin}")")
    done
    if [[ "${annotate}" -eq 0 ]]; then
        scripts/check-plugin-imports.sh "${components[@]}"
        return
    fi
    scripts/check-plugin-imports.sh "${components[@]}" \
        || { echo "::error::a plugin imports outside rdownloader:plugin"; exit 1; }
}

package_dev() {
    local plugin
    mkdir -p dist/plugins
    for plugin in "$@"; do
        target/debug/rd-pack plugin package --development \
            --manifest "plugins/${plugin}/manifest.toml" \
            --component "$(component "${plugin}")" \
            --locales "plugins/${plugin}/locales" \
            --output "dist/plugins/${plugin}.rdplug"
        target/debug/rd-pack plugin verify --development-mode "dist/plugins/${plugin}.rdplug"
    done
}

# The templates ship their own copy so a scaffold builds without this repository. A copy that
# drifts hands third parties a contract this core no longer speaks.
templates() {
    local template
    for template in sdk/templates/*/wit/rdownloader.wit; do
        diff -u crates/rd-plugin-api/wit/rdownloader.wit "${template}" \
            || { echo "::error::${template} is out of date"; exit 1; }
    done
}

# In /tmp on purpose: outside the workspace, with no path dependencies to fall back on, which is
# the only way to prove a third party can build one.
#
# Every type, not a sample. The `components` job used to build three — `resolver` as the plainest
# world, `oauth` and `crawler` as the two with logic of their own — on the reasoning that the unit
# test in `crates/rd-pack/src/plugin.rs` covers the rest. It covers that a template exists, names
# its type and carries the current contract; it does not compile the template's code against that
# contract, and the `auth` scaffold was broken by a contract change on the day it was written
# without anything noticing. The scaffolds share one target directory, so the dependency graph is
# built once and each further world costs one crate. The list is the template directories
# themselves (RD-160-04), and check-sdk-templates.sh holds them to the WIT's worlds.
scaffolds() {
    local packager template world out
    packager="${GITHUB_WORKSPACE}/target/debug/rd-pack"
    CARGO_TARGET_DIR="$(mktemp -d)/scaffold-target"
    export CARGO_TARGET_DIR
    for template in sdk/templates/*/; do
        world=$(basename "${template}")
        out=$(mktemp -d)/scaffolded
        "${packager}" plugin new --type "${world}" --out "${out}"
        # The private key stays out of the author's first `git add .` (audit K1).
        git -C "${out}" init --quiet
        git -C "${out}" check-ignore --quiet plugin-signing.key \
            || { echo "::error::a ${world} scaffold does not ignore plugin-signing.key"; exit 1; }
        # The two steps the template's README gives (RD-1110-08): the core module, then its
        # component, written where the README writes it (here the target directory is elsewhere).
        (cd "${out}" && cargo build --release --target wasm32-unknown-unknown)
        mkdir -p "${out}/target"
        wasm-tools component new "${CARGO_TARGET_DIR}/wasm32-unknown-unknown/release/scaffolded.wasm" \
            -o "${out}/target/scaffolded.wasm"
        # The scaffold's own unit tests, on the host target: a third party runs these before
        # touching anything, so they have to pass in a fresh scaffold.
        if grep -rq '#\[test\]' "${out}/src"; then
            (cd "${out}" && cargo test)
        fi
        "${packager}" plugin package \
            --manifest "${out}/manifest.toml" \
            --component "${out}/target/scaffolded.wasm" \
            --locales "${out}/locales" \
            --key "${out}/plugin-signing.key" \
            --output "${out}/scaffolded.rdplug"
        scripts/check-plugin-imports.sh "${out}/target/scaffolded.wasm"
        "${packager}" plugin conformance --json \
            --trusted-key "scaffolded-release-v1=$(sed -n 's/^public_key = "\(.*\)"/\1/p' "${out}/manifest.toml")" \
            --no-default-plugin-key \
            "${out}/scaffolded.rdplug"
    done
}

conformance() {
    local plugin
    for plugin in "$@"; do
        target/debug/rd-pack plugin conformance --development-mode --json \
            "dist/plugins/${plugin}.rdplug"
    done
}

package_signed() {
    local plugin version notes
    mkdir -p dist/plugins dist/plugin-notes
    for plugin in "$@"; do
        version=$(sed -n 's/^version = "\(.*\)"/\1/p' "plugins/${plugin}/manifest.toml" | head -n 1)
        # RD-1140-03: the section of this version in the plugin's CHANGES.md is its notes in the index.
        notes=$(scripts/plugin-release-notes.sh "${plugin}" "${version}")
        if [[ -n "${notes}" ]]; then
            printf '%s\n' "${notes}" > "dist/plugin-notes/${plugin}-${version}.txt"
        fi
        target/release-test/rd-pack plugin package \
            --manifest "plugins/${plugin}/manifest.toml" \
            --component "$(component "${plugin}")" \
            --locales "plugins/${plugin}/locales" \
            --output "dist/plugins/${plugin}-${version}.rdplug"
        target/release-test/rd-pack plugin verify "dist/plugins/${plugin}-${version}.rdplug"
    done
}

command="${1:-}"
shift || true
case "${command}" in
    list) list "$@" ;;
    imports) imports "$@" ;;
    package-dev) package_dev "$@" ;;
    templates) templates ;;
    scaffolds) scaffolds ;;
    conformance) conformance "$@" ;;
    package-signed) package_signed "$@" ;;
    *)
        echo "usage: scripts/ci-plugins.sh list|imports|package-dev|templates|scaffolds|conformance|package-signed [...]" >&2
        exit 2
        ;;
esac
