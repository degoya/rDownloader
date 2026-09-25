#!/usr/bin/env bash
#
# Builds the Linux release and assembles artifacts/linux plus the distributable tarball, and
# puts the verified site-rule file beside it as artifacts/rdownloader-site-rules.json.
# Mirrors scripts/package-windows.sh; see the comments there for why the job count is capped
# and why artifacts/linux/vendor is left alone.
#
# Usage:
#   scripts/package-linux.sh
#   scripts/package-linux.sh --skip-web
#   JOBS=2 scripts/package-linux.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
rd_take_lock "$@"
OUT="$ROOT/artifacts/linux"
TARBALL="$ROOT/artifacts/rdownloader-linux-x86_64.tar.gz"

cd "$ROOT"

skip_web=0
for argument in "$@"; do
    case "$argument" in
        --skip-web) skip_web=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

version="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
echo "==> packaging rDownloader ${version:?version not found in Cargo.toml} for linux (jobs: $JOBS)"

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

echo "==> building the binaries"
# shellcheck source=lib/version-file.sh
source "$ROOT/scripts/lib/version-file.sh"
# Before the build, so the binary and VERSION.txt carry the same commit and time (RD-130-12).
rd_build_stamp "$version"
CARGO_BUILD_JOBS="$JOBS" cargo build --locked --release -j "$JOBS" -p rdownloader -p rd-capture

# Respects CARGO_TARGET_DIR, the same derivation check.sh uses for its checkout marker.
binaries="${CARGO_TARGET_DIR:-$ROOT/target}/release"
mkdir -p "$OUT/plugins"

echo "==> assembling $OUT"
install -m 755 "$binaries/rdownloader" "$OUT/rdownloader"
install -m 755 "$binaries/rdownloader-capture" "$OUT/rdownloader-capture"
install -m 644 README.md LICENSE "$OUT/"
rd_write_version_file "$OUT" "$version" "linux x86_64"
install -m 755 scripts/linux/start-rdownloader.sh scripts/linux/stop-rdownloader.sh "$OUT/"

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
    # The tools are downloaded, their licences are not: they come from the repository, so every
    # package carries the same texts the About page names (RD-130-12).
    install -d "$OUT/vendor/licenses"
    # Shared texts plus the ones whose wording differs per platform build (7-Zip).
    install -m 644 resources/vendor-licenses/*.txt resources/vendor-licenses/linux/*.txt \
        "$OUT/vendor/licenses/"
    echo "    vendor licences: $(ls -1 "$OUT/vendor/licenses" | wc -l)"
fi

# RD-130-07: the project's site rules are not compiled in; every release carries them as a
# signed file beside the packages. It is signed locally with the site-rules key and committed
# (`rdownloader site-rules sign`), so this only proves the committed file verifies under the
# root of the binary just built — the same check the import runs — and puts it next to the
# tarball. A file that does not verify stops the package rather than reaching a person.
SITE_RULES="$ROOT/artifacts/rdownloader-site-rules.json"
echo "==> verifying the site-rule file"
"$OUT/rdownloader" site-rules verify crates/rd-siterules/resources/site-rules.json
install -m 644 crates/rd-siterules/resources/site-rules.json "$SITE_RULES"

echo "==> writing $TARBALL"
rm -f "$TARBALL"
tar -czf "$TARBALL" -C "$OUT/.." linux

echo "==> done"
ls -la "$OUT/rdownloader" "$OUT/rdownloader-capture" "$TARBALL" "$SITE_RULES"
