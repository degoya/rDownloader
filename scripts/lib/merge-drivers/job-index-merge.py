#!/usr/bin/env python3
"""The `rd-jobindex` merge driver for the two job indexes, docs/roadmap/jobs/README.md and its
archive/README.md (RD-1220-01), which .gitattributes names and scripts/integrate.sh registers.

    job-index-merge.py <base> <ours> <theirs> [<path>]    # git's %O %A %B %P

Both indexes are tables of rows, one per job, that every branch touches: a new job, a status
word, the Job Inventory's counts, and archive-jobs.sh moving finished rows between the two. git
merges them as text, which conflicts in nearly every wave (nine times in the 1.9.1 integration).

The driver merges as text and resolves a conflict whose two sides are only table rows (`| `),
list items (`- [`) and blank lines, three-way against the merge base (RD-1250-02). A row stands
for what it names: the job file it links (a catalog row and an Open Work item stay two), or the
area of a Job Inventory row. Taking both sides, as the driver did until 1.25, brought back the
rows an archive run had removed and doubled a row whose status one side had changed (1.21, and
in 1.24 w3/w4 four archived rows, RD-1240-24 `Open` beside `In progress` and two count tables).
Now, per conflict:

  * a row one side removed and the other left as the base had it stays removed;
  * a row one side changed and the other left alone takes the change; changed on both sides, the
    job row with the later status word wins (Open, In progress, Partial, then Implemented or
    Blocked/No-Go), an inventory row is ours;
  * a row removed on one side and changed on the other is kept — archive-jobs.sh decides it by
    where the job file lies;
  * a row only one side has, and the base has not, is added after ours.

Afterwards a job listed twice in the same kind of list anywhere in the file keeps the copy with
the later status word (the first among equals), and an inventory area its first row. The counts
themselves are archive-jobs.sh's: integrate.sh runs it after the last merge, it recounts the Job
Inventory from both catalogs and keeps one row per job where its file lies, and its --check (in
every check.sh run) names a miscount a hand merge left. A conflict with any other line in it (a
heading, prose) stays a conflict for a person. Writes the result into <ours>; exit 0 when merged,
1 on a conflict.
"""

import os
import re
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from archive_jobs_rows import row_target, row_word  # noqa: E402

RANK = {"Open": 0, "In progress": 1, "Partial": 2, "Implemented": 3, "Blocked/No-Go": 3}
COUNT = re.compile(r"^\*{0,2}\d+\*{0,2}$")


def is_row(line):
    return line.startswith("| ") or line.startswith("- [") or not line.strip()


def key(line):
    """What a row stands for: ("job", kind, file), ("area", name), or the line itself."""
    name = row_target(line)
    if name:
        return ("job", line[0], name)
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if line.startswith("| ") and len(cells) > 1 and all(COUNT.match(c) for c in cells[1:]):
        return ("area", cells[0])
    return ("line", line)


def rank(line):
    return RANK.get(row_word(line), -1)


def later(ours, theirs):
    """Of one row changed on both sides: the job row with the later status, else ours."""
    if key(ours)[0] == "job" and rank(theirs) > rank(ours):
        return theirs
    return ours


def resolve_hunk(base, ours, theirs):
    """One conflict's lines, merged row by row against the base."""
    at_base = {key(l): l for l in base if l.strip()}
    at_theirs = {key(l): l for l in theirs if l.strip()}
    out, seen = [], set()
    for line in ours:
        if not line.strip():
            out.append(line)
            continue
        k = key(line)
        seen.add(k)
        if k not in at_theirs and at_base.get(k) == line:
            continue  # theirs removed it, ours left it alone
        if k in at_theirs and at_theirs[k] != line:
            other = at_theirs[k]
            if at_base.get(k) == line:
                line = other  # only theirs changed it
            elif at_base.get(k) != other:
                line = later(line, other)  # both changed it
        out.append(line)
    for line in theirs:
        k = key(line)
        if not line.strip() or k in seen:
            continue
        if at_base.get(k) == line:
            continue  # ours removed it, theirs left it alone
        seen.add(k)
        out.append(line)
    return out


def resolve(merged):
    """The text with every row-only conflict resolved, or None when one is not row-only."""
    out, sides, part = [], {}, None
    for line in merged.split("\n"):
        if line.startswith("<<<<<<< "):
            part, sides = "ours", {"ours": [], "base": [], "theirs": []}
        elif part and line.startswith("||||||| "):
            part = "base"
        elif part and line == "=======":
            part = "theirs"
        elif part and line.startswith(">>>>>>> "):
            if not all(is_row(l) for l in sides["ours"] + sides["theirs"]):
                return None
            out += resolve_hunk(sides["base"], sides["ours"], sides["theirs"])
            part = None
        elif part:
            sides[part].append(line)
        else:
            out.append(line)
    return "\n".join(out)


def single(text):
    """The text with each job once per kind of list (the later status) and each area once."""
    lines = text.split("\n")
    keep = {}
    for i, line in enumerate(lines):
        k = key(line)
        if k[0] == "line":
            continue
        if k not in keep or (k[0] == "job" and rank(line) > rank(lines[keep[k]])):
            keep[k] = i
    return "\n".join(l for i, l in enumerate(lines) if key(l)[0] == "line" or keep[key(l)] == i)


def main(argv):
    base, ours, theirs = argv[1:4]
    path = argv[4] if len(argv) > 4 else "docs/roadmap/jobs/README.md"
    run = subprocess.run(["git", "merge-file", "-p", "--diff3", "-L", "ours", "-L", "base",
                          "-L", "theirs", ours, base, theirs],
                         capture_output=True, text=True, check=False)
    if run.returncode < 0 or run.returncode > 127:
        print(f"rd-jobindex: {path}: git merge-file failed: {run.stderr.strip()}", file=sys.stderr)
        return 1
    merged = run.stdout if run.returncode == 0 else resolve(run.stdout)
    if merged is None:
        print(f"rd-jobindex: {path}: a conflict beyond table rows; merged as text", file=sys.stderr)
        open(ours, "w", encoding="utf-8").write(run.stdout)
        return 1
    open(ours, "w", encoding="utf-8").write(single(merged))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
