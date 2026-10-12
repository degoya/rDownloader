"""The counted status lines of scripts/lib/archive-jobs.py (RD-1270-17).

A job file that collects open items from many others - RD-1120-24 and the older live-check
jobs - states in its status line how many are left. Written by hand, that figure was stale
within days (RD-1120-24 said 81 open points while the file held 131 open boxes). A status line
that carries `gezählt: <n> offen, <m> abgehakt` therefore has the two numbers counted from the
file's own checkboxes (`- [ ]` and `- [x]`, at any indentation): a run of archive-jobs.sh writes
them, --check names a line whose numbers the file does not bear out. Only docs/roadmap/jobs/
itself is read; a file in archive/ is finished and keeps its last count.
"""

import os
import re

from archive_jobs_status import JOBS, STATUS_PREFIX

# `gezählt` is the German "counted": the job files are German, this source stays ASCII.
COUNTED = re.compile("gezählt: (\\d+) offen, (\\d+) abgehakt")
OPEN_BOX = re.compile(r"^\s*[-*] \[ \]", re.MULTILINE)
DONE_BOX = re.compile(r"^\s*[-*] \[[xX]\]", re.MULTILINE)


def counted_status(text):
    """`text` with the counted status line brought up to date (the same text when it has none
    or is right), and (written, counted) for the line."""
    lines = text.split("\n")
    at = next((i for i, l in enumerate(lines) if l.strip().startswith(STATUS_PREFIX)), None)
    if at is None or not COUNTED.search(lines[at]):
        return text, None
    fresh = f"gezählt: {len(OPEN_BOX.findall(text))} offen, {len(DONE_BOX.findall(text))} abgehakt"
    written = COUNTED.search(lines[at]).group(0)
    if written == fresh:
        return text, None
    lines[at] = COUNTED.sub(fresh, lines[at], count=1)
    return "\n".join(lines), (written, fresh)


def stale_counts(repo, write):
    """[(path, written, counted)] for every counted status line in docs/roadmap/jobs/ that
    disagrees with its file's checkboxes; with `write` each is corrected in place."""
    stale = []
    directory = os.path.join(repo, JOBS)
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if not name.endswith(".md") or not os.path.isfile(path):
            continue
        text = open(path, encoding="utf-8").read()
        fresh, change = counted_status(text)
        if change is None:
            continue
        stale.append((f"{JOBS}/{name}", *change))
        if write:
            open(path, "w", encoding="utf-8").write(fresh)
    return stale
