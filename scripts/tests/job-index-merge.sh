#!/usr/bin/env bash
#
# The `rd-jobindex` driver on the case of 1.24 w3/w4 (RD-1250-02), rebuilt in a scratch
# repository: a feature branch forks while the job index lists four 1.24 jobs; it starts one
# (`Open` -> `In progress`), adds a job and counts it. Meanwhile `development` sets two others
# `Implemented` and archive-jobs moves them, recounting the Job Inventory. Then the branch is
# merged with the drivers integrate.sh registers.
#
# Expected right after the merge, before any archive-jobs run, in both directions: no archived row
# back in the open index, the started job once in each list with the new status, the added job
# once in each list, one Job Inventory row per area — and after archive-jobs, as integrate.sh runs
# it, the counts the catalogs bear out and a clean --check. With the driver of 1.24 (both sides of
# a row-only conflict) the archived rows came back, the started job stood twice and the inventory
# twice. Then the driver alone on three files: a row changed on both sides keeps the later status,
# a row removed on one side and changed on the other stays.
#
#   scripts/tests/job-index-merge.sh
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
INDEX="$JOBS/README.md"
archive_jobs() { python3 "$ROOT/scripts/lib/archive-jobs.py" "$TREE" "$@"; }
commit() { git -C "$TREE" add -A && git -C "$TREE" commit -qm "$1"; }
job() { printf '# %s\n\n- **Milestone:** 1.24\n- **Status:** %s\n' "$2" "$3" > "$JOBS/$1"; }
rows() { grep -c -- "$1" "$2" || true; }
item() { printf -- '- [RD-%s — %s](./%s.md) — %s — owner' "${1%%-[a-z]*}" "$2" "$1" "$3"; }
row() { printf '| [RD-%s — %s](./%s.md) | P2 | %s |' "${1%%-[a-z]*}" "$2" "$1" "$3"; }

git init -q -b development "$TREE"
mkdir -p "$JOBS/archive"
cp "$ROOT/.gitattributes" "$TREE/"
cat > "$INDEX" <<EOF
# Jobs

## Job Inventory

| Area | Open (here) | Archived (\`archive/\`) | Total |
| --- | ---: | ---: | ---: |
| Milestone 1.23 — Old | 0 | 1 | 1 |
| Milestone 1.24 — Follow-ups | 4 | 0 | 4 |
| **Total** | **4** | **1** | **5** |

## Open Work

Every open job.

**Milestone 1.24** — Nachzüge

$(item 1240-23-alpha Alpha "In progress")
$(item 1240-24-tray Tray Open)
$(item 1240-25-beta Beta "In progress")
$(item 1240-37-gamma Gamma "In progress")

## Catalog

### Milestone 1.24 — Follow-ups

| Job | Priority | Status |
| --- | --- | --- |
$(row 1240-23-alpha Alpha "In progress")
$(row 1240-24-tray Tray Open)
$(row 1240-25-beta Beta "In progress")
$(row 1240-37-gamma Gamma "In progress")
EOF
cat > "$JOBS/archive/README.md" <<'EOF'
# rDownloader Roadmap Jobs — Archive

### Milestone 1.23 — Old

| Job | Priority | Status |
| --- | --- | --- |
| [RD-1230-01 — Old](./1230-01-old.md) | P2 | Implemented |
EOF
printf '# RD-1230-01 — Old\n\n- **Milestone:** 1.23\n- **Status:** Implemented\n' > "$JOBS/archive/1230-01-old.md"
job 1240-23-alpha.md RD-1240-23 "In progress"
job 1240-24-tray.md RD-1240-24 Open
job 1240-25-beta.md RD-1240-25 "In progress"
job 1240-37-gamma.md RD-1240-37 "In progress"
commit base
git -C "$TREE" tag fork

# The older feature side: the tray job started, a job added after it, both counted.
git -C "$TREE" checkout -q -b feat/tray
job 1240-24-tray.md RD-1240-24 "In progress"
job 1240-38-noise.md RD-1240-38 Open
sed -i -e "s/^\(- \[RD-1240-24.*\) — Open — owner$/\1 — In progress — owner/" \
    -e "s/^\(| \[RD-1240-24.*\) | Open |$/\1 | In progress |/" "$INDEX"
sed -i -e "/^- \[RD-1240-24/a $(item 1240-38-noise Noise Open)" \
    -e "/^| \[RD-1240-24/a $(row 1240-38-noise Noise Open)" \
    -e 's/^| Milestone 1.24 — Follow-ups | 4 | 0 | 4 |$/| Milestone 1.24 — Follow-ups | 5 | 0 | 5 |/' \
    -e 's/^| \*\*Total\*\* | \*\*4\*\* | \*\*1\*\* | \*\*5\*\* |$/| **Total** | **5** | **1** | **6** |/' \
    "$INDEX"
commit "feat: tray"

# The archiving side: two jobs implemented, archive-jobs moves and recounts.
git -C "$TREE" checkout -q development
job 1240-23-alpha.md RD-1240-23 Implemented
job 1240-25-beta.md RD-1240-25 Implemented
commit "docs: alpha and beta implemented"
run_status archive_jobs
expect_status "development archives the two finished jobs" 0
commit "docs(roadmap): archive alpha and beta"
expect "and the open index no longer lists them" "0" "$(rows '1240-2[35]-' "$INDEX")"

rd_integrate_merge_drivers "$TREE" "$ROOT"

# Checks the merged index before and after archive-jobs; $1 names the direction.
verify() {
    local how="$1"
    expect "$how: no archived row back in the open index" "0" "$(rows '1240-2[35]-' "$INDEX")"
    expect "$how: the archived rows once in the archive's" "2" \
        "$(rows '1240-2[35]-' "$JOBS/archive/README.md")"
    expect "$how: the started job once in each list, with the new status" \
        "$(item 1240-24-tray Tray "In progress")|$(row 1240-24-tray Tray "In progress")" \
        "$(grep -F 1240-24-tray "$INDEX" | paste -sd'|' -)"
    expect "$how: the added job once in each list" "2" "$(rows 1240-38-noise "$INDEX")"
    expect "$how: the untouched job once in each list" "2" "$(rows 1240-37-gamma "$INDEX")"
    expect "$how: one Job Inventory row per area" "1|1|1" \
        "$(rows '^| Milestone 1.23' "$INDEX")|$(rows '^| Milestone 1.24' "$INDEX")|$(rows '^| \*\*Total' "$INDEX")"
    expect_true "$how: no conflict marker" '! grep -q "^<<<<<<< " "$INDEX"'
    run_status archive_jobs
    expect_status "$how: archive-jobs, integrate.sh's generator, runs" 0
    expect "$how: the Job Inventory recounted" \
        "| Milestone 1.23 — Old | 0 | 1 | 1 ||| Milestone 1.24 — Follow-ups | 3 | 2 | 5 ||| **Total** | **3** | **3** | **6** |" \
        "$(grep -E '^\| (Milestone|\*\*Total)' "$INDEX" | paste -sd'|' -)"
    run_status archive_jobs --check
    expect_status "$how: archive-jobs --check finds nothing" 0
    commit "chore(generated): $how" > /dev/null || true
}

# --- the branch into development, as integrate.sh merges it -------------------------------------
git -C "$TREE" checkout -q -b integration development
run_status git -C "$TREE" merge --no-ff --no-edit feat/tray
expect_status "the older branch merges into development without a person" 0
verify "into development"

# --- development into the branch ---------------------------------------------------------------
git -C "$TREE" checkout -q -b feat/tray-later feat/tray
run_status git -C "$TREE" merge --no-ff --no-edit development
expect_status "development merges into the older branch without a person" 0
verify "into the branch"

# --- the driver alone --------------------------------------------------------------------------
DRIVER="$ROOT/scripts/lib/merge-drivers/job-index-merge.py"
three() { # base ours theirs, each a list of lines joined by @
    tr '@' '\n' <<< "$1" > "$SCRATCH/base"
    tr '@' '\n' <<< "$2" > "$SCRATCH/ours"
    tr '@' '\n' <<< "$3" > "$SCRATCH/theirs"
    run_status python3 "$DRIVER" "$SCRATCH/base" "$SCRATCH/ours" "$SCRATCH/theirs"
}
a="- [RD-1-A](./a.md) — Open — x"
b="- [RD-2-B](./b.md) — Open — x"
c="- [RD-3-C](./c.md) — Open — x"
three "$a@$b@$c" "${a/Open/Partial}@${b/Open/Implemented}" "${a/Open/In progress}@$b@$c@- [RD-4-D](./d.md) — Open — x"
expect_status "a row-only conflict merges" 0
expect "both changed: the later status; removed and unchanged: gone; new: added" \
    "${a/Open/Partial}@${b/Open/Implemented}@- [RD-4-D](./d.md) — Open — x" \
    "$(paste -sd'@' "$SCRATCH/ours")"
three "$a@$b@$c" "$b" "${a/Open/In progress}@$b@${c/Open/Partial}"
expect_status "a row removed here and changed there merges" 0
expect "and stays, for archive-jobs to place" \
    "${a/Open/In progress}@$b@${c/Open/Partial}" "$(paste -sd'@' "$SCRATCH/ours")"

finish_tests job-index-merge
