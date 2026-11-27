#!/usr/bin/env bash
#
# scripts/lib/archive-jobs.py (RD-140-19) against a small repository built in a temp directory:
# one finished and one open job of milestone 1.4 that link each other, the working file of a
# tagged 1.3.0 and of an untagged 1.4.0, a document and a source comment naming the finished
# job, and a plugin comment that must stay as it is. The run is made twice; the second has to
# be a no-op, and every relative link in the tree has to resolve after the first. Then --check has
# to refuse a miscounted Job Inventory row that a plain run recounts (RD-140-24), a job that is
# open again but still lies in the archive, and last every status line that is missing or not one
# of the five words (RD-120-14, moved here from a Rust test by RD-191-09), and a status line that
# counts its file's checkboxes (RD-1270-17), and finally the open boxes of the archive: marked
# `→ offen in RD-<id>`, struck or exempt, and every mark naming a job file (RD-1490-01).
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

# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck disable=SC2034  # `status` is read inside the eval of expect()
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
expect_true "check names what is due and exits 1" '[[ $status -eq 1 ]] && has "$SCRATCH/out" 140-01-done.md && has "$SCRATCH/out" 130-00-release'
expect_true "check leaves the untagged 1.4.0 working file alone" '! has "$SCRATCH/out" 140-00-release'
expect_true "check writes nothing" '[[ -z "$(git -C "$TREE" status --porcelain)" ]]'

# The release chain's checks before its archive-jobs step: due is named, not a failure.
run --check --due-ok
expect_true "check --due-ok names what is due and exits 0" '[[ $status -eq 0 ]] && has "$SCRATCH/out" 140-01-done.md && has "$SCRATCH/out" "archive-jobs step moves it"'

echo "edit in progress" >> "$JOBS/140-02-open.md"
run
expect_true "uncommitted work under the jobs directory is refused" '[[ $status -eq 2 ]] && [[ -f "$JOBS/140-01-done.md" ]]'
git -C "$TREE" checkout -q -- docs/roadmap/jobs

run
expect_true "the run succeeds" '[[ $status -eq 0 ]]'
expect_true "the finished job and the tagged working file moved" '[[ -f "$JOBS/archive/140-01-done.md" && -f "$JOBS/archive/130-00-release-koordination.md" && ! -e "$JOBS/140-01-done.md" ]]'
expect_true "the open job and the untagged working file stayed" '[[ -f "$JOBS/140-02-open.md" && -f "$JOBS/140-00-release-koordination.md" ]]'
expect_true "a moved job links the open one from one level down" 'has "$JOBS/archive/140-01-done.md" "(../140-02-open.md)"'
expect_true "a moved job keeps its anchor and its root-relative link" 'has "$JOBS/archive/140-01-done.md" "(../../../roadmap.md#a)" && has "$JOBS/archive/140-01-done.md" "(/login)"'
expect_true "an open job links into the archive" 'has "$JOBS/140-02-open.md" "(./archive/140-01-done.md)"'
expect_true "two moved files link each other in place" 'has "$JOBS/archive/130-00-release-koordination.md" "(140-01-done.md)"'
expect_true "a document elsewhere follows the move" 'has "$TREE/docs/roadmap.md" "(roadmap/jobs/archive/140-01-done.md)" && has "$TREE/docs/roadmap.md" "(roadmap/jobs/140-02-open.md#x)"'
expect_true "a path in a source comment follows the move" 'has "$TREE/crates/x/src/lib.rs" "docs/roadmap/jobs/archive/140-01-done.md"'
expect_true "plugins/ is left alone" 'has "$TREE/plugins/p/src/lib.rs" "docs/roadmap/jobs/140-01-done.md"'
expect_true "the index keeps only the open rows" '! has "$JOBS/README.md" "140-01-done" && ! has "$JOBS/README.md" "### Milestone 1.3" && has "$JOBS/README.md" "| [RD-140-02 — Open](./140-02-open.md) | P2 | Open |"'
expect_true "the archive has the row under its milestone, status taken from the file" 'has "$JOBS/archive/README.md" "### Milestone 1.4 — Plugin Distribution" && has "$JOBS/archive/README.md" "| [RD-140-01 — Done](./140-01-done.md) | P1 | Implemented |"'
expect_true "the archive lists milestones in order" '[[ "$(grep -n "^### " "$JOBS/archive/README.md" | cut -d: -f1 | head -1)" -lt "$(grep -n "^### Milestone 1.4" "$JOBS/archive/README.md" | cut -d: -f1)" ]]'
expect_true "the inventory is recounted" 'has "$JOBS/README.md" "| Milestone 1.4 — Plugin Distribution | 2 | 1 | 3 |" && has "$JOBS/README.md" "| Milestone 1.3 — Everyday Use | 0 | 1 | 1 |" && has "$JOBS/README.md" "| **Total** | **2** | **2** | **4** |"'
expect_true "the rest of the index is untouched" 'has "$JOBS/README.md" "Nothing here moves."'

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
expect_true "every relative link resolves" '[[ -z "$dangling" ]]' || printf '     %s\n' "$dangling"

git -C "$TREE" -c user.name=t -c user.email=t@t add -A
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qm archived
run
expect_true "a second run is a no-op" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "nothing to archive" && [[ -z "$(git -C "$TREE" status --porcelain)" ]]'
run --check
expect_true "check after the run exits 0" '[[ $status -eq 0 ]]'
run --check --release 1.4.0
expect_true "the release being cut counts as tagged" '[[ $status -eq 1 ]] && has "$SCRATCH/out" 140-00-release'

sed -i 's/^| Milestone 1.4 — Plugin Distribution | 2 | 1 | 3 |$/| Milestone 1.4 — Plugin Distribution | 3 | 1 | 4 |/' "$JOBS/README.md"
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qam miscounted
run --check
expect_true "check refuses a miscounted inventory row" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "miscounted:" && has "$SCRATCH/out" "| Milestone 1.4 — Plugin Distribution | 2 | 1 | 3 |"'
run
expect_true "a run with nothing due recounts the inventory" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "recounted" && has "$JOBS/README.md" "| Milestone 1.4 — Plugin Distribution | 2 | 1 | 3 |" && has "$JOBS/README.md" "Nothing here moves."'
run --check
expect_true "check after the recount exits 0" '[[ $status -eq 0 ]]'
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qam recounted

sed -i 's/^- \*\*Status:\*\* Implemented.*/- **Status:** Partial (reopened)/' "$JOBS/archive/140-01-done.md"
run --check
expect_true "check refuses an open job left in the archive" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "open again: docs/roadmap/jobs/archive/140-01-done.md (Partial)"'

git -C "$TREE" checkout -q -- docs/roadmap/jobs

# The status words (RD-120-14, a Rust test until RD-191-09): five words, each standing alone.
printf -- '- **Status:** Done (2026-10-04)\n' > "$JOBS/140-03-done-word.md"
printf -- '- **Status:** Openish\n' > "$JOBS/140-04-openish.md"
printf '# RD-140-05\n\n- **Milestone:** 1.4\n' > "$JOBS/140-05-no-line.md"
printf -- '- **Status:** Erledigt\n' > "$JOBS/archive/140-06-archived.md"
run --check
expect_true "check refuses \`Done\`" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "status: docs/roadmap/jobs/140-03-done-word.md: status word \`Done\`"'
expect_true "a word that merely starts with an allowed one is refused" 'has "$SCRATCH/out" "140-04-openish.md: status word \`Openish\`"'
expect_true "a job file without a status line is refused" 'has "$SCRATCH/out" "140-05-no-line.md: no \`- **Status:**\` line"'
expect_true "the archive is read too" 'has "$SCRATCH/out" "archive/140-06-archived.md: status word \`Erledigt\`"'
expect_true "the message names the allowed words" 'has "$SCRATCH/out" "\`Blocked/No-Go\`, \`Implemented\`, \`In progress\`, \`Open\`, \`Partial\`"'
expect_true "a working file needs no status line" '! has "$SCRATCH/out" "140-00-release-koordination.md: no"'
rm "$JOBS/140-03-done-word.md" "$JOBS/140-04-openish.md" "$JOBS/140-05-no-line.md" "$JOBS/archive/140-06-archived.md"
for value in "Open" "Open / feasibility-gated" "In progress (2026-10-04)" "Partial, corrected by RD-095-02" \
    "Blocked/No-Go — gemessen" "Implemented (Abnahme auf Windows offen)"; do
    printf -- '- **Status:** %s\n' "$value" > "$JOBS/140-07-word.md"
    run --check
    expect_true "\`$value\` passes" '! has "$SCRATCH/out" "140-07-word.md:"'
done
printf -- '- **Status:** Done\n' > "$JOBS/140-00-release-koordination.md"
run --check
expect_true "a working file that grows a status line is checked like any other" 'has "$SCRATCH/out" "140-00-release-koordination.md: status word \`Done\`"'

# A counted status line (RD-1270-17): the numbers come from the file's own checkboxes.
rm -f "$JOBS/140-07-word.md"
git -C "$TREE" checkout -q -- docs/roadmap/jobs
printf -- '- **Status:** Open (gezählt: 1 offen, 0 abgehakt)\n\n- [ ] a\n  - [x] b\n- [X] c\n- [ ] d\n* [ ] e\n' \
    > "$JOBS/140-08-live.md"
printf -- '- **Status:** Open (Stand: 3 offen)\n\n- [ ] a\n' > "$JOBS/140-09-plain.md"
git -C "$TREE" add -A && git -C "$TREE" -c user.name=t -c user.email=t@t commit -qm counted
run --check
expect_true "check refuses a counted status line its checkboxes do not bear out" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "miscounted: docs/roadmap/jobs/140-08-live.md status says \`gezählt: 1 offen, 0 abgehakt\`, its checkboxes \`gezählt: 3 offen, 2 abgehakt\`"'
expect_true "a status line without the counted form is left alone" '! has "$SCRATCH/out" 140-09-plain.md'
run
expect_true "a run writes the count" '[[ $status -eq 0 ]] && has "$JOBS/140-08-live.md" "- **Status:** Open (gezählt: 3 offen, 2 abgehakt)" && has "$JOBS/140-09-plain.md" "(Stand: 3 offen)"'
run --check
expect_true "check after the count exits 0" '[[ $status -eq 0 ]]'
git -C "$TREE" -c user.name=t -c user.email=t@t commit -qam recounted-status
run
expect_true "a second run leaves the count as it is" '[[ $status -eq 0 ]] && [[ -z "$(git -C "$TREE" status --porcelain)" ]]'

# The index's status words (RD-1480-24): written from the file, named by --check without failing.
printf '# RD-140-10 — Words\n\n- **Milestone:** 1.4\n- **Status:** In progress\n' > "$JOBS/140-10-words.md"
python3 - "$JOBS/README.md" <<'PY'
import re
import sys

path = sys.argv[1]
text = open(path, encoding="utf-8").read()
row = next(line for line in text.split("\n") if line.startswith("| [RD-140-02"))
text = text.replace(row, row + "\n| [RD-140-10 — Words](./140-10-words.md) | P2 | Open (wartet) |")
item = "**Milestone 1.4** — Plugins\n\n- [RD-140-10 — Words](./140-10-words.md) — Open (wartet) — Audit\n"
text = re.sub(r"(\(\./140-02-open\.md\) — Open).*", r"\1 (bleibt) — Audit", text)
text = text.replace("## Catalog", item + "\n## Catalog", 1)
open(path, "w", encoding="utf-8").write(text)
PY
run
git -C "$TREE" add -A && git -C "$TREE" -c user.name=t -c user.email=t@t commit -qm words
sed -i 's/(.\/140-10-words.md) | P2 | In progress |/(.\/140-10-words.md) | P2 | Open |/' "$JOBS/README.md"
run --check
expect_true "check names an index word that is not its file's and does not fail over it" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "another status for 140-10-words.md"'
run
expect_true "a run writes the file's word into the catalog" '[[ $status -eq 0 ]] && has "$JOBS/README.md" "(./140-10-words.md) | P2 | In progress |"'
expect_true "and into Open Work, with the note behind it" 'has "$JOBS/README.md" "(./140-10-words.md) — In progress — Audit"'
expect_true "a right word keeps what follows it" 'has "$JOBS/README.md" "(./140-02-open.md) — Open (bleibt) — Audit"'
run --check
expect_true "check after the run names no word" '! has "$SCRATCH/out" "another status"'

# The open boxes of the archive (RD-1490-01): each carries `→ offen in RD-<id>` in its paragraph
# (the box line and its indented lines) or is struck through; `Blocked/No-Go` is exempt, a
# working file without a status is not, and every mark names a job file that exists.
ARC="$JOBS/archive"
cat > "$ARC/140-11-referenced.md" <<'EOF'
# RD-140-11 — Referenced

- **Status:** Implemented

- [ ] Left open by a wave, marked the way scripts/job-evidence.sh marks it
  → offen in RD-140-02
- [ ] A box whose mark sits on its third line
      with a continuation line
      and the mark at its end → offen in RD-140-01.
EOF
cat > "$ARC/140-12-struck.md" <<'EOF'
# RD-140-12 — Struck

- **Status:** Implemented

- [ ] ~~Lapsed with the plan,
      over two lines~~ — verfallen (PLAN-8)

The rule quotes its form, `→ offen in RD-…` or `→ offen in RD-<id>`, and names no job.
EOF
cat > "$ARC/140-13-unreferenced.md" <<'EOF'
# RD-140-13 — Unreferenced

- **Status:** Implemented

- [ ] Still open
  with a second line
- [ ] The next box, which holds the mark
  → offen in RD-140-02
- [ ] Struck only ~~in part~~
```text
- [ ] a box in a code fence
```
EOF
cat > "$ARC/140-14-blocked.md" <<'EOF'
# RD-140-14 — Blocked

- **Status:** Blocked/No-Go (never built)

- [ ] Open, because the job was never built
EOF
cat > "$ARC/150-00-release-koordination.md" <<'EOF'
# Release 1.5.0

- [ ] A box of a working file without a status line
EOF
cat > "$ARC/140-15-dangling.md" <<'EOF'
# RD-140-15 — Dangling

- **Status:** Implemented

- [ ] Marked, but towards no job
  → offen in RD-999-99
EOF
run --check
expect_true "check refuses an unreferenced box and names it file:line: text" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "unreferenced: docs/roadmap/jobs/archive/140-13-unreferenced.md:5: Still open"'
expect_true "a box partly struck is not struck" 'has "$SCRATCH/out" "140-13-unreferenced.md:9: Struck only ~~in part~~"'
expect_true "a mark belongs to the box above it, a fenced box is no box" '! has "$SCRATCH/out" "140-13-unreferenced.md:7:" && ! has "$SCRATCH/out" "140-13-unreferenced.md:11:"'
expect_true "a marked box passes, also with the mark on a continuation line" '! has "$SCRATCH/out" "140-11-referenced.md"'
expect_true "a struck box passes, and prose quoting the mark's form is no reference" '! has "$SCRATCH/out" "140-12-struck.md"'
expect_true "a Blocked/No-Go file is exempt" '! has "$SCRATCH/out" "140-14-blocked.md"'
expect_true "a working file without a status is checked" 'has "$SCRATCH/out" "unreferenced: docs/roadmap/jobs/archive/150-00-release-koordination.md:3: A box of a working file"'
expect_true "a mark that names no job file is refused" 'has "$SCRATCH/out" "dangling: docs/roadmap/jobs/archive/140-15-dangling.md:6: \`→ offen in RD-999-99\` names no job file"'
expect_true "the run counts them" 'has "$SCRATCH/out" "4 archive reference problem(s)"'
rm "$ARC/140-13-unreferenced.md" "$ARC/150-00-release-koordination.md" "$ARC/140-15-dangling.md"
run --check
expect_true "check with every box referenced, struck or exempt exits 0" '[[ $status -eq 0 ]] && ! has "$SCRATCH/out" "archive reference"'

finish_tests archive-jobs
