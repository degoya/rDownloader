"""The status side of scripts/lib/archive-jobs.py (RD-140-19): where the job files lie, the
status words, and which files are due for the archive, open again in it or carry an unusable
status line. The rules are in archive-jobs.py's docstring."""

import os
import re
import subprocess

JOBS = "docs/roadmap/jobs"
ARCHIVE = f"{JOBS}/archive"
FINISHED_WORDS = ("Implemented", "Blocked/No-Go")
STATUS_WORDS = ("Blocked/No-Go", "In progress", "Implemented", "Partial", "Open")
STATUS_PREFIX = "- **Status:**"
WORKING_FILE = re.compile(r"^(\d)(\d{1,2})(\d)-00-.*\.md$")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, check=True).stdout


def status_line(text):
    return next((l.strip() for l in text.splitlines() if l.strip().startswith(STATUS_PREFIX)), None)


def status_word(line):
    value = line[len(STATUS_PREFIX):].strip()
    return next((w for w in STATUS_WORDS if value.startswith(w)), value.split(" ")[0])


def status_word_problem(name, text):
    """Why the status line of the job file `name` is unusable, or None when it is fine. A word
    must stand alone: `Openish` is not `Open`, `Open (…)`, `Open / …` and `Open, …` are."""
    line = status_line(text)
    if line is None:
        if WORKING_FILE.match(name) or name == "README.md":
            return None
        return f"no `{STATUS_PREFIX}` line"
    value = line[len(STATUS_PREFIX):].strip()
    for word in STATUS_WORDS:
        rest = value[len(word):] if value.startswith(word) else None
        if rest is not None and not (rest[:1].isalnum() or rest[:1] == "_"):
            return None
    written = re.split(r"[\s(,;:]", value, maxsplit=1)[0]
    return f"status word `{written}` is not one of {', '.join(f'`{w}`' for w in sorted(STATUS_WORDS))}"


def bad_status_files(repo):
    """(path, problem) for every job file here or in archive/ with an unusable status line."""
    bad = []
    for directory in (JOBS, ARCHIVE):
        if not os.path.isdir(os.path.join(repo, directory)):
            continue
        for name in sorted(os.listdir(os.path.join(repo, directory))):
            path = os.path.join(repo, directory, name)
            if not name.endswith(".md") or not os.path.isfile(path):
                continue
            problem = status_word_problem(name, open(path, encoding="utf-8").read())
            if problem:
                bad.append((f"{directory}/{name}", problem))
    return bad


def due_files(repo, release):
    """(name, reason) for every file of docs/roadmap/jobs/ that belongs in the archive."""
    tags = set(git(repo, "tag", "-l", "v*").split())
    due = []
    for name in sorted(os.listdir(os.path.join(repo, JOBS))):
        path = os.path.join(repo, JOBS, name)
        if not name.endswith(".md") or name == "README.md" or not os.path.isfile(path):
            continue
        line = status_line(open(path, encoding="utf-8").read())
        if line is not None:
            if status_word(line) in FINISHED_WORDS:
                due.append((name, status_word(line)))
            continue
        m = WORKING_FILE.match(name)
        if m:
            version = ".".join(m.groups())
            if f"v{version}" in tags or version == release:
                due.append((name, f"working file of {version}"))
    return due


def misplaced_files(repo):
    """(name, word) for every file of archive/ whose status says it is not finished."""
    directory = os.path.join(repo, ARCHIVE)
    if not os.path.isdir(directory):
        return []
    misplaced = []
    for name in sorted(os.listdir(directory)):
        if not name.endswith(".md") or name == "README.md":
            continue
        line = status_line(open(os.path.join(directory, name), encoding="utf-8").read())
        if line is not None and status_word(line) not in FINISHED_WORDS:
            misplaced.append((name, status_word(line)))
    return misplaced
