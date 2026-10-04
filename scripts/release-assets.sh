#!/usr/bin/env bash
#
# The files of the two GitHub releases a tag publishes, as the Release workflow's `publish` job
# lays them out (owner, 2026-10-04): the application release `vX.Y.Z` and the plugin release
# `plugins-vX.Y.Z`. v1.9.0 listed its 69 signed plugins alphabetically before the installers, and
# GitHub has no folders for assets.
#
#   scripts/release-assets.sh split <assets> <plugins>
#
# <assets> is every workflow artifact of the run, downloaded into one directory. Drops what is
# no release file — the archives without their plugins (`*.unpacked.*`), the packager `rd-pack`,
# a `*.dockerbuild` build record of docker/build-push-action — and moves every `*.rdplug` into
# <plugins>. Everything else stays: archives, installers, extensions, the site-rule pack and the
# plugin index, whose entries point into the plugin release. Exit 1 when <assets> holds no
# `.rdplug`: a release without its plugins would publish an index naming files nobody can fetch.
#
#   scripts/release-assets.sh sums <directory>...
#
# Writes `<directory>/SHA256SUMS` over every file in it but `SHA256SUMS*`, sorted by name, so each
# release carries the checksums of its own assets and `sha256sum --check SHA256SUMS` verifies a
# full download of either. Run after the SBOM, which the application's sums cover.
#
#   scripts/release-assets.sh fetch-plugins <repository> <directory>
#
# Downloads the `.rdplug` files of the newest release of <repository> (`owner/name`, through
# `gh`) into <directory>: from `plugins-<tag>` of the release GitHub calls `latest`, or from the
# release itself while that is v1.9.0 or older, which carried its plugins as its own assets.
# installers.yml stages its packages with it.
set -euo pipefail

usage() {
    echo "usage: scripts/release-assets.sh split <assets> <plugins>" >&2
    echo "       scripts/release-assets.sh sums <directory>..." >&2
    echo "       scripts/release-assets.sh fetch-plugins <repository> <directory>" >&2
    exit 2
}

split() {
    local assets="$1" plugins="$2" moved=0 package
    [[ -d "$assets" ]] || { echo "release-assets: $assets is not a directory" >&2; exit 2; }
    rm -f "$assets"/*.unpacked.* "$assets/rd-pack" "$assets"/*.dockerbuild
    mkdir -p "$plugins"
    for package in "$assets"/*.rdplug; do
        [[ -f "$package" ]] || continue
        mv "$package" "$plugins/"
        moved=$((moved + 1))
    done
    if [[ "$moved" -eq 0 ]]; then
        echo "release-assets: $assets holds no .rdplug; the plugin release would be empty" >&2
        exit 1
    fi
    echo "release-assets: $moved plugin(s) to $plugins, $(find "$assets" -maxdepth 1 -type f | wc -l) file(s) stay in $assets"
}

sums() {
    local directory
    for directory in "$@"; do
        [[ -d "$directory" ]] || { echo "release-assets: $directory is not a directory" >&2; exit 2; }
        (
            cd "$directory"
            find . -maxdepth 1 -type f ! -name 'SHA256SUMS*' -print0 \
                | sort --zero-terminated \
                | xargs --null --no-run-if-empty sha256sum > SHA256SUMS
        )
        echo "release-assets: $directory/SHA256SUMS lists $(wc -l < "$directory/SHA256SUMS") file(s)"
    done
}

fetch_plugins() {
    local repository="$1" directory="$2" tag release
    tag="$(gh release view --repo "$repository" --json tagName --jq .tagName)"
    release="plugins-$tag"
    if ! gh release view "$release" --repo "$repository" > /dev/null 2>&1; then
        release="$tag"
    fi
    mkdir -p "$directory"
    gh release download "$release" --repo "$repository" --pattern '*.rdplug' --dir "$directory"
    echo "release-assets: $(find "$directory" -maxdepth 1 -name '*.rdplug' | wc -l) plugin(s) of $release in $directory"
}

case "${1:-}" in
    split)
        [[ $# -eq 3 ]] || usage
        split "$2" "$3"
        ;;
    sums)
        [[ $# -ge 2 ]] || usage
        shift
        sums "$@"
        ;;
    fetch-plugins)
        [[ $# -eq 3 ]] || usage
        fetch_plugins "$2" "$3"
        ;;
    *) usage ;;
esac
