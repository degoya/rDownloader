#!/usr/bin/env bash
#
# scripts/plugin-release-notes.sh against a scratch changelog (RD-160-09): the entries that name
# a plugin version become its notes, as plain text, and nothing else does; --missing on a scratch
# repository names the plugins raised since a tag that no entry names, grouped by version.
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

# --missing: a scratch repository with five plugins at the tag, then raised, one added.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts"
cp "$ROOT/scripts/plugin-release-notes.sh" "$TREE/scripts/"
manifest() {
    mkdir -p "$TREE/plugins/$1"
    printf 'manifest_version = 3\nname = "%s"\nversion = "%s"\n\n[tool]\nversion = "9.9.9"\n' \
        "$1" "$2" > "$TREE/plugins/$1/manifest.toml"
}
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
git init -q -b development "$TREE"
for plugin in alpha bravo charlie delta foxtrot; do manifest "$plugin" 0.1.0; done
printf '# Changelog\n' > "$TREE/CHANGELOG.md"
git -C "$TREE" add -A && git -C "$TREE" commit -q -m base && git -C "$TREE" tag v1.0.0
manifest alpha 0.2.0
manifest bravo 0.2.0
manifest charlie 0.1.1
manifest echo 0.1.1
manifest foxtrot 0.1.1
cat >> "$TREE/CHANGELOG.md" <<'EOF'

## [Unreleased]

- **Bravo signs in again.** `bravo` 0.2.0.
EOF
missing() { "$TREE/scripts/plugin-release-notes.sh" --missing "$@"; }

run_status missing v1.0.0 alpha bravo charlie delta echo foxtrot
expect_status "raised plugins without an entry: exit 1" 1
expect "grouped by version, in the changelog's notation; a new plugin counts, an unchanged one not" \
    '`charlie`, `echo` and `foxtrot` 0.1.1
`alpha` 0.2.0' "$(missing v1.0.0 alpha bravo charlie delta echo foxtrot 2> /dev/null)"
expect_output "and says what the lines are for" "4 plugin versions raised since v1.0.0 have no entry"

printf -- '- Rebuilt. `charlie`, `echo` and `foxtrot` 0.1.1; `alpha` 0.2.0.\n' >> "$TREE/CHANGELOG.md"
run_status missing v1.0.0 alpha bravo charlie delta echo foxtrot
expect_status "every raised plugin named: exit 0" 0
expect "and nothing to add" "" "$(missing v1.0.0 alpha bravo charlie delta echo foxtrot 2> /dev/null)"
expect "the lines it printed are notes now" "Rebuilt. charlie, echo and foxtrot 0.1.1; alpha 0.2.0." \
    "$("$TREE/scripts/plugin-release-notes.sh" echo 0.1.1 "$TREE/CHANGELOG.md")"

run_status missing v0.0.0 alpha
expect_status "an unknown tag is a usage error" 2
run_status missing
expect_status "--missing without a tag is a usage error" 2

finish_tests plugin-release-notes
