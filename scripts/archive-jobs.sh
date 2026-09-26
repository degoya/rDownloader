#!/usr/bin/env bash
#
# Moves finished roadmap job files into docs/roadmap/jobs/archive/ (RD-140-19), so the index and
# the directory show only what is still open.
#
# Due is a job file whose status begins with `Implemented` or `Blocked/No-Go`, and a working file
# (`<NNN>-00-*.md`, no status line) whose release is tagged or is the one named by --release.
# Each is moved with `git mv`, its name unchanged; every relative Markdown link and every
# `roadmap/jobs/<file>` path mention in the repository is rewritten to the new place; its index
# row moves to archive/README.md under the same milestone, and the Job Inventory is recounted.
# plugins/, crates/rd-db/migrations/ and docs/ideas_and_infos.md are never touched. The logic
# and the full rules are in scripts/lib/archive-jobs.py.
#
# Idempotent: with nothing due it writes nothing and exits 0. It refuses (exit 2) to run over
# uncommitted changes under docs/roadmap/jobs/, so it never mixes into someone's work. It stages
# the moves only; the rewritten files are left for the commit that takes them.
#
# The release pipeline runs it as its `archive-jobs` step, between docs-gate and commit-guard,
# so the jobs a release finished move within the release commit.
#
# Usage:
#   scripts/archive-jobs.sh                   # move what is due
#   scripts/archive-jobs.sh --check           # name what is due, and any open job lying in
#                                             # archive/; exit 1 if there is either (check.sh)
#   scripts/archive-jobs.sh --release 1.4.0   # also treat 1.4.0 as tagged (the pipeline's form)
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/lib/archive-jobs.py" "$ROOT" "$@"
