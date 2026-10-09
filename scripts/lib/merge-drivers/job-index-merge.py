#!/usr/bin/env python3
"""The `rd-jobindex` merge driver for the two job indexes, docs/roadmap/jobs/README.md and its
archive/README.md (RD-1220-01), which .gitattributes names and scripts/integrate.sh registers.

    job-index-merge.py <base> <ours> <theirs> [<path>]    # git's %O %A %B %P

Both indexes are tables of rows, one per job, that every branch touches: a new job, a status
word, the Job Inventory's counts, and archive-jobs.sh moving finished rows between the two. git
merges them as text, which conflicts in nearly every wave (nine times in the 1.9.1 integration),
and a conflict resolved by keeping both sides brought archived rows back and doubled others (1.21).

The driver merges as text and resolves a conflict whose two sides are only table rows (`| `),
list items (`- [`) and blank lines: our lines, then theirs that we lack. A doubled or misplaced
row that leaves is no longer the driver's to judge but the job files': scripts/archive-jobs.sh,
which integrate.sh runs after the last merge, keeps one row per job where its file lies and
recounts the Job Inventory, and its --check names whatever is left. A conflict with any other
line in it (a heading, prose) stays a conflict for a person. Writes the result into <ours>; exit 0
when merged, 1 on a conflict.
"""

import subprocess
import sys


def is_row(line):
    return line.startswith("| ") or line.startswith("- [") or not line.strip()


def resolve(merged):
    """The text with every row-only conflict resolved, or None when one is not row-only."""
    out, ours, theirs, part = [], [], [], None
    for line in merged.split("\n"):
        if line.startswith("<<<<<<< "):
            part, ours, theirs = "ours", [], []
        elif part and line.startswith("||||||| "):
            part = "base"
        elif part and line == "=======":
            part = "theirs"
        elif part and line.startswith(">>>>>>> "):
            if not all(is_row(l) for l in ours + theirs):
                return None
            out += ours + [l for l in theirs if l not in ours]
            part = None
        elif part == "ours":
            ours.append(line)
        elif part == "theirs":
            theirs.append(line)
        elif part is None:
            out.append(line)
    return "\n".join(out)


def main(argv):
    base, ours, theirs = argv[1:4]
    path = argv[4] if len(argv) > 4 else "docs/roadmap/jobs/README.md"
    run = subprocess.run(["git", "merge-file", "-p", "-L", "ours", "-L", "base", "-L", "theirs",
                          ours, base, theirs], capture_output=True, text=True, check=False)
    if run.returncode < 0 or run.returncode > 127:
        print(f"rd-jobindex: {path}: git merge-file failed: {run.stderr.strip()}", file=sys.stderr)
        return 1
    merged = run.stdout if run.returncode == 0 else resolve(run.stdout)
    if merged is None:
        print(f"rd-jobindex: {path}: a conflict beyond table rows; merged as text", file=sys.stderr)
        open(ours, "w", encoding="utf-8").write(run.stdout)
        return 1
    open(ours, "w", encoding="utf-8").write(merged)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
