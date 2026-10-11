#!/usr/bin/env python3
"""Refuses a public mention of another download manager outside the allowlist (RD-1250-11).

    public-names.py <checkout> <exclude-list> <allowlist>

The owner, 2026-10-11: nothing public names JDownloader as the model or the source of a
behaviour, written out (`jdownloader`, any case) or abbreviated (`JD` as a word, case-sensitive,
so an identifier or a hex string carrying `jd` is not read) or cited from its source tree (a
`.java` file, `svn_trunk`, `jd.plugins`, the mirror's owner). Public is every tracked file but the paths the exclude list
(scripts/public-exclude.txt) leaves out of the public repository. What may still name it — a
format and its interop, a released version's history, a migration sqlx checksums — is listed
in the allowlist (scripts/public-names-allow.txt), one entry per line:

    <path or glob> | <regular expression> | <reason>

A mention passes when an entry's path matches its file and the expression is found in its
line. The expression `@released` stands for the released history instead: in CHANGELOG.md a
mention passes under a `## [X.Y.Z] - date` heading and fails under `## [Unreleased]`; in
RELEASE-NOTES.md it passes in a `## X.Y.Z` section that carries no draft comment and whose
version CHANGELOG.md dates, and fails in any other. An entry that no mention needs any more is
a finding too, so the list cannot outgrow the tree.

Searches the working tree's tracked files (`git grep`), binaries left out. Prints one line per
finding and exits 1 when there is any, 0 otherwise.
"""

import fnmatch
import re
import subprocess
import sys

# (git grep arguments, the name a finding gives)
TERMS = (
    (["-i", "-e", "jdownloader"], "jdownloader"),
    (["-w", "-e", "JD"], "JD"),
    (["-E", "-e", r"\.java\b|svn_trunk|jd\.plugins|mycodedoesnotcompile2"], "a source-mirror path"),
)
RELEASED = "@released"
CHANGELOG_SECTION = re.compile(r"^## \[([^\]]+)\](?:\s+-\s+(\S+))?")
NOTES_SECTION = re.compile(r"^## (\S+)\s*$")


def excluded_paths(listing):
    """The entries of the exclude list, the way export-public.sh reads them."""
    entries = []
    with open(listing, encoding="utf-8") as handle:
        for line in handle:
            path = line.split("#", 1)[0].strip().rstrip("/")
            if path:
                entries.append(path)
    return entries


def is_excluded(path, entries):
    return any(path == entry or path.startswith(entry + "/") for entry in entries)


def allow_entries(listing):
    """[(line number, path glob, compiled expression or RELEASED, reason)], or findings."""
    entries, findings = [], []
    with open(listing, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            text = line.strip()
            if not text or text.startswith("#"):
                continue
            parts = text.split(" | ", 1)
            if len(parts) != 2 or " | " not in parts[1]:
                findings.append(f"{listing}:{number}: not '<path> | <expression> | <reason>'")
                continue
            path = parts[0].strip()
            expression, reason = (part.strip() for part in parts[1].rsplit(" | ", 1))
            if not path or not expression or not reason:
                findings.append(f"{listing}:{number}: path, expression and reason are all required")
                continue
            if expression == RELEASED:
                entries.append((number, path, RELEASED, reason))
                continue
            try:
                entries.append((number, path, re.compile(expression), reason))
            except re.error as error:
                findings.append(f"{listing}:{number}: bad expression: {error}")
    return entries, findings


def mentions(checkout):
    """{(path, line number): (text, term)} of every tracked text line naming one of TERMS."""
    found = {}
    for arguments, term in TERMS:
        result = subprocess.run(
            ["git", "-C", checkout, "grep", "-n", "-I", "--full-name", *arguments],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode not in (0, 1):
            sys.exit(f"git grep failed: {result.stderr.strip()}")
        for line in result.stdout.splitlines():
            path, number, text = line.split(":", 2)
            found.setdefault((path, int(number)), (text, term))
    return found


def read_lines(checkout, path):
    try:
        with open(f"{checkout}/{path}", encoding="utf-8") as handle:
            return handle.read().splitlines()
    except OSError:
        return []


def changelog_released(checkout):
    """{line number: version or None} for CHANGELOG.md, and the set of dated versions."""
    sections, dated, current = {}, set(), None
    for number, line in enumerate(read_lines(checkout, "CHANGELOG.md"), 1):
        heading = CHANGELOG_SECTION.match(line)
        if heading:
            current = heading.group(1) if heading.group(2) else None
            if current:
                dated.add(current)
        sections[number] = current
    return sections, dated


def notes_released(checkout, dated):
    """{line number: True when the section is a released version} for RELEASE-NOTES.md."""
    lines = read_lines(checkout, "RELEASE-NOTES.md")
    starts = [(n, m.group(1)) for n, line in enumerate(lines, 1) if (m := NOTES_SECTION.match(line))]
    released = {}
    for index, (start, version) in enumerate(starts):
        end = starts[index + 1][0] if index + 1 < len(starts) else len(lines) + 1
        body = "\n".join(lines[start : end - 1])
        draft = any("draft" in comment.lower() for comment in re.findall(r"<!--.*?-->", body, re.S))
        for number in range(start, end):
            released[number] = version in dated and not draft
    return released


def main(argv):
    if len(argv) != 4:
        print("usage: public-names.py <checkout> <exclude-list> <allowlist>", file=sys.stderr)
        return 2
    checkout, excluded = argv[1], excluded_paths(argv[2])
    entries, findings = allow_entries(argv[3])
    changelog, dated = changelog_released(checkout)
    notes = notes_released(checkout, dated)
    used = set()
    for (path, number), (text, term) in sorted(mentions(checkout).items()):
        if is_excluded(path, excluded):
            continue
        allowed = False
        for line_number, glob, expression, _ in entries:
            if not fnmatch.fnmatchcase(path, glob):
                continue
            if expression is RELEASED:
                if path == "CHANGELOG.md":
                    allowed = changelog.get(number) is not None
                elif path == "RELEASE-NOTES.md":
                    allowed = notes.get(number, False)
            else:
                allowed = expression.search(text) is not None
            if allowed:
                used.add(line_number)
                break
        if not allowed:
            findings.append(f"{path}:{number}: names {term} outside the allowlist: {text.strip()}")
    for line_number, glob, _, reason in entries:
        if line_number not in used:
            findings.append(f"{argv[3]}:{line_number}: no mention needs this entry any more ({glob}: {reason})")
    for finding in findings:
        print(finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
