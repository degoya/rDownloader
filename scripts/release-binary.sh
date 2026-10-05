#!/usr/bin/env bash
#
# The bash steps of release.yml's binary jobs (`linux-binaries`, `binaries`) and of the container
# jobs that take their Linux archives apart again (RD-1101-07 moved them out of the workflow as
# they were). CI only; each runs from the checkout root after `cargo build --release --target`.
#
#   ARTIFACT=rdownloader-linux-x86_64 scripts/release-binary.sh version-file
#   scripts/release-binary.sh macos-helper                    # into stage/ (macOS)
#   scripts/release-binary.sh package-unix <target> <artifact> # stage/ → <artifact>.unpacked.tar
#   scripts/release-binary.sh docker-layout <arch>:<docker arch>...
#
# `version-file` reads RD_VERSION from scripts/release-build-env.sh's environment; RUNNER_OS as the
# runner sets it.
set -euo pipefail

# VERSION.txt, as in the local packages (scripts/lib/archive-layout.sh): the archives carried none
# until 1.8 (RD-180-05). The platform in the words the About page uses.
version_file() {
    local platform
    mkdir stage
    # shellcheck source=lib/version-file.sh
    source scripts/lib/version-file.sh
    platform="${ARTIFACT#rdownloader-}"
    rd_write_version_file stage "${RD_VERSION}" "${platform%%-*} ${platform#*-}" release
    cat stage/VERSION.txt
}

macos_helper() {
    local version
    osacompile -o "stage/rDownloader Capture.app" resources/macos/rdownloader-capture.applescript
    cp resources/macos/capture-helper-Info.plist "stage/rDownloader Capture.app/Contents/Info.plist"
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
    # Apple allows digits and periods only in both keys (at most three integers), so a beta's
    # `-beta.N` stays out: 1.8.0-beta.2 is 1.8.0 here, as in the browser manifest.
    version="${version%%[-+]*}"
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "stage/rDownloader Capture.app/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "stage/rDownloader Capture.app/Contents/Info.plist"
    plutil -lint "stage/rDownloader Capture.app/Contents/Info.plist"
    codesign --force --deep --sign - "stage/rDownloader Capture.app"
}

# Everything but the plugins, which `packages` adds: an uncompressed tar, so the executable bits
# survive the artifact store and the entries stay the ones made here.
package_unix() {
    local target="$1" artifact="$2"
    cp "target/${target}/release/rdownloader" stage/
    cp "target/${target}/release/rdownloader-capture" stage/
    cp LICENSE README.md stage/
    if [[ "${RUNNER_OS}" == "Linux" ]]; then
        cp scripts/linux/start-rdownloader.sh scripts/linux/stop-rdownloader.sh \
            scripts/linux/start-capture.sh scripts/linux/stop-capture.sh stage/
    else
        cp scripts/macos/start-rdownloader.command scripts/macos/stop-rdownloader.command \
            scripts/macos/start-capture.command scripts/macos/stop-capture.command stage/
    fi
    chmod +x stage/start-rdownloader.* stage/stop-rdownloader.* stage/start-capture.* stage/stop-capture.*
    tar --directory stage --create --file "${artifact}.unpacked.tar" .
}

# docker/Dockerfile's `prebuilt` target reads dist/docker/linux-$TARGETARCH/rdownloader. From the
# tarballs in release-binaries/ rather than loose files: upload-artifact drops the executable bit,
# tar keeps it, and the image then carries exactly the binary the release ships.
docker_layout() {
    local pair arch docker_arch
    for pair in "$@"; do
        arch="${pair%%:*}"
        docker_arch="${pair##*:}"
        mkdir -p "dist/docker/linux-${docker_arch}"
        tar --extract --file "release-binaries/rdownloader-linux-${arch}.unpacked.tar" \
            --directory "dist/docker/linux-${docker_arch}" ./rdownloader
        file "dist/docker/linux-${docker_arch}/rdownloader"
    done
}

command="${1:-}"
shift || true
case "${command}" in
    version-file) version_file ;;
    macos-helper) macos_helper ;;
    package-unix) package_unix "${1:?target}" "${2:?artifact}" ;;
    docker-layout) docker_layout "$@" ;;
    *)
        echo "usage: scripts/release-binary.sh version-file|macos-helper|package-unix <target> <artifact>|docker-layout <arch>:<docker arch>..." >&2
        exit 2
        ;;
esac
