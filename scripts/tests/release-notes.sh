#!/usr/bin/env bash
#
# scripts/release-notes.sh against a scratch checkout (RD-1150-02): the version's section of
# RELEASE-NOTES.md becomes its points for users, as plain text; --anchor is GitHub's anchor of the
# CHANGELOG heading; --check refuses a section that names a job, a path or code, runs long or
# mixes points with prose, and with --version a release without its section or with a draft.
#
#   scripts/tests/release-notes.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# Characters, not bytes.
export LC_ALL=C.UTF-8

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts"
cp "$ROOT/scripts/release-notes.sh" "$TREE/scripts/"
cat > "$TREE/RELEASE-NOTES.md" <<'EOF'
# Release notes

<!--
How a section is written; "## X.Y.Z" in a comment is no section.
-->

## 2.1.0

<!-- draft: completed at the release -->

- Something new.

## 2.0.0

- Downloads from file hosters run **in parallel**, see
  [the handbook](https://example.test/handbook).
- The Updates page says what is new.

## 1.9.1

Maintenance release: internal changes only, no change in behaviour.
EOF
cat > "$TREE/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

## [2.0.0] - 2026-10-10

### Added

- **A thing (RD-1150-02).** Text.

## [1.9.1] - 2026-09-30
EOF
notes() { "$TREE/scripts/release-notes.sh" "$@"; }

expect "the points, plain text, wrapped lines joined" \
    "- Downloads from file hosters run in parallel, see the handbook.
- The Updates page says what is new." "$(notes 2.0.0)"
expect "a leading v is the same version" "$(notes 2.0.0)" "$(notes v2.0.0)"
expect "the maintenance sentence" \
    "Maintenance release: internal changes only, no change in behaviour." "$(notes 1.9.1)"
expect "a beta falls back to its release" "$(notes 2.0.0)" "$(notes 2.0.0-beta.1)"
expect "a version without a section has no notes" "" "$(notes 2.0.1)"
expect "another file names the source" "" "$(notes 2.0.0 "$SCRATCH/none.md")"

expect "the anchor of the CHANGELOG heading" "200---2026-10-10" "$(notes --anchor v2.0.0)"
expect "a beta's anchor falls back to its release" "200---2026-10-10" "$(notes --anchor 2.0.0-beta.2)"
expect "no heading, no anchor" "" "$(notes --anchor 2.1.0)"

# The GitHub release's description (scripts/release-publish.sh app-release): the points first,
# then the developers' link to the CHANGELOG section at the tag.
cp "$ROOT/scripts/release-publish.sh" "$TREE/scripts/"
mkdir -p "$SCRATCH/runner"
(cd "$TREE" && RUNNER_TEMP="$SCRATCH/runner" REF_NAME=v2.0.0 GITHUB_REPOSITORY=owner/repo \
    scripts/release-publish.sh app-release)
expect "the application release's text" "## What's new

- Downloads from file hosters run in parallel, see the handbook.
- The Updates page says what is new.

**For developers:** every change of 2.0.0 is in [CHANGELOG.md](https://github.com/owner/repo/blob/v2.0.0/CHANGELOG.md#200---2026-10-10)." \
    "$(cat "$SCRATCH/runner/app-release.md")"

run_status notes
expect_status "no version is a usage error" 2
run_status notes --check --version
expect_status "--version without a version is a usage error" 2

# --check: the file as written passes, the draft named.
check() { "$TREE/scripts/release-notes.sh" --check "$@"; }
run_status check
expect_status "every section short and for users: exit 0" 0
expect_output "the draft is named" "RELEASE-NOTES.md: 2.1.0 is a draft"
run_status check --version 2.0.0
expect_status "a release with its section: exit 0" 0
run_status check --version 2.0.0-beta.1
expect_status "a beta with its release's section: exit 0" 0
run_status check --version 2.1.0
expect_status "the release of a draft: exit 1" 1
expect_output "names the draft" "the section 2.1.0 is still a draft"
run_status check --version 2.2.0
expect_status "a release without its section: exit 1" 1
expect_output "names the section to write" 'no section "## 2.2.0" for the release'

# One forbidden section at a time, as the only section; the line it has to name.
refused() {
    local name="$1" text="$2" needle="$3"
    printf '# Release notes\n\n## 3.0.0\n\n%s\n' "$text" > "$TREE/RELEASE-NOTES.md"
    run_status check
    expect_status "$name: exit 1" 1
    expect_output "$name: named" "RELEASE-NOTES.md:3: 3.0.0 $needle"
}
refused "a job number" "- Narrower visibility in every crate (RD-1120-12, CR-9)." "point 1 names a job"
refused "a plugin job" "- The plugin of PL-12 starts." "point 1 names a job"
refused "a path" "- Fixed in crates/rd-http/src/lib.rs." "point 1 names a path"
# shellcheck disable=SC2016  # the backticks are the text
refused "a code span" '- The chain no longer stops at a stale `web/dist`.' "point 1 names a code identifier"
refused "a snake_case name" "- A new setting unwrap_package_folder." "point 1 names a code identifier"
refused "German" "- Downloads laufen schneller über das Netz." "point 1 is not English"
refused "no sentence" "- Faster downloads" "point 1 does not end a sentence"
refused "too long" "- $(printf 'Faster. %.0s' {1..30})" "point 1 is 239 characters, at most 200"
refused "too many points" "$(printf -- '- A point.\n%.0s' {1..9})" "has 9 points, at most 8"
refused "prose" "Many things got better." "is not a list of points"
refused "prose beside points" "$(printf 'An intro.\n\n- A point.')" "mixes points with other text"
refused "an empty section" "" "has no points"

# What passes: a version, a domain, "and/or", a time.
printf '# Release notes\n\n## 3.0.0\n\n%s\n' \
    "- Version 3.0 checks example.com and/or its mirrors at 10:30." > "$TREE/RELEASE-NOTES.md"
run_status check
expect_status "a version, a domain, a slash and a time pass" 0

printf '# Release notes\n\n## 3.0.0\n\n- One.\n\n## 3.0.0\n\n- Two.\n\n## next\n\n- Three.\n' > "$TREE/RELEASE-NOTES.md"
run_status check
expect_status "a second section and a heading that is no version: exit 1" 1
expect_output "the second section" "RELEASE-NOTES.md:7: 3.0.0 has a second section"
expect_output "the heading" "RELEASE-NOTES.md:11: next is not a version"

rm "$TREE/RELEASE-NOTES.md"
run_status check
expect_status "no RELEASE-NOTES.md: exit 1" 1

# The real file: every section passes.
run_status "$ROOT/scripts/release-notes.sh" --check
expect_status "RELEASE-NOTES.md of this checkout keeps the rules" 0

finish_tests release-notes
