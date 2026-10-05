#!/usr/bin/env bash
#
# The bash steps of ci.yml's `rust` and `docker` jobs that check what a platform ships beside the
# binaries: launchers, the macOS helper app, the Homebrew formula and the Scoop manifest
# (RD-1101-07 moved them out of the workflow as they were). The PowerShell half is
# scripts/ci-platform-smoke.ps1. CI only: each step runs on its runner from the checkout root.
#
#   scripts/ci-platform-smoke.sh launcher-syntax   # bash -n over the Linux or macOS launchers
#   scripts/ci-platform-smoke.sh macos-helper      # compile and lint the NZB helper app (macOS)
#   scripts/ci-platform-smoke.sh portable          # debug build, portable launcher, /api/v1/health
#   scripts/ci-platform-smoke.sh homebrew          # the tap's formulas over this tree's build (macOS)
#   scripts/ci-platform-smoke.sh scoop-manifest    # render the Scoop manifest for the zip (Windows)
#   scripts/ci-platform-smoke.sh optimised         # `docker`: the release-test binary, same launchers
#
# Reads RUNNER_OS and RUNNER_TEMP as the runner sets them.
set -euo pipefail

launcher_syntax() {
    if [[ "${RUNNER_OS}" == "Linux" ]]; then
        bash -n scripts/linux/start-rdownloader.sh scripts/linux/stop-rdownloader.sh \
            scripts/linux/start-capture.sh scripts/linux/stop-capture.sh
    else
        bash -n scripts/macos/start-rdownloader.command scripts/macos/stop-rdownloader.command \
            scripts/macos/start-capture.command scripts/macos/stop-capture.command
    fi
}

macos_helper() {
    local helper_dir version
    helper_dir="$(mktemp -d)"
    osacompile -o "${helper_dir}/rDownloader Capture.app" resources/macos/rdownloader-capture.applescript
    cp resources/macos/capture-helper-Info.plist "${helper_dir}/rDownloader Capture.app/Contents/Info.plist"
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${helper_dir}/rDownloader Capture.app/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "${helper_dir}/rDownloader Capture.app/Contents/Info.plist"
    plutil -lint "${helper_dir}/rDownloader Capture.app/Contents/Info.plist"
}

# One debug build of both binaries for this smoke and, on macOS, the Homebrew formula below:
# no job of ci.yml builds a macOS binary otherwise (`rust` only lints there), so there is no
# artefact to take instead (RD-191-09 T08).
portable() {
    local packages=(-p rdownloader) portable_dir
    if [[ "${RUNNER_OS}" == "macOS" ]]; then packages+=(-p rd-capture); fi
    cargo build --locked "${packages[@]}"
    portable_dir="$(mktemp -d)"
    cp target/debug/rdownloader "${portable_dir}/"
    if [[ "${RUNNER_OS}" == "Linux" ]]; then
        cp scripts/linux/start-rdownloader.sh scripts/linux/stop-rdownloader.sh "${portable_dir}/"
        start_script="${portable_dir}/start-rdownloader.sh"
        stop_script="${portable_dir}/stop-rdownloader.sh"
    else
        cp scripts/macos/start-rdownloader.command scripts/macos/stop-rdownloader.command "${portable_dir}/"
        start_script="${portable_dir}/start-rdownloader.command"
        stop_script="${portable_dir}/stop-rdownloader.command"
    fi
    "${start_script}" server
    trap '"${stop_script}" server' EXIT
    curl --fail --silent --show-error --retry 10 --retry-delay 1 \
        http://127.0.0.1:8710/api/v1/health
    "${stop_script}" server
    trap - EXIT
}

# RD-180-06: the Homebrew formula as the tap gets it, rendered by scripts/package-managers.sh —
# but over an archive in the release layout made from this tree's debug build instead of a
# published release, so what is tested is this tree's formula. Install from a local tap, `brew
# test`, the service through `brew services` up to /api/v1/health, uninstall. The capture agent's
# formula is installed and tested too, never started: the runner has no login session for its
# tray. The tap is rendered as rdownloader-ci's, so its dependency on
# <owner>/rdownloader/rdownloader names this local tap instead of cloning the published one.
homebrew() {
    local version arch fixture other
    version="$(scripts/set-version.sh)"
    arch="$(uname -m)"
    [[ "${arch}" == arm64 ]] && arch=aarch64
    fixture="${RUNNER_TEMP}/brew-fixture"
    mkdir -p "${fixture}/stage/plugins"
    cp target/debug/rdownloader target/debug/rdownloader-capture LICENSE README.md "${fixture}/stage/"
    osacompile -o "${fixture}/stage/rDownloader Capture.app" resources/macos/rdownloader-capture.applescript
    COPYFILE_DISABLE=1 tar --directory "${fixture}/stage" --create --gzip \
        --file "${fixture}/rdownloader-macos-${arch}.tar.gz" .
    # The generator wants every archive of a release; the others are never downloaded here.
    (cd "${fixture}" && shasum -a 256 "./rdownloader-macos-${arch}.tar.gz") > "${fixture}/SHA256SUMS"
    for other in macos-x86_64.tar.gz macos-aarch64.tar.gz linux-x86_64.tar.gz linux-aarch64.tar.gz windows-x86_64.zip; do
        grep -q "rdownloader-${other}" "${fixture}/SHA256SUMS" \
            || printf '%064d  ./rdownloader-%s\n' 0 "${other}" >> "${fixture}/SHA256SUMS"
    done
    scripts/package-managers.sh "${version}" "${fixture}/SHA256SUMS" "${fixture}/out" \
        --repository rdownloader-ci/rDownloader --base-url "file://${fixture}"
    brew tap-new --no-git rdownloader-ci/rdownloader
    cp "${fixture}/out/rdownloader.rb" "${fixture}/out/rdownloader-capture.rb" \
        "$(brew --repository rdownloader-ci/rdownloader)/Formula/"
    brew install rdownloader-ci/rdownloader/rdownloader
    brew test rdownloader-ci/rdownloader/rdownloader
    brew install rdownloader-ci/rdownloader/rdownloader-capture
    brew test rdownloader-ci/rdownloader/rdownloader-capture
    log="$(brew --prefix)/var/log/rdownloader.log"
    brew services start rdownloader
    trap 'brew services stop rdownloader || true; cat "${log}" || true' EXIT
    curl --fail --silent --show-error --retry 30 --retry-delay 1 --retry-connrefused \
        http://127.0.0.1:8710/api/v1/health
    test -f "$(brew --prefix)/var/rdownloader/data/rdownloader.sqlite3"
    brew services stop rdownloader
    trap - EXIT
    brew uninstall rdownloader-capture rdownloader
}

# The middle of the Scoop check (RD-180-06), between the zip and the install that
# scripts/ci-platform-smoke.ps1 make: the generator is bash, Scoop is PowerShell.
scoop_manifest() {
    local fixture="${RUNNER_TEMP}/scoop-fixture" other
    (cd "${fixture}" && sha256sum ./rdownloader-windows-x86_64.zip) > "${fixture}/SHA256SUMS"
    # The generator wants every archive of a release; the others are never downloaded here.
    for other in macos-x86_64 macos-aarch64 linux-x86_64 linux-aarch64; do
        printf '%064d  ./rdownloader-%s.tar.gz\n' 0 "${other}" >> "${fixture}/SHA256SUMS"
    done
    scripts/package-managers.sh "$(scripts/set-version.sh)" "${fixture}/SHA256SUMS" \
        "${fixture}/out" --base-url http://127.0.0.1:8765
}

# The `docker` job's optimised binary through the portable Linux launchers to /api/v1/health
# before its image is built; on x86_64 that repeats `portable`'s debug-build check with it. The
# arm64 leg is the only run that starts a Linux aarch64 binary (RD-190-12): before it, the aarch64
# tarball and the arm64 image were built and shipped but never started.
optimised() {
    file target/release-test/rdownloader
    portable_dir="$(mktemp -d)"
    cp target/release-test/rdownloader scripts/linux/start-rdownloader.sh \
        scripts/linux/stop-rdownloader.sh "${portable_dir}/"
    "${portable_dir}/start-rdownloader.sh" server
    trap '"${portable_dir}/stop-rdownloader.sh" server' EXIT
    curl --fail --silent --show-error --retry 30 --retry-delay 1 --retry-connrefused \
        http://127.0.0.1:8710/api/v1/health
    echo
    "${portable_dir}/stop-rdownloader.sh" server
    trap - EXIT
}

case "${1:-}" in
    launcher-syntax) launcher_syntax ;;
    macos-helper) macos_helper ;;
    portable) portable ;;
    homebrew) homebrew ;;
    scoop-manifest) scoop_manifest ;;
    optimised) optimised ;;
    *)
        echo "usage: scripts/ci-platform-smoke.sh launcher-syntax|macos-helper|portable|homebrew|scoop-manifest|optimised" >&2
        exit 2
        ;;
esac
