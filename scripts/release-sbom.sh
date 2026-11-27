#!/usr/bin/env bash
#
# The release's SBOM (RD-1480-14, LIC-5): SPDX 2.3 as JSON, of the crates and npm packages the
# release files are made of, with their versions, checksums and licences. Up to 1.47 Syft scanned
# the directory of packed archives, and the binaries carry no dependency data for it to read.
#
#   scripts/release-sbom.sh <output>
#
# Read from files, no network and no build: Cargo.lock and web/pnpm-lock.yaml say which version
# with which checksum, crates/rd-api/licenses/third-party.json (scripts/licenses.sh) which of them
# ship and under which licence. A crate the licence list does not know stays in, its licence
# unknown: the SBOM rather names one too many. The same checkout gives the same bytes: the time is
# SOURCE_DATE_EPOCH, else the commit's.
#
# Exit 1 with fewer packages than the floors below: an SBOM that lost its content stops the
# release instead of being published. Exit 1 too when the licence list names an npm package the
# lock file no longer holds (RD-1490-10): the list is stale and the SBOM would miss what replaced
# it. RD_SBOM_STALE_LIST=warn makes that a warning, for a wave's preflight, which runs before the
# generators that renew the list (scripts/tests/release-sbom.sh). Runs in release.yml's `publish`. Left out: the container
# image's Debian and Python packages (BuildKit's own SBOM on the image) and the programs a user
# puts into the vendor folder.
#
# RD_SBOM_ROOT names another checkout, for the test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# 645 crates and 329 npm packages on 2026-11-25. Lower them only with the reason in the commit.
MIN_CRATES=500
MIN_NPM=250

if [[ $# -ne 1 || "$1" == -* ]]; then
    echo "usage: scripts/release-sbom.sh <output>" >&2
    exit 2
fi

epoch="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct)}"
exec python3 "$ROOT/scripts/lib/release-sbom.py" "${RD_SBOM_ROOT:-$ROOT}" "$1" "$epoch" \
    "$MIN_CRATES" "$MIN_NPM"
