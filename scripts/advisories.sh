#!/usr/bin/env bash
#
# Known vulnerabilities in the Python tools of the container image (docker/requirements.txt) and
# in the npm packages of the web interface (web/pnpm-lock.yaml), asked of OSV (https://osv.dev)
# once a week by .github/workflows/advisories.yml (RD-1480-14, SEC-5). The crates are cargo-deny's,
# in the same workflow. Until 1.47 nothing watched the Python tools, which read what strangers
# serve: yt-dlp, requests, urllib3, lxml.
#
#   scripts/advisories.sh            # ask; exit 1 with a finding, 2 when OSV does not answer
#   scripts/advisories.sh --list     # what would be asked for, `<ecosystem> <name> <version>`
#
# Needs the network and nothing but Python: no tool from PyPI judges the tools from PyPI. Asked
# are names and versions of public packages. A finding is fixed with `scripts/docker-tools.sh
# --bump` or an npm update, or waived in scripts/advisories-allow.txt, one `<id> <reason>` a line,
# the id or one of its aliases (CVE, GHSA, PYSEC). A waiver nothing matches any more fails as well,
# so the list only shrinks.
#
# Not run by check.sh: the answer changes without the tree.
#
# RD_OSV_URL, RD_ADVISORIES_ROOT: another server and another checkout, for the test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "$#:${1:-}" in
    0:) mode=ask ;;
    1:--list) mode=list ;;
    *) echo "usage: scripts/advisories.sh [--list]" >&2; exit 2 ;;
esac
exec python3 "$ROOT/scripts/lib/advisories.py" "${RD_ADVISORIES_ROOT:-$ROOT}" \
    "${RD_OSV_URL:-https://api.osv.dev}" "$mode"
