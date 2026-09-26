#!/usr/bin/env bash
#
# scripts/lib/archive-jobs.py (RD-140-19) against a small repository built in a temp directory:
# one finished and one open job of milestone 1.4 that link each other, the working file of a
# tagged 1.3.0 and of an untagged 1.4.0, a document and a source comment naming the finished
# job, and a plugin comment that must stay as it is. The run is made twice; the second has to
# be a no-op, and every relative link in the tree has to resolve after the first. Last, --check
# has to refuse a job that is open again but still lies in the archive.
#
# Pure python3, bash and git: it runs in a second. check.sh runs it when scripts/lib/ or
# scripts/tests/ change, and under --full.
#
#   scripts/tests/archive-jobs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/lib/archive-jobs.py"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
TREE="$SCRATCH/repo"
JOBS="$TREE/docs/roadmap/jobs"

failures=0
passed=0
ok() { echo "ok   $1"; passed=$((passed + 1)); }
fail() { echo "FAIL $1"; failures=$((failures + 1)); }
expect() { if eval "$2"; then ok "$1"; else fail "$1"; fi; }
run() { python3 "$SCRIPT" "$TREE" "$@" > "$SCRATCH/out" 2>&1 && status=0 || status=$?; }
has() { grep -qF -- "$2" "$1"; }

mkdir -p "$JOBS" "$TREE/crates/x/src" "$TREE/plugins/p/src"
cat > "$JOBS/README.md" <<'EOF'
# Jobs

## Job Inventory

| Area | Open (here) | Archived (`archive/`) | Total |
| --- | ---: | ---: | ---: |
| Milestone 1.3 — Everyday Use | 1 | 0 | 1 |
| Milestone 1.4 — Plugin Distribution | 3 | 0 | 3 |
| **Total** | **4** | **0** | **4** |

## Open Work

**Milestone 1.4** — Plugin-Verteilung

- [RD-140-01 — Done](./140-01-done.md) — Implemented
- [RD-140-02 — Open](./140-02-open.md) — Open

## Catalog

### Milestone 1.3 — Everyday Use

| Job | Priority | Status |
| --- | --- | --- |
| [RD-130-00 — Koordination](./130-00-release-koordination.md) | — | Arbeitsdatei |

### Milestone 1.4 — Plugin Distribution

| Job | Priority | Status |
| --- | --- | --- |
| [RD-140-01 — Done](./140-01-done.md) | P1 | Open |
| [RD-140-02 — Open](./140-02-open.md) | P2 | Open |
| [RD-140-00 — Koordination](./140-00-release-koordination.md) | — | Arbeitsdatei |

## Rules

Nothing here moves.
EOF
cat > "$JOBS/140-01-done.md" <<'EOF'
# RD-140-01 — Done

- **Milestone:** 1.4
- **Status:** Implemented (2026-09-26)

Follows [RD-140-02](./140-02-open.md), see [the roadmap](../../roadmap.md#a) and [login](/login).
EOF
cat > "$JOBS/140-02-open.md" <<'EOF'
# RD-140-02 — Open

- **Status:** Open

Builds on [RD-140-01](./140-01-done.md) and [the index](README.md).
EOF
printf '# Koordination 1.3.0\n\nKein Job. [RD-140-01](140-01-done.md)\n' > "$JOBS/130-00-release-koordination.md"
printf '# Koordination 1.4.0\n\nKein Job.\n' > "$JOBS/140-00-release-koordination.md"
printf '# Roadmap\n\n[RD-140-01](roadmap/jobs/140-01-done.md) and [open](roadmap/jobs/140-02-open.md#x)\n' \
    > "$TREE/docs/roadmap.md"
printf '//! `docs/roadmap/jobs/140-01-done.md` records it.\n' > "$TREE/crates/x/src/lib.rs"
printf '//! `docs/roadmap/jobs/140-01-done.md` records it.\n' > "$TREE/plugins/p/src/lib.rs"
git -C "$TREE" init -q
git -C "$TREE" -c user.name=t -c user.email=t@t add -A
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qm fixture
git -C "$TREE" tag v1.3.0

run --check
expect "check names what is due and exits 1" '[[ $status -eq 1 ]] && has "$SCRATCH/out" 140-01-done.md && has "$SCRATCH/out" 130-00-release'
expect "check leaves the untagged 1.4.0 working file alone" '! has "$SCRATCH/out" 140-00-release'
expect "check writes nothing" '[[ -z "$(git -C "$TREE" status --porcelain)" ]]'

echo "edit in progress" >> "$JOBS/140-02-open.md"
run
expect "uncommitted work under the jobs directory is refused" '[[ $status -eq 2 ]] && [[ -f "$JOBS/140-01-done.md" ]]'
git -C "$TREE" checkout -q -- docs/roadmap/jobs

run
expect "the run succeeds" '[[ $status -eq 0 ]]'
expect "the finished job and the tagged working file moved" '[[ -f "$JOBS/archive/140-01-done.md" && -f "$JOBS/archive/130-00-release-koordination.md" && ! -e "$JOBS/140-01-done.md" ]]'
expect "the open job and the untagged working file stayed" '[[ -f "$JOBS/140-02-open.md" && -f "$JOBS/140-00-release-koordination.md" ]]'
expect "a moved job links the open one from one level down" 'has "$JOBS/archive/140-01-done.md" "(../140-02-open.md)"'
expect "a moved job keeps its anchor and its root-relative link" 'has "$JOBS/archive/140-01-done.md" "(../../../roadmap.md#a)" && has "$JOBS/archive/140-01-done.md" "(/login)"'
expect "an open job links into the archive" 'has "$JOBS/140-02-open.md" "(./archive/140-01-done.md)"'
expect "two moved files link each other in place" 'has "$JOBS/archive/130-00-release-koordination.md" "(140-01-done.md)"'
expect "a document elsewhere follows the move" 'has "$TREE/docs/roadmap.md" "(roadmap/jobs/archive/140-01-done.md)" && has "$TREE/docs/roadmap.md" "(roadmap/jobs/140-02-open.md#x)"'
expect "a path in a source comment follows the move" 'has "$TREE/crates/x/src/lib.rs" "docs/roadmap/jobs/archive/140-01-done.md"'
expect "plugins/ is left alone" 'has "$TREE/plugins/p/src/lib.rs" "docs/roadmap/jobs/140-01-done.md"'
expect "the index keeps only the open rows" '! has "$JOBS/README.md" "140-01-done" && ! has "$JOBS/README.md" "### Milestone 1.3" && has "$JOBS/README.md" "| [RD-140-02 — Open](./140-02-open.md) | P2 | Open |"'
expect "the archive has the row under its milestone, status taken from the file" 'has "$JOBS/archive/README.md" "### Milestone 1.4 — Plugin Distribution" && has "$JOBS/archive/README.md" "| [RD-140-01 — Done](./140-01-done.md) | P1 | Implemented |"'
expect "the archive lists milestones in order" '[[ "$(grep -n "^### " "$JOBS/archive/README.md" | cut -d: -f1 | head -1)" -lt "$(grep -n "^### Milestone 1.4" "$JOBS/archive/README.md" | cut -d: -f1)" ]]'
expect "the inventory is recounted" 'has "$JOBS/README.md" "| Milestone 1.4 — Plugin Distribution | 2 | 1 | 3 |" && has "$JOBS/README.md" "| Milestone 1.3 — Everyday Use | 0 | 1 | 1 |" && has "$JOBS/README.md" "| **Total** | **2** | **2** | **4** |"'
expect "the rest of the index is untouched" 'has "$JOBS/README.md" "Nothing here moves."'

dangling="$(cd "$TREE" && python3 - <<'PY'
import os, re, subprocess
for page in subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "*.md"],
                           capture_output=True, text=True).stdout.split():
    for target in re.findall(r"\]\(([^)\s]+)\)", open(page).read()):
        path = target.split("#")[0]
        if path and not path.startswith(("/", "http")) and not os.path.exists(
                os.path.normpath(os.path.join(os.path.dirname(page), path))):
            print(f"{page}: {target}")
PY
)"
expect "every relative link resolves" '[[ -z "$dangling" ]]' || printf '     %s\n' "$dangling"

git -C "$TREE" -c user.name=t -c user.email=t@t add -A
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qm archived
run
expect "a second run is a no-op" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "nothing to archive" && [[ -z "$(git -C "$TREE" status --porcelain)" ]]'
run --check
expect "check after the run exits 0" '[[ $status -eq 0 ]]'
run --check --release 1.4.0
expect "the release being cut counts as tagged" '[[ $status -eq 1 ]] && has "$SCRATCH/out" 140-00-release'

sed -i 's/^- \*\*Status:\*\* Implemented.*/- **Status:** Partial (reopened)/' "$JOBS/archive/140-01-done.md"
run --check
expect "check refuses an open job left in the archive" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "open again: docs/roadmap/jobs/archive/140-01-done.md (Partial)"'

echo
echo "$passed passed, $failures failed"
[[ "$failures" -eq 0 ]]
