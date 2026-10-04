#!/usr/bin/env bash
#
# Builds the Windows release from WSL and assembles artifacts/windows plus the distributable
# zip. Everything the package needs that is *not* built here — the vendored helper binaries in
# artifacts/windows/vendor — is left untouched, because those are downloaded third-party tools
# rather than build output. They stay in the folder: the zip has the release layout
# (scripts/lib/archive-layout.sh), flat and with the files release.yml packs.
#
# Usage:
#   scripts/package-windows.sh              # full run
#   scripts/package-windows.sh --skip-web   # reuse the existing web/dist
#   scripts/package-windows.sh --profile release-test   # a test package, built much faster
#   JOBS=2 scripts/package-windows.sh       # lower the build parallelism further
#
# A Windows package is what gets installed and run, so it needs a `scripts/check.sh --full`
# green on this content, documentation changes excepted (RD-120-58); a branch green is scoped
# and does not count.
# RD_UNVERIFIED_PACKAGE=1 builds one anyway, for testing the cross-build itself — loudly, and
# with UNVERIFIED.txt inside the package so it cannot pass for a verified one.
#
set -euo pipefail

TARGET="x86_64-pc-windows-msvc"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Deliberately capped and not derived from nproc; the reason is in the library.
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
rd_take_lock "$@"
OUT="$ROOT/artifacts/windows"
ZIP="$ROOT/artifacts/rdownloader-windows-x86_64.zip"

cd "$ROOT"

# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
unverified=0
if ! rd_full_gate "$ROOT" "the Windows package"; then
    if [[ "${RD_UNVERIFIED_PACKAGE:-0}" != 1 ]]; then
        echo "   (RD_UNVERIFIED_PACKAGE=1 builds an unverified package, never one for the owner)" >&2
        exit 1
    fi
    unverified=1
    echo "!! ================================================================================" >&2
    echo "!! RD_UNVERIFIED_PACKAGE=1: building WITHOUT a --full green. Not for the owner." >&2
    echo "!! ================================================================================" >&2
fi

# shellcheck source=lib/package.sh
source "$ROOT/scripts/lib/package.sh"
rd_package_args "$@"
rd_package_version
echo "==> packaging rDownloader $version for $TARGET (profile: $profile, jobs: $JOBS)"

rd_package_web

echo "==> cross-building the binaries"
# shellcheck source=lib/version-file.sh
source "$ROOT/scripts/lib/version-file.sh"
# Before the build, so the binary and VERSION.txt carry the same commit and time (RD-130-12).
rd_build_stamp "$version"
# The build directory: the checkout's target, or the lane the release chain gives this step
# (RD_LANE_TARGET_DIR, scripts/lib/lanes.sh) so it can build beside the other package.
build_dir="$(rd_build_dir "$ROOT")"
CARGO_TARGET_DIR="$build_dir" CARGO_BUILD_JOBS="$JOBS" cargo xwin build --locked --profile "$profile" --target "$TARGET" \
    -j "$JOBS" -p rdownloader -p rd-capture

binaries="$build_dir/$TARGET/$profile"
mkdir -p "$OUT/plugins"

echo "==> assembling $OUT"
install -m 755 "$binaries/rdownloader.exe" "$OUT/rdownloader.exe"
install -m 755 "$binaries/rdownloader-capture.exe" "$OUT/rdownloader-capture.exe"
install -m 644 README.md LICENSE "$OUT/"
rd_write_version_file "$OUT" "$version" "windows x86_64" "$profile"
install -m 644 scripts/windows/start-rdownloader.bat scripts/windows/stop-rdownloader.bat \
    scripts/windows/start-capture.bat scripts/windows/stop-capture.bat "$OUT/"
rm -f "$OUT/UNVERIFIED.txt"
if [[ "$unverified" -eq 1 ]]; then
    printf '%s\n' "Built from $(git rev-parse --short HEAD) without a check.sh --full green." \
        "Not a package for the owner's instance (RD-120-58)." \
        "Cargo profile: $profile." > "$OUT/UNVERIFIED.txt"
fi

rd_package_plugins "$OUT"

if [[ ! -d "$OUT/vendor" ]]; then
    echo "    vendor: $OUT/vendor is missing; the package will have no helper binaries" >&2
else
    echo "    vendor: $(ls -1 "$OUT/vendor" | wc -l) entries kept"
    # The installer's 7z.exe loads every archive format from 7z.dll beside it; without the DLL it
    # opens nothing, and until 1.3 the package shipped exactly that (RD-130-12).
    if [[ -f "$OUT/vendor/7z.exe" && ! -f "$OUT/vendor/7z.dll" ]]; then
        echo "!! $OUT/vendor has 7z.exe but no 7z.dll; 7z.exe opens no archive without it." >&2
        echo "   Take 7z.dll from the same official 7-Zip installer as 7z.exe." >&2
        exit 1
    fi
    # The tools are downloaded, their licences are not: they come from the repository, so every
    # package carries the same texts the About page names (RD-130-12). Until 1.3 the Windows
    # package carried none at all.
    install -d "$OUT/vendor/licenses"
    # Shared texts plus the ones whose wording differs per platform build: 7-Zip's Windows
    # installer carries its own License.txt, which is the one for the 7z.exe shipped here.
    install -m 644 resources/vendor-licenses/*.txt resources/vendor-licenses/windows/*.txt \
        "$OUT/vendor/licenses/"
    echo "    vendor licences: $(ls -1 "$OUT/vendor/licenses" | wc -l)"
fi

echo "==> writing $ZIP"
# The entries by name, as for the Linux tarball; UNVERIFIED.txt goes with them when it exists.
# shellcheck source=lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"
entries=()
while IFS= read -r entry; do entries+=("$entry"); done < <(rd_archive_entries windows)
[[ -f "$OUT/UNVERIFIED.txt" ]] && entries+=(UNVERIFIED.txt)
rm -f "$ZIP"
(cd "$OUT" && zip -qr "$ZIP" "${entries[@]}" plugins)
rd_check_archive_layout "$ZIP" windows

echo "==> done"
ls -la "$OUT/rdownloader.exe" "$OUT/rdownloader-capture.exe" "$ZIP"
