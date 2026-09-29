#!/usr/bin/env bash
#
# scripts/plugin-release-notes.sh against a scratch changelog (RD-160-09): the entries that name
# a plugin version become its notes, as plain text, and nothing else does.
#
#   scripts/tests/plugin-release-notes.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# Characters, not bytes: the ellipsis is three bytes.
export LC_ALL=C.UTF-8

CHANGELOG="$SCRATCH/CHANGELOG.md"
cat > "$CHANGELOG" <<'EOF'
# Changelog

## [1.5.2] - 2026-09-28

### Fixed

- **The link check no longer waits (RD-150-09).** One request per check instead of one per
  link, see [the job](https://example.test/job). `realdebrid` 0.2.2.

- **The sign-in finishes.** The token request sends a form body.
  `realdebrid-auth` 0.2.1.

## [1.5.0] - 2026-09-28

### Added

- **Torrents through Real-Debrid.** `realdebrid-torrents` 0.2.0, and `realdebrid` 0.2.21 for
  a version that only starts like the one asked for.

- The account page names the plan. `realdebrid` 0.2.2

- Containers keep their names. `premiumize-transfers`, `alldebrid` and `torbox-jobs` 0.2.2.
EOF
notes() { "$ROOT/scripts/plugin-release-notes.sh" "$1" "$2" "$CHANGELOG"; }

expect "one entry, plain text, wrapped lines joined" \
    "The sign-in finishes. The token request sends a form body. realdebrid-auth 0.2.1." \
    "$(notes realdebrid-auth 0.2.1)"
expect "every entry naming the version, one line each" \
    "- The link check no longer waits (RD-150-09). One request per check instead of one per link, see the job. realdebrid 0.2.2.
- The account page names the plan. realdebrid 0.2.2" \
    "$(notes realdebrid 0.2.2)"
expect "a name in a list of names" \
    "Containers keep their names. premiumize-transfers, alldebrid and torbox-jobs 0.2.2." \
    "$(notes alldebrid 0.2.2)"
expect "the last name of the list" \
    "Containers keep their names. premiumize-transfers, alldebrid and torbox-jobs 0.2.2." \
    "$(notes torbox-jobs 0.2.2)"
expect "a longer version is another version" "" "$(notes realdebrid 0.2.1)"
expect "a version nobody named has no notes" "" "$(notes ddownload 0.10.13)"
expect "the name is the whole name" "" "$(notes debrid 0.2.2)"

LONG="$SCRATCH/LONG.md"
printf -- '- %s `long` 1.0.0.\n' "$(printf '%2500s' '' | tr ' ' x)" > "$LONG"
long="$("$ROOT/scripts/plugin-release-notes.sh" long 1.0.0 "$LONG")"
expect "an overlong entry is cut to the index limit" "2000" "${#long}"
expect "and says so" "…" "${long: -1}"

run_status "$ROOT/scripts/plugin-release-notes.sh" realdebrid
expect_status "a missing version is a usage error" 2
run_status "$ROOT/scripts/plugin-release-notes.sh" realdebrid 0.2.2 "$SCRATCH/none.md"
expect_status "a missing changelog is an error" 2

finish_tests plugin-release-notes
