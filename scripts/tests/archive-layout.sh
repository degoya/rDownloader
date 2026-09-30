#!/usr/bin/env bash
#
# scripts/lib/archive-layout.sh on scratch archives (RD-180-05): an archive packed flat as
# release.yml packs it passes for every platform, and the layouts the local scripts wrote until
# 1.8 fail — a folder above the files, vendor/ inside, no VERSION.txt — as does one without
# plugins or with a stray file under plugins/.
#
#   scripts/tests/archive-layout.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"

# A stage with every entry of a platform and one signed package, as release.yml lays it out.
stage() {
    local platform="$1" directory="$2" entry
    mkdir -p "$directory/plugins"
    while IFS= read -r entry; do
        if [[ "$entry" == *.app ]]; then
            mkdir -p "$directory/$entry/Contents"
            echo plist > "$directory/$entry/Contents/Info.plist"
        else
            echo "$entry" > "$directory/$entry"
        fi
    done < <(rd_archive_entries "$platform")
    echo package > "$directory/plugins/http-1.0.0.rdplug"
}

for platform in linux macos; do
    stage "$platform" "$SCRATCH/$platform"
    tar --directory "$SCRATCH/$platform" --create --gzip --file "$SCRATCH/$platform.tar.gz" .
    run_status rd_check_archive_layout "$SCRATCH/$platform.tar.gz" "$platform"
    expect_status "a flat $platform tarball as release.yml packs it" 0
done

stage windows "$SCRATCH/windows"
(cd "$SCRATCH/windows" && zip -qr "$SCRATCH/windows.zip" .)
run_status rd_check_archive_layout "$SCRATCH/windows.zip" windows
expect_status "a flat Windows zip" 0

echo "not verified" > "$SCRATCH/windows/UNVERIFIED.txt"
(cd "$SCRATCH/windows" && zip -qr "$SCRATCH/unverified.zip" .)
run_status rd_check_archive_layout "$SCRATCH/unverified.zip" windows
expect_status "a local package marked UNVERIFIED.txt keeps its mark" 0

# The layout package-linux.sh wrote until 1.8: everything below linux/.
tar --directory "$SCRATCH" --create --gzip --file "$SCRATCH/nested.tar.gz" linux
run_status rd_check_archive_layout "$SCRATCH/nested.tar.gz" linux
expect_status "a tarball with a folder above the files" 1
expect_output "naming the missing top-level entry" "lacks rdownloader at its top level"
expect_output "and the folder" "carries linux, which no release archive does"

mkdir -p "$SCRATCH/linux/vendor"
echo tool > "$SCRATCH/linux/vendor/yt-dlp"
tar --directory "$SCRATCH/linux" --create --gzip --file "$SCRATCH/vendor.tar.gz" .
run_status rd_check_archive_layout "$SCRATCH/vendor.tar.gz" linux
expect_status "a tarball carrying vendor/" 1
expect_output "naming vendor" "carries vendor"
rm -rf "$SCRATCH/linux/vendor"

rm "$SCRATCH/linux/VERSION.txt"
tar --directory "$SCRATCH/linux" --create --gzip --file "$SCRATCH/unversioned.tar.gz" .
run_status rd_check_archive_layout "$SCRATCH/unversioned.tar.gz" linux
expect_status "a tarball without VERSION.txt, as CI packed them until 1.8" 1
expect_output "naming VERSION.txt" "lacks VERSION.txt"
echo version > "$SCRATCH/linux/VERSION.txt"

echo notes > "$SCRATCH/linux/plugins/README"
tar --directory "$SCRATCH/linux" --create --gzip --file "$SCRATCH/stray.tar.gz" .
run_status rd_check_archive_layout "$SCRATCH/stray.tar.gz" linux
expect_status "a stray file under plugins/" 1
rm "$SCRATCH/linux/plugins/README" "$SCRATCH/linux/plugins/http-1.0.0.rdplug"
tar --directory "$SCRATCH/linux" --create --gzip --file "$SCRATCH/bare.tar.gz" .
run_status rd_check_archive_layout "$SCRATCH/bare.tar.gz" linux
expect_status "a tarball without a signed plugin" 1
expect_output "naming the plugins" "carries no plugins/*.rdplug"

run_status rd_check_archive_layout "$SCRATCH/absent.tar.gz" linux
expect_status "an archive that is not there" 1

finish_tests archive-layout
