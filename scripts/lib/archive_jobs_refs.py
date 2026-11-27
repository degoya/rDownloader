"""The open boxes of the archive (RD-1490-01), for scripts/lib/archive-jobs.py --check.

An archived job is finished, so a box still open in it says where its rest went or that it
lapsed: its paragraph — the `- [ ]` line and the indented lines below it — carries
`→ offen in RD-<id>`, or its text is struck through (`- [ ] ~~…~~`). scripts/job-evidence.sh
writes the mark under every box it leaves open when it closes a job. A file whose status is
`Blocked/No-Go` is exempt (the job was never built); a working file without a status line
(`<NNN>-00-*.md`) is not. Every `→ offen in RD-<id>` in the archive names a job file that exists,
here or in docs/roadmap/jobs/.
"""

import os
import re

from archive_jobs_status import ARCHIVE, JOBS, status_line, status_word

EXEMPT_WORD = "Blocked/No-Go"
OPEN_BOX = re.compile(r"^(\s*)[-*] \[ \] ?(.*)$")
INDENTED = re.compile(r"^(\s+)\S")
FENCE = re.compile(r"^\s*(```|~~~)")
MARK = re.compile(r"→ offen in RD-")
REFERENCE = re.compile(r"→ offen in RD-(\d+-\d+|r\d+)(?![\w-])")


def open_boxes(text):
    """[(line number, box text, paragraph)] for every open box outside a code fence."""
    lines = text.split("\n")
    boxes, fenced = [], False
    for i, line in enumerate(lines):
        if FENCE.match(line):
            fenced = not fenced
            continue
        m = None if fenced else OPEN_BOX.match(line)
        if not m:
            continue
        depth, paragraph = len(m.group(1)), [line]
        for follow in lines[i + 1:]:
            indent = INDENTED.match(follow)
            if not indent or len(indent.group(1)) <= depth or FENCE.match(follow):
                break
            paragraph.append(follow)
        boxes.append((i + 1, m.group(2).strip(), "\n".join(paragraph)))
    return boxes


def is_referenced(box_text, paragraph):
    return box_text.startswith("~~") or bool(REFERENCE.search(paragraph))


def job_ids(repo):
    """The ids (`1120-24`, `r01`) of every job file here or in archive/."""
    ids = set()
    for directory in (JOBS, ARCHIVE):
        path = os.path.join(repo, directory)
        if not os.path.isdir(path):
            continue
        for name in os.listdir(path):
            m = re.match(r"^(\d+-\d+|r\d+)-.*\.md$", name)
            if m:
                ids.add(m.group(1))
    return ids


def archive_ref_problems(repo):
    """[problem] for every unreferenced open box of the archive and every `→ offen in RD-…` there
    that names no job file, each as `path:line: …`."""
    directory = os.path.join(repo, ARCHIVE)
    if not os.path.isdir(directory):
        return []
    known, problems = job_ids(repo), []
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if not name.endswith(".md") or name == "README.md" or not os.path.isfile(path):
            continue
        text = open(path, encoding="utf-8").read()
        line = status_line(text)
        if line is None or status_word(line) != EXEMPT_WORD:
            for number, box_text, paragraph in open_boxes(text):
                if not is_referenced(box_text, paragraph):
                    problems.append(f"unreferenced: {ARCHIVE}/{name}:{number}: {box_text}")
        for number, row in enumerate(text.split("\n"), start=1):
            for mark in MARK.finditer(row):
                # Only a job id is a reference; prose that quotes the form (`RD-…`, `RD-<id>`)
                # names none and is left alone, as in the archived RD-1490-01 that set the rule.
                ref = REFERENCE.match(row, mark.start())
                if ref is not None and ref.group(1) not in known:
                    problems.append(f"dangling: {ARCHIVE}/{name}:{number}: "
                                    f"`→ offen in RD-{ref.group(1)}` names no job file")
    return problems
