#!/usr/bin/env bash
#
# scripts/release-assets.sh on a scratch download of a release run (owner, 2026-10-04): `split`
# drops the intermediate files and the docker build record and moves the plugins apart, `sums`
# writes one SHA256SUMS per release over its own files only. Then the workflows' push triggers,
# read from .github/workflows/*.yml: the release tag `vX.Y.Z` starts release.yml but neither
# installers.yml nor self-update.yml (owner, 2026-10-04), and a push of the plugin release's tag
# `plugins-vX.Y.Z` starts no workflow at all.
#
#   scripts/tests/release-assets.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# File names in byte order, as `sort` gives them in CI.
export LC_ALL=C

assets_of() { "$ROOT/scripts/release-assets.sh" "$@"; }
listing() { (cd "$1" && find . -maxdepth 1 -type f -printf '%f\n' | sort | paste -sd' '); }

# What `publish` downloads: every artifact of the run, merged into one directory.
download="$SCRATCH/release-assets"
mkdir -p "$download"
for file in rdownloader-linux-x86_64.tar.gz rdownloader-windows-x86_64.zip \
    rdownloader-windows-x86_64.msi rdownloader_1.9.1_amd64.deb rdownloader-chrome.zip \
    rdownloader-plugin-index.json rdownloader-site-rules.json \
    rdownloader-linux-x86_64.unpacked.tar rdownloader-windows-x86_64.unpacked.zip rd-pack \
    'degoya~rDownloader~ABC123.dockerbuild' ddownload-0.3.1.rdplug http-1.0.0.rdplug; do
    echo "$file" > "$download/$file"
done
plugins="$SCRATCH/plugin-assets"

run_status assets_of split "$download" "$plugins"
expect_status "split" 0
expect "the plugin release holds the plugins only" \
    "ddownload-0.3.1.rdplug http-1.0.0.rdplug" "$(listing "$plugins")"
expect "the application release keeps the index and drops the intermediates and the build record" \
    "rdownloader-chrome.zip rdownloader-linux-x86_64.tar.gz rdownloader-plugin-index.json rdownloader-site-rules.json rdownloader-windows-x86_64.msi rdownloader-windows-x86_64.zip rdownloader_1.9.1_amd64.deb" \
    "$(listing "$download")"

echo sbom > "$download/rdownloader.spdx.json"
run_status assets_of sums "$download" "$plugins"
expect_status "sums" 0
expect "each SHA256SUMS lists its own release's files, sorted" \
    "./ddownload-0.3.1.rdplug ./http-1.0.0.rdplug" "$(awk '{ print $2 }' "$plugins/SHA256SUMS" | paste -sd' ')"
expect "the application's SHA256SUMS names no plugin and covers the SBOM" \
    "0 1" "$(grep -c 'rdplug' "$download/SHA256SUMS" || true) $(grep -c 'rdownloader.spdx.json' "$download/SHA256SUMS")"
expect "the plugin index is in the application's SHA256SUMS" \
    "1" "$(grep -c 'rdownloader-plugin-index.json' "$download/SHA256SUMS")"
run_status bash -c "cd '$download' && sha256sum --check --quiet SHA256SUMS && cd '$plugins' && sha256sum --check --quiet SHA256SUMS"
expect_status "both verify with sha256sum --check" 0
echo signature > "$plugins/SHA256SUMS.sigstore.json"
run_status assets_of sums "$plugins"
expect "SHA256SUMS* is never listed, so a re-run does not checksum its own signature" \
    "2" "$(wc -l < "$plugins/SHA256SUMS" | tr -d ' ')"

empty="$SCRATCH/no-plugins"
mkdir -p "$empty"
echo archive > "$empty/rdownloader-linux-x86_64.tar.gz"
run_status assets_of split "$empty" "$SCRATCH/empty-plugins"
expect_status "a run without plugins publishes nothing" 1
expect_output "naming why" "holds no .rdplug"
run_status assets_of split "$empty"
expect_status "split without its second directory" 2

# The push filters of one workflow as `<key><TAB><pattern>` lines (`push<TAB>` for a bare push),
# read from its `on:` block: keys at four spaces under `  push:`, values inline (`["a", "b"]`)
# or as a list. Enough for this repository's workflows, which all write `on:` as a block.
push_filters() {
    awk '
        function emit(value,   items, n, i, item) {
            gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", value)
            n = split(value, items, ",")
            for (i = 1; i <= n; i++) {
                item = items[i]
                gsub(/^[[:space:]]*["'\'']?|["'\'']?[[:space:]]*$/, "", item)
                if (item != "") print key "\t" item
            }
        }
        /^on:[[:space:]]*$/ { on = 1; next }
        on && /^[^[:space:]#]/ { on = 0 }
        !on { next }
        /^  push:[[:space:]]*$/ { push = 1; print "push\t"; next }
        push && /^  [^[:space:]#]/ { push = 0 }
        !push || /^[[:space:]]*#/ { next }
        /^    [a-z-]+:/ {
            key = $1; sub(/:$/, "", key)
            value = $0; sub(/^[^:]*:/, "", value)
            if (value ~ /[^[:space:]]/) emit(value)
            next
        }
        /^      - / { value = $0; sub(/^      - /, "", value); emit(value) }
    ' "$1"
}

# Whether GitHub starts workflow $1 for a push of tag $2. Path filters are not evaluated for
# tags; a workflow that filters only branches never runs for one.
starts_on_tag() {
    local filters patterns pattern matched=1
    filters="$(push_filters "$1")"
    [[ -n "$filters" ]] || return 1
    if grep -q $'^tags\t' <<< "$filters"; then
        patterns="$(sed -n $'s/^tags\t//p' <<< "$filters")"
        matched=1
        while IFS= read -r pattern; do
            # shellcheck disable=SC2053  # the pattern is a glob on purpose
            if [[ "$pattern" == '!'* ]]; then
                [[ "$2" == ${pattern#!} ]] && matched=1
            elif [[ "$2" == $pattern ]]; then
                matched=0
            fi
        done <<< "$patterns"
        return "$matched"
    fi
    if grep -q $'^tags-ignore\t' <<< "$filters"; then
        while IFS= read -r pattern; do
            # shellcheck disable=SC2053
            [[ "$2" == $pattern ]] && return 1
        done < <(sed -n $'s/^tags-ignore\t//p' <<< "$filters")
        return 0
    fi
    ! grep -qE $'^branches(-ignore)?\t' <<< "$filters"
}

release_yml="$ROOT/.github/workflows/release.yml"
expect "release.yml's tag pattern" "v*.*.*" "$(sed -n $'s/^tags\t//p' < <(push_filters "$release_yml"))"
for tag in v1.9.1 v1.10.0-beta.1; do
    expect_true "release.yml starts on $tag" "starts_on_tag '$release_yml' '$tag'"
done
for workflow in installers.yml self-update.yml; do
    expect_true "$workflow, which filters branches only, runs on no release tag" \
        "! starts_on_tag '$ROOT/.github/workflows/$workflow' v1.9.1"
done
expect_true "ci.yml, which filters branches only, runs on no tag" \
    "! starts_on_tag '$ROOT/.github/workflows/ci.yml' v1.9.1"
for workflow in "$ROOT"/.github/workflows/*.yml; do
    for tag in plugins-v1.9.1 plugins-v1.10.0-beta.1; do
        expect_true "$(basename "$workflow") does not start on $tag" \
            "! starts_on_tag '$workflow' '$tag'"
    done
done
expect "the index points into the plugin release" "1" \
    "$(grep -cF 'releases/download/${PLUGIN_RELEASE_TAG}/' "$release_yml")"
expect "the plugin release is never latest" "1" "$(grep -c '^ *make_latest: false$' "$release_yml")"

finish_tests "release-assets"
