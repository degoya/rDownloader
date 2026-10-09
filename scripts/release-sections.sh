#!/usr/bin/env bash
#
# Every released section of CHANGELOG.md and RELEASE-NOTES.md as its release left it
# (RD-1220-01): each `## [X.Y.Z]` / `## X.Y.Z` section is compared with the file at the tag
# vX.Y.Z, and a difference — what a merge across a release does when it puts a branch's entries
# into the section the release made — is printed with its diff. A version without its tag in this
# clone is skipped. The rules, and the few sections that differed before the check came, are in
# scripts/lib/release-sections.py. Run by check.sh's file checks, so the preflight too.
#
#   scripts/release-sections.sh     # exit 1 while a released section differs
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 -B "$ROOT/scripts/lib/release-sections.py" "$ROOT"
