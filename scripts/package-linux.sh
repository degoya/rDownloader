#!/usr/bin/env bash
#
# Builds the Linux release and assembles artifacts/linux plus the distributable tarball, and
# puts the verified site-rule file beside it as artifacts/rdownloader-site-rules.json.
# Mirrors scripts/package-windows.sh; see the comments there for why the job count is capped
# and why artifacts/linux/vendor is left alone. The tarball has the release layout
# (scripts/lib/archive-layout.sh): flat, the files release.yml packs, vendor/ not among them.
#
# Usage:
#   scripts/package-linux.sh
#   scripts/package-linux.sh --skip-web
#   scripts/package-linux.sh --profile release-test   # a test package, built much faster
#   JOBS=2 scripts/package-linux.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
# A worktree's own web/dist would not be what the binary embeds (scripts/lib/web-dist.sh).
# shellcheck source=lib/web-dist.sh
source "$ROOT/scripts/lib/web-dist.sh"
rd_web_dist_guard "$ROOT" "the package" || exit 2
rd_take_lock "$@"
OUT="$ROOT/artifacts/linux"
TARBALL="$ROOT/artifacts/rdownloader-linux-x86_64.tar.gz"

cd "$ROOT"

# shellcheck source=lib/package.sh
source "$ROOT/scripts/lib/package.sh"
rd_package_args "$@"
rd_package_version
echo "==> packaging rDownloader $version for linux (profile: $profile, jobs: $JOBS)"

rd_package_web

echo "==> building the binaries"
# shellcheck source=lib/version-file.sh
source "$ROOT/scripts/lib/version-file.sh"
# Before the build, so the binary and VERSION.txt carry the same commit and time (RD-130-12).
rd_build_stamp "$version"
# The build directory: the checkout's target, or the lane the release chain gives this step
# (RD_LANE_TARGET_DIR, scripts/lib/lanes.sh) so it can build beside the other package.
build_dir="$(rd_build_dir "$ROOT")"
CARGO_TARGET_DIR="$build_dir" CARGO_BUILD_JOBS="$JOBS" cargo build --locked --profile "$profile" -j "$JOBS" \
    -p rdownloader -p rd-capture

binaries="$build_dir/$profile"
mkdir -p "$OUT/plugins"

echo "==> assembling $OUT"
install -m 755 "$binaries/rdownloader" "$OUT/rdownloader"
install -m 755 "$binaries/rdownloader-capture" "$OUT/rdownloader-capture"
install -m 644 README.md LICENSE "$OUT/"
rd_write_version_file "$OUT" "$version" "linux x86_64" "$profile"
install -m 755 scripts/linux/start-rdownloader.sh scripts/linux/stop-rdownloader.sh \
    scripts/linux/start-capture.sh scripts/linux/stop-capture.sh "$OUT/"

rd_package_plugins "$OUT"

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
# The entries by name, not the folder: whatever else sits in artifacts/linux (vendor/, the logs
# and data of a test run) stays out, and the archive unpacks as the published one does.
# shellcheck source=lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"
entries=()
while IFS= read -r entry; do entries+=("./$entry"); done < <(rd_archive_entries linux)
rm -f "$TARBALL"
tar -czf "$TARBALL" -C "$OUT" "${entries[@]}" ./plugins
rd_check_archive_layout "$TARBALL" linux

echo "==> done"
ls -la "$OUT/rdownloader" "$OUT/rdownloader-capture" "$TARBALL" "$SITE_RULES"
