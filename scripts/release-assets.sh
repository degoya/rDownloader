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
# the binary legs' web interface `web-dist.tar` (RD-1120-07), a `*.dockerbuild` build record of
# docker/build-push-action — and moves every `*.rdplug` into
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
#   scripts/release-assets.sh agent <assets>
#
# Writes the capture agent's own archive beside every application archive in <assets>
# (RD-1210-03): `rdownloader-capture-<platform>-<arch>.<tar.gz|zip>` from
# `rdownloader-<platform>-<arch>.<tar.gz|zip>`, holding the agent, its start and stop scripts,
# the macOS helper app, VERSION.txt, LICENSE and README.md — what an agent installed without the
# service runs from and updates itself from. Run after `split` and before `sums`, so SHA256SUMS,
# its Sigstore signature and the update manifest's `agent_artifacts` cover them like every other
# asset. Exit 1 when <assets> holds no application archive, or one without the agent.
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
    echo "       scripts/release-assets.sh agent <assets>" >&2
    echo "       scripts/release-assets.sh fetch-plugins <repository> <directory>" >&2
    exit 2
}

split() {
    local assets="$1" plugins="$2" moved=0 package
    [[ -d "$assets" ]] || { echo "release-assets: $assets is not a directory" >&2; exit 2; }
    rm -f "$assets"/*.unpacked.* "$assets/rd-pack" "$assets/web-dist.tar" "$assets"/*.dockerbuild
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

# What the agent's archive takes from the application's, where the archive has it.
AGENT_ENTRIES=(rdownloader-capture rdownloader-capture.exe start-capture.sh stop-capture.sh
    start-capture.bat stop-capture.bat start-capture.command stop-capture.command
    "rDownloader Capture.app" VERSION.txt LICENSE README.md)

agent() {
    local assets="$1" archive name stem extension work out entry count=0
    local -a entries
    [[ -d "$assets" ]] || { echo "release-assets: $assets is not a directory" >&2; exit 2; }
    assets="$(cd "$assets" && pwd)"
    for archive in "$assets"/rdownloader-*.tar.gz "$assets"/rdownloader-*.zip; do
        [[ -f "$archive" ]] || continue
        name="$(basename "$archive")"
        case "$name" in
            *.tar.gz) stem="${name%.tar.gz}" extension=tar.gz ;;
            *) stem="${name%.zip}" extension=zip ;;
        esac
        # The application's portable archives only: not the agent's, not an extension's.
        [[ "${stem#rdownloader-}" =~ ^(linux|windows|macos)-(x86_64|aarch64)$ ]] || continue
        work="$(mktemp -d)"
        if [[ "$extension" == tar.gz ]]; then
            tar --extract --gzip --file "$archive" --directory "$work"
        else
            unzip -q "$archive" -d "$work"
        fi
        entries=()
        for entry in "${AGENT_ENTRIES[@]}"; do
            if [[ -e "$work/$entry" ]]; then
                entries+=("$entry")
            fi
        done
        if [[ ! -f "$work/rdownloader-capture" && ! -f "$work/rdownloader-capture.exe" ]]; then
            echo "release-assets: $name carries no rdownloader-capture" >&2
            rm -rf "$work"
            exit 1
        fi
        out="$assets/rdownloader-capture-${stem#rdownloader-}.$extension"
        rm -f "$out"
        if [[ "$extension" == tar.gz ]]; then
            tar --create --directory "$work" --sort=name --owner=0 --group=0 --numeric-owner \
                --file - "${entries[@]}" | gzip --no-name > "$out"
        else
            (cd "$work" && zip -q -r -X "$out" "${entries[@]}")
        fi
        rm -rf "$work"
        count=$((count + 1))
    done
    if [[ "$count" -eq 0 ]]; then
        echo "release-assets: $assets holds no application archive to take the agent from" >&2
        exit 1
    fi
    echo "release-assets: $count capture agent archive(s) in $assets"
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
    agent)
        [[ $# -eq 2 ]] || usage
        agent "$2"
        ;;
    fetch-plugins)
        [[ $# -eq 3 ]] || usage
        fetch_plugins "$2" "$3"
        ;;
    *) usage ;;
esac
