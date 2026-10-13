# shellcheck shell=bash
#
# The capture agent's macOS helper app, `rDownloader Capture.app` (RD-1280-15): compiled from its
# AppleScript, its Info.plist stamped with the workspace version and linted. One copy for
# scripts/release-binary.sh (which signs it ad hoc) and scripts/ci-platform-smoke.sh, which had
# each their own, reading Cargo.toml with an inline `sed` instead of lib/workspace-version.sh.
# macOS only; run from the checkout root.
#
#   source "$ROOT/scripts/lib/macos-helper.sh"
#   rd_macos_capture_helper <dir> [--sign]
#   rd_bundle_version < Cargo.toml          # 1.8.0-beta.2 -> 1.8.0

# shellcheck source=workspace-version.sh
source "$(dirname "${BASH_SOURCE[0]}")/workspace-version.sh"

# The workspace version on stdin as a bundle version: Apple allows digits and periods only in
# both keys (at most three integers), so a beta's `-beta.N` stays out — 1.8.0-beta.2 is 1.8.0
# here, as in the browser manifest.
rd_bundle_version() {
    local version
    version="$(rd_workspace_version)"
    printf '%s\n' "${version%%[-+]*}"
}

# Builds the helper into directory $1; `--sign` as $2 signs it ad hoc, as the release does.
rd_macos_capture_helper() {
    local dir="$1" sign="${2:-}" app plist version
    app="$dir/rDownloader Capture.app"
    plist="$app/Contents/Info.plist"
    osacompile -o "$app" resources/macos/rdownloader-capture.applescript
    cp resources/macos/capture-helper-Info.plist "$plist"
    version="$(rd_bundle_version < Cargo.toml)"
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "$plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "$plist"
    plutil -lint "$plist"
    if [[ "$sign" == --sign ]]; then
        codesign --force --deep --sign - "$app"
    fi
}
