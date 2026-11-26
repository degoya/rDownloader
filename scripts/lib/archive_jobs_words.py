"""The status words of the job index (scripts/lib/archive-jobs.py, RD-1480-24): the catalog's last
cell and the Open Work item of a job, held against the job file, which is the status truth.

A run writes the file's word into both; what follows the word stays when the word was right and
leaves with it when it was not. --check names a word that disagrees without failing: a branch sets
its job file to `In progress` and leaves the index alone (the wave rules), so the two may differ
until the next run - the release's at the latest."""

import os
import re

from archive_jobs_status import JOBS, STATUS_WORDS, status_line, status_word

LINK = re.compile(r"\]\(\./([^)/#]+\.md)\)")


def index_word(text):
    """The status word an index entry starts with, or None."""
    return next((w for w in STATUS_WORDS if re.match(re.escape(w) + r"(?!\w)", text)), None)


def fresh_line(line, word):
    """The index line with the file's `word`, or the line as it is when it has no status to set."""
    if line.startswith("| ["):
        cells = line.split(" | ")
        if len(cells) < 3 or index_word(cells[-1]) == word:
            return line
        return " | ".join(cells[:-1] + [word + " |"])
    head, mark, rest = line.partition(") — ")
    written = index_word(rest) if mark else None
    if written is None or written == word:
        return line
    _, dash, note = rest.partition(" — ")
    return head + mark + word + dash + note


def stale_words(repo, write):
    """[(job file, written line, fresh line)] for every index entry whose word is not its file's."""
    path = os.path.join(repo, JOBS, "README.md")
    lines = open(path, encoding="utf-8").read().split("\n")
    stale = []
    for number, line in enumerate(lines):
        link = LINK.search(line) if line.startswith(("| [", "- [")) else None
        job = os.path.join(repo, JOBS, link.group(1)) if link else None
        if job is None or not os.path.isfile(job):
            continue
        status = status_line(open(job, encoding="utf-8").read())
        fresh = fresh_line(line, status_word(status)) if status else line
        if fresh != line:
            stale.append((link.group(1), line, fresh))
            lines[number] = fresh
    if stale and write:
        open(path, "w", encoding="utf-8").write("\n".join(lines))
    return stale
