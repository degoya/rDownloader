#!/usr/bin/env python3
"""Refuses a link into a path the public repository leaves out.

    public-links.py <tree> <exclude-list>          # an exported repository tree
    public-links.py <tree> <exclude-list> --wiki   # a converted wiki tree

The exclude list is scripts/public-exclude.txt. A path it names is not in the public repository,
so a link to it is dead there: GitHub answers it with a 404, and the reader is left with a
reference to material that is deliberately not published. Two forms are checked:

  * A GitHub address of the repository — `github.com/degoya/rDownloader/blob/<ref>/<path>`,
    `tree/`, `raw/` and `raw.githubusercontent.com/degoya/rDownloader/<ref>/<path>` — in every
    text file of the tree, both modes.
  * In a repository tree only, a relative link in a Markdown file: `[text](path)`, a reference
    definition `[label]: path`, and an HTML `href="path"` or `src="path"`, resolved against the
    file's folder (a leading `/` against the root). In a wiki a relative link names a wiki page,
    which export-wiki.sh checks itself.

Code fences are skipped for the relative form: a link written out as an example is not a link.
A path named in prose or a code comment (`docs/…`) is not a link either and is not refused.

Prints one line per finding and exits 1 when there is any, 0 otherwise.
"""

import os
import posixpath
import re
import sys

GITHUB = re.compile(
    r"(?:github\.com/degoya/rDownloader/(?:blob|tree|raw)|raw\.githubusercontent\.com/degoya/rDownloader)"
    r"/[^/\s]+/([^\s)\"'<>#?`\]]+)",
    re.IGNORECASE,
)
INLINE = re.compile(r"\]\(\s*<?([^)\s>]+)")
REFERENCE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)")
HTML = re.compile(r"\b(?:href|src)\s*=\s*[\"']([^\"']+)[\"']", re.IGNORECASE)
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")


def excluded_paths(listing):
    """The entries of the exclude list, the way export-public.sh reads them."""
    entries = []
    with open(listing, encoding="utf-8") as handle:
        for line in handle:
            path = line.split("#", 1)[0].strip().rstrip("/")
            if path:
                entries.append(path)
    return entries


def excluded_by(path, entries):
    for entry in entries:
        if path == entry or path.startswith(entry + "/"):
            return entry
    return None


def relative_target(page, target):
    """The repository path a relative link on `page` points at, or None for anything else."""
    if not target or target.startswith(("#", "//")) or SCHEME.match(target):
        return None
    path = re.split(r"[#?]", target, maxsplit=1)[0]
    if not path:
        return None
    if path.startswith("/"):
        resolved = posixpath.normpath(path.lstrip("/"))
    else:
        resolved = posixpath.normpath(posixpath.join(posixpath.dirname(page), path))
    if resolved == ".." or resolved.startswith("../"):
        return None
    return resolved


def main(argv):
    if len(argv) not in (3, 4) or (len(argv) == 4 and argv[3] != "--wiki"):
        print("usage: public-links.py <tree> <exclude-list> [--wiki]", file=sys.stderr)
        return 2
    root, entries, wiki = argv[1], excluded_paths(argv[2]), len(argv) == 4
    problems = []
    for directory, folders, files in os.walk(root):
        folders[:] = sorted(f for f in folders if f != ".git")
        for name in sorted(files):
            full = os.path.join(directory, name)
            page = posixpath.relpath(full, root).replace(os.sep, "/")
            try:
                with open(full, encoding="utf-8") as handle:
                    lines = handle.read().split("\n")
            except (UnicodeDecodeError, OSError):
                continue
            markdown = name.endswith(".md") and not wiki
            fenced = False
            for number, line in enumerate(lines, 1):
                for match in GITHUB.finditer(line):
                    entry = excluded_by(match.group(1).rstrip("/.,;:"), entries)
                    if entry:
                        problems.append(f"{page}:{number}: links to {entry}, which is not public: {match.group(0)}")
                if not markdown:
                    continue
                if line.lstrip().startswith(("```", "~~~")):
                    fenced = not fenced
                    continue
                if fenced:
                    continue
                targets = [m.group(1) for m in INLINE.finditer(line)]
                targets += [m.group(1) for m in HTML.finditer(line)]
                reference = REFERENCE.match(line)
                if reference:
                    targets.append(reference.group(1))
                for target in targets:
                    resolved = relative_target(page, target)
                    entry = resolved and excluded_by(resolved, entries)
                    if entry:
                        problems.append(f"{page}:{number}: links to {entry}, which is not public: {target}")
    for problem in problems:
        print(problem)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
