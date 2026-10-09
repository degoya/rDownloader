"""The rows side of scripts/lib/archive-jobs.py (RD-1220-01): each index lists a job once, and only
where its file lies — docs/roadmap/jobs/README.md (its Open Work and its Catalog) the files of
docs/roadmap/jobs/, archive/README.md those of archive/.

A merge of a branch that forked before archive-jobs moved finished jobs, or a conflict resolved by
keeping both sides, breaks that: in 1.21 the archived 1.20 rows came back into the open index and
the 1210 rows stood there twice, `Open` beside `In progress`, and the next archive run copied the
doubles into the archive. The job files are the truth, so most of it mends itself:

  * a row twice in one list keeps the copy whose status word is its file's (else the first);
  * a row whose file lies in the other directory, where the other index has its row, goes.

What the files cannot decide is only named: a row whose file lies in the other directory without
a row there, and a row whose job file does not exist.
"""

import os
import re

from archive_jobs_status import ARCHIVE, JOBS, status_line, status_word

# The open index lists a job in two places; the archive's index has only its catalog.
INDEXES = ((JOBS, ("## Open Work", "## Catalog")), (ARCHIVE, None))
ROW_LINK = re.compile(r"\]\(\./([^)#/]+\.md)")
ITEM_WORD = re.compile(r"^- \[.*?\]\([^)]*\) — ([^—]+?)(?: — |$)")


def row_target(line):
    """The job file a catalog row (`| [`) or an Open Work item (`- [`) links, or None."""
    if not (line.startswith("| [") or line.startswith("- [")):
        return None
    m = ROW_LINK.search(line)
    return m.group(1) if m else None


def row_word(line):
    if line.startswith("| "):
        return line.strip().strip("|").split("|")[-1].strip()
    m = ITEM_WORD.match(line)
    return m.group(1).strip() if m else ""


def file_word(repo, directory, name):
    line = status_line(open(os.path.join(repo, directory, name), encoding="utf-8").read())
    return status_word(line) if line else "Arbeitsdatei"


def ranges(lines, titles):
    """(start, end) of each listed section; the whole file without titles."""
    if titles is None:
        return [(0, len(lines))]
    found = []
    for title in titles:
        if title in lines:
            start = lines.index(title)
            end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
                       len(lines))
            found.append((start, end))
    return found


def catalog_names(lines, titles):
    spans = ranges(lines, titles[-1:] if titles else None)
    return {row_target(lines[i]) for start, end in spans for i in range(start, end)} - {None}


def row_problems(repo, write=False):
    """[(problem, mended)] for both indexes. With write, every mendable one is mended in the file;
    without, nothing is written and `mended` says only whether a run would mend it."""
    loaded = {}
    for directory, titles in INDEXES:
        path = os.path.join(repo, directory, "README.md")
        if os.path.isfile(path):
            loaded[directory] = (path, titles, open(path, encoding="utf-8").read().split("\n"))
    problems = []
    for directory, (path, titles, lines) in loaded.items():
        other = ARCHIVE if directory == JOBS else JOBS
        other_rows = catalog_names(loaded[other][2], loaded[other][1]) if other in loaded else set()
        index = f"{directory}/README.md"
        drop = set()
        for start, end in ranges(lines, titles):
            where = lines[start] if titles else "its catalog"
            at = {}
            for i in range(start, end):
                name = row_target(lines[i])
                if name:
                    at.setdefault(name, []).append(i)
            for name, rows in at.items():
                if not os.path.isfile(os.path.join(repo, directory, name)):
                    there = os.path.isfile(os.path.join(repo, other, name))
                    if there and name in other_rows:
                        problems.append((f"misplaced: {index} lists {name} under {where}, which lies "
                                         f"in {other}/ with its row there", True))
                        drop.update(rows)
                    elif there:
                        problems.append((f"misplaced: {index} lists {name} under {where}, which lies "
                                         f"in {other}/ without a row there", False))
                    else:
                        problems.append((f"misplaced: {index} lists {name} under {where}, and no job "
                                         "file has that name", False))
                    continue
                if len(rows) > 1:
                    word = file_word(repo, directory, name)
                    keep = next((i for i in rows if row_word(lines[i]) == word), rows[0])
                    problems.append((f"twice: {index} lists {name} {len(rows)} times under {where}",
                                     True))
                    drop.update(i for i in rows if i != keep)
        if write and drop:
            lines[:] = [line for i, line in enumerate(lines) if i not in drop]
            open(path, "w", encoding="utf-8").write("\n".join(lines))
    return problems
