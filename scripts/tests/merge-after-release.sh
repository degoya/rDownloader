#!/usr/bin/env bash
#
# A wave merged after a release (RD-1220-01), rebuilt in a scratch repository the way 1.21 went:
# a branch forks while `[Unreleased]` holds the coming release's entries and the job index lists
# its jobs, adds a CHANGELOG entry, starts its job and adds a second one; meanwhile `development`
# releases 1.20.0 (`[Unreleased]` moved under `## [1.20.0]`, tagged) and archive-jobs moves the
# finished job — rewriting the path the released section names. Then the branch is merged with
# the drivers integrate.sh registers, and archive-jobs runs as integrate.sh's generator does.
#
# Expected, in both directions (the branch into development, development into the branch): the
# merge needs no person, the new entry stands under `[Unreleased]` in its `### Added`, the released
# section is the tag's, release-sections.py agrees, and the job index lists every job once, where
# its file lies, with the file's status, the Job Inventory recounted. Without the fix — git's
# union for CHANGELOG.md, the job index merged as text — the merge stops on the index and the
# entry lands inside `[1.20.0]`.
#
# And where a person must decide: an entry the release took and the branch edited, and prose of
# the index changed on both sides, stay conflicts; release-sections.py names a released section
# that changed and only that; archive-jobs.sh --check names a doubled and a misplaced row.
#
#   scripts/tests/merge-after-release.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/integrate.sh
source "$ROOT/scripts/lib/integrate.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export PYTHONDONTWRITEBYTECODE=1

TREE="$SCRATCH/repo"
JOBS="$TREE/docs/roadmap/jobs"
archive_jobs() { python3 "$ROOT/scripts/lib/archive-jobs.py" "$TREE" "$@"; }
sections() { python3 "$ROOT/scripts/lib/release-sections.py" "$TREE"; }
commit() { git -C "$TREE" add -A && git -C "$TREE" commit -qm "$1"; }
# The `## [<version>]` section of CHANGELOG.md in file $1, heading to the line before the next.
section() { awk -v v="## [$2]" 'index($0, v) == 1 { on = 1; print; next } /^## / { on = 0 } on' "$1"; }
job() {
    printf '# %s — %s\n\n- **Milestone:** 1.20\n- **Priority:** P2\n- **Status:** %s\n' \
        "$2" "$3" "$4" > "$JOBS/$1"
}
rows() { grep -c -- "$1" "$2" || true; }

git init -q -b development "$TREE"
mkdir -p "$JOBS/archive"
cp "$ROOT/.gitattributes" "$TREE/"
cat > "$TREE/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

### Added

- **Alpha.** Recorded in `docs/roadmap/jobs/1200-01-alpha.md`.

## [1.19.0] - 2026-10-08

### Fixed

- **Older.** Released before.
EOF
cat > "$JOBS/README.md" <<'EOF'
# Jobs

## Job Inventory

| Area | Open (here) | Archived (`archive/`) | Total |
| --- | ---: | ---: | ---: |
| Milestone 1.19 — Audit | 0 | 1 | 1 |
| Milestone 1.20 — Wishes | 2 | 0 | 2 |
| **Total** | **2** | **1** | **3** |

## Open Work

Every open job.

**Milestone 1.20** — Wünsche

- [RD-1200-01 — Alpha](./1200-01-alpha.md) — In progress — owner
- [RD-1210-01 — Beta](./1210-01-beta.md) — Open — owner

## Catalog

### Milestone 1.20 — Wishes

| Job | Priority | Status |
| --- | --- | --- |
| [RD-1200-01 — Alpha](./1200-01-alpha.md) | P2 | In progress |
| [RD-1210-01 — Beta](./1210-01-beta.md) | P2 | Open |
EOF
cat > "$JOBS/archive/README.md" <<'EOF'
# rDownloader Roadmap Jobs — Archive

### Milestone 1.19 — Audit

| Job | Priority | Status |
| --- | --- | --- |
| [RD-1190-01 — Old](./1190-01-old.md) | P2 | Implemented |
EOF
printf '# RD-1190-01 — Old\n\n- **Milestone:** 1.19\n- **Status:** Implemented\n' > "$JOBS/archive/1190-01-old.md"
job 1200-01-alpha.md RD-1200-01 Alpha "In progress"
job 1210-01-beta.md RD-1210-01 Beta Open
commit base
git -C "$TREE" tag v1.19.0

# The branch: a new entry, its job started, a second job added.
git -C "$TREE" checkout -q -b feat/beta
sed -i 's/^- \*\*Alpha\.\*\* .*/&\n\n- **Beta.** New after the fork./' "$TREE/CHANGELOG.md"
job 1210-01-beta.md RD-1210-01 Beta "In progress"
job 1210-02-gamma.md RD-1210-02 Gamma Open
sed -i -e 's/^\(- \[RD-1210-01.*\) — Open — owner$/\1 — In progress — owner/' \
    -e 's/^\(| \[RD-1210-01.*\) | Open |$/\1 | In progress |/' "$JOBS/README.md"
sed -i -e '/^- \[RD-1210-01/a - [RD-1210-02 — Gamma](./1210-02-gamma.md) — Open — owner' \
    -e '/^| \[RD-1210-01/a | [RD-1210-02 — Gamma](./1210-02-gamma.md) | P2 | Open |' \
    -e 's/^| Milestone 1.20 — Wishes | 2 | 0 | 2 |$/| Milestone 1.20 — Wishes | 3 | 0 | 3 |/' \
    -e 's/^| \*\*Total\*\* | \*\*2\*\* | \*\*1\*\* | \*\*3\*\* |$/| **Total** | **3** | **1** | **4** |/' \
    "$JOBS/README.md"
commit "feat: beta"

# development: the release of 1.20.0, then the finished job archived.
git -C "$TREE" checkout -q development
sed -i 's/^## \[Unreleased\]$/&\n\n## [1.20.0] - 2026-10-09/' "$TREE/CHANGELOG.md"
commit "chore(release): 1.20.0"
git -C "$TREE" tag v1.20.0
job 1200-01-alpha.md RD-1200-01 Alpha Implemented
commit "docs: alpha implemented"
run_status archive_jobs
expect_status "development archives the finished job" 0
commit "docs(roadmap): archive alpha"
expect_true "and rewrites the path the released section names" \
    'grep -qF "jobs/archive/1200-01-alpha.md" "$TREE/CHANGELOG.md"'
run_status sections
expect_status "release-sections.py reads an archive path as the path it was" 0

rd_integrate_merge_drivers "$TREE" "$ROOT"

# Checks the merged tree after the generator; $1 names the direction.
verify() {
    local how="$1" log="$TREE/CHANGELOG.md" index="$JOBS/README.md"
    run_status archive_jobs
    expect_status "$how: archive-jobs, integrate.sh's generator, mends the index" 0
    commit "chore(generated): $how" > /dev/null || true
    expect "$how: [Unreleased] holds the new entry under its ### Added" \
        "## [Unreleased]||### Added||- **Beta.** New after the fork.|" \
        "$(section "$log" Unreleased | paste -sd'|' -)"
    expect "$how: the released section is the tag's but for the archived path" \
        "$(git -C "$TREE" show v1.20.0:CHANGELOG.md | section /dev/stdin 1.20.0 | sed 's#jobs/1200#jobs/archive/1200#')" \
        "$(section "$log" 1.20.0)"
    run_status sections
    expect_status "$how: release-sections.py agrees" 0
    expect "$how: the archived job has left the open index" "0" "$(rows 1200-01-alpha "$index")"
    expect "$how: and is in the archive's once" "1" "$(rows 1200-01-alpha "$JOBS/archive/README.md")"
    expect "$how: the started job once in each list, with its file's status" \
        "- [RD-1210-01 — Beta](./1210-01-beta.md) — In progress — owner|| [RD-1210-01 — Beta](./1210-01-beta.md) | P2 | In progress |" \
        "$(grep -F 1210-01-beta "$index" | paste -sd'|' -)"
    expect "$how: the added job once in each list" "2" "$(rows 1210-02-gamma "$index")"
    expect "$how: the Job Inventory recounted, each area once" \
        "| Milestone 1.19 — Audit | 0 | 1 | 1 ||| Milestone 1.20 — Wishes | 2 | 1 | 3 ||| **Total** | **2** | **2** | **4** |" \
        "$(grep -E '^\| (Milestone|\*\*Total)' "$index" | paste -sd'|' -)"
    run_status archive_jobs --check
    expect_status "$how: archive-jobs --check finds nothing" 0
}

# --- the branch into development, as integrate.sh merges it -------------------------------------
git -C "$TREE" checkout -q -b integration development
run_status git -C "$TREE" merge --no-ff --no-edit feat/beta
expect_status "the branch merges into development without a person" 0
verify "into development"

# --- development into the branch ---------------------------------------------------------------
git -C "$TREE" checkout -q -b feat/beta-later feat/beta
run_status git -C "$TREE" merge --no-ff --no-edit development
expect_status "development merges into the branch without a person" 0
verify "into the branch"

# --- what stays a person's ---------------------------------------------------------------------
git -C "$TREE" checkout -q -b feat/edit v1.19.0
sed -i 's/^- \*\*Alpha\.\*\* .*/- **Alpha, reworded.** Recorded elsewhere./' "$TREE/CHANGELOG.md"
commit "docs: alpha reworded"
git -C "$TREE" checkout -q -b integration-edit development
run_status git -C "$TREE" merge --no-ff --no-edit feat/edit
expect_status "an entry the release took and the branch edited stops the merge" 1
expect_true "as a conflict in CHANGELOG.md, with markers" \
    'git -C "$TREE" diff --name-only --diff-filter=U | grep -qx CHANGELOG.md && grep -q "^<<<<<<< " "$TREE/CHANGELOG.md"'
git -C "$TREE" merge --abort

git -C "$TREE" checkout -q -b feat/prose v1.19.0
sed -i 's/^Every open job\.$/Every open job, by milestone./' "$JOBS/README.md"
commit "docs: prose here"
git -C "$TREE" checkout -q -b integration-prose v1.19.0
sed -i 's/^Every open job\.$/Each open job./' "$JOBS/README.md"
commit "docs: prose there"
run_status git -C "$TREE" merge --no-ff --no-edit feat/prose
expect_status "prose of the job index changed on both sides stops the merge" 1
expect_true "as a conflict for a person" 'grep -q "^<<<<<<< " "$JOBS/README.md"'
git -C "$TREE" merge --abort

# --- the checks after it -----------------------------------------------------------------------
git -C "$TREE" checkout -q integration
sed -i 's/^### Added$/&\n\n- **Late.** Put into a released section./' "$TREE/CHANGELOG.md"
run_status sections
expect_status "a released section that changed after its tag is named" 1
expect_output "with its version" "the section 1.20.0 differs from its release commit v1.20.0"
expect_true "and only that one" '! grep -q "section 1.19.0" <<< "$output"'
git -C "$TREE" checkout -q -- CHANGELOG.md

# A doubled catalog row, and a row of an archived job appended to the open catalog.
sed -i '/^| \[RD-1210-01/p' "$JOBS/README.md"
printf '| [RD-1190-01 — Old](./1190-01-old.md) | P2 | Implemented |\n' >> "$JOBS/README.md"
run_status archive_jobs --check
expect_status "archive-jobs --check refuses a doubled and a misplaced row" 1
expect_output "naming the doubled one" "twice: docs/roadmap/jobs/README.md lists 1210-01-beta.md 2 times under ## Catalog"
expect_output "and the misplaced one" "misplaced: docs/roadmap/jobs/README.md lists 1190-01-old.md under ## Catalog, which lies in docs/roadmap/jobs/archive/ with its row there"

finish_tests merge-after-release
