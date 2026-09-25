#!/usr/bin/env bash
#
# Builds the Windows release from WSL and assembles artifacts/windows plus the distributable
# zip. Everything the package needs that is *not* built here — the vendored helper binaries in
# artifacts/windows/vendor — is left untouched, because those are downloaded third-party tools
# rather than build output.
#
# Usage:
#   scripts/package-windows.sh              # full run
#   scripts/package-windows.sh --skip-web   # reuse the existing web/dist
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

skip_web=0
for argument in "$@"; do
    case "$argument" in
        --skip-web) skip_web=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

version="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
if [[ -z "$version" ]]; then
    echo "could not read the workspace version from Cargo.toml" >&2
    exit 1
fi
echo "==> packaging rDownloader $version for $TARGET (jobs: $JOBS)"

# The web UI is embedded at compile time via rust-embed, so it has to exist before cargo runs.
if [[ "$skip_web" -eq 0 ]]; then
    echo "==> type-checking and building the web UI"
    # `npm run build` was `vue-tsc --build --force && vite build` until RD-120-25.
    # The type check is now its own script, so it has to be named here to keep the
    # release chain checking exactly what it checked before.
    npm run typecheck:full --prefix web
    npm run build --prefix web
elif ! scripts/web-dist-stale.sh; then
    echo "--skip-web was given but web/dist is not current; drop the flag" >&2
    exit 1
fi

echo "==> cross-building the binaries"
# shellcheck source=lib/version-file.sh
source "$ROOT/scripts/lib/version-file.sh"
# Before the build, so the binary and VERSION.txt carry the same commit and time (RD-130-12).
rd_build_stamp "$version"
CARGO_BUILD_JOBS="$JOBS" cargo xwin build --locked --release --target "$TARGET" \
    -j "$JOBS" -p rdownloader -p rd-capture

# Respects CARGO_TARGET_DIR, the same derivation check.sh uses for its checkout marker.
binaries="${CARGO_TARGET_DIR:-$ROOT/target}/$TARGET/release"
mkdir -p "$OUT/plugins"

echo "==> assembling $OUT"
install -m 755 "$binaries/rdownloader.exe" "$OUT/rdownloader.exe"
install -m 755 "$binaries/rdownloader-capture.exe" "$OUT/rdownloader-capture.exe"
install -m 644 README.md LICENSE "$OUT/"
rd_write_version_file "$OUT" "$version" "windows x86_64"
install -m 644 scripts/windows/start-rdownloader.bat scripts/windows/stop-rdownloader.bat "$OUT/"
rm -f "$OUT/UNVERIFIED.txt"
if [[ "$unverified" -eq 1 ]]; then
    printf '%s\n' "Built from $(git rev-parse --short HEAD) without a check.sh --full green." \
        "Not a package for the owner's instance (RD-120-58)." > "$OUT/UNVERIFIED.txt"
fi

# Signed plugins come from dist/plugins, which is what the signing step writes. Replaced as a
# set rather than merged, so a plugin dropped from the bundle does not linger in the package.
if compgen -G "dist/plugins/*.rdplug" > /dev/null; then
    rm -f "$OUT"/plugins/*.rdplug
    install -m 644 dist/plugins/*.rdplug "$OUT/plugins/"
    echo "    plugins: $(ls -1 "$OUT"/plugins/*.rdplug | wc -l)"
    # A package silently short of a plugin looks fine until somebody misses the feature. The
    # expected number is simply how many plugin directories carry a manifest.
    # Not simply every manifest: a plugin demanding a newer application version than this
    # build cannot be packaged yet, and build-plugins.sh owns that rule.
    expected="$("$ROOT/scripts/build-plugins.sh" --list-packageable | wc -l)"
    actual="$(ls -1 "$OUT"/plugins/*.rdplug | wc -l)"
    if [[ "$expected" -ne "$actual" ]]; then
        echo "!! $actual packaged, but $expected plugins have a manifest." >&2
        echo "   run scripts/build-plugins.sh — a missing one is usually never built here." >&2
        exit 1
    fi
else
    echo "    plugins: dist/plugins holds no .rdplug — leaving the packaged set as it is" >&2
fi

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
rm -f "$ZIP"
(cd "$OUT/.." && zip -qr "$ZIP" windows)

echo "==> done"
ls -la "$OUT/rdownloader.exe" "$OUT/rdownloader-capture.exe" "$ZIP"
