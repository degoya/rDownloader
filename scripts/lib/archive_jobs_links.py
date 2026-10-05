"""The link side of scripts/lib/archive-jobs.py (RD-140-19): every relative link and plain path
mention of a moved job file, rewritten across the tracked files. The rules are in
archive-jobs.py's docstring."""

import os
import posixpath
import re
import urllib.parse

from archive_jobs_status import ARCHIVE, JOBS, git

EXCLUDED_PREFIXES = ("crates/rd-db/migrations/", "plugins/")
EXCLUDED_FILES = ("docs/ideas_and_infos.md",)

INLINE = re.compile(r"(\]\(\s*<?)([^)\s>]+)")
REFERENCE = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*<?)(\S+?)(?=>?(?:\s|$))")
HTML = re.compile(r"(\b(?:href|src)\s*=\s*[\"'])([^\"']+)", re.IGNORECASE)
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")


def rewrite_references(repo, moved):
    old2new = {f"{JOBS}/{n}": f"{ARCHIVE}/{n}" for n in moved}
    new2old = {v: k for k, v in old2new.items()}

    def existed(path):
        if path in old2new:
            return True
        if path in new2old:
            return False
        return os.path.exists(os.path.join(repo, path))

    def fix(page, target):
        if SCHEME.match(target) or target.startswith(("#", "/")):
            return target
        page_old = new2old.get(page, page)
        cut = min([i for i in (target.find("#"), target.find("?")) if i != -1] or [len(target)])
        path, suffix = target[:cut], target[cut:]
        if not path:
            return target
        decoded = urllib.parse.unquote(path)
        resolved = posixpath.normpath(posixpath.join(posixpath.dirname(page_old), decoded))
        if not existed(resolved):
            return target
        resolved_new = old2new.get(resolved, resolved)
        if page_old == page and resolved == resolved_new:
            return target
        rel = posixpath.relpath(resolved_new, posixpath.dirname(page))
        if path.startswith("./") and not rel.startswith("../"):
            rel = "./" + rel
        if decoded.endswith("/") and not rel.endswith("/"):
            rel += "/"
        if decoded != path:
            rel = urllib.parse.quote(rel)
        counts["links"] += rel != path
        return rel + suffix

    alternatives = "|".join(re.escape(n) for n in sorted(moved, key=len, reverse=True))
    mention = re.compile(r"(roadmap/jobs/)(" + alternatives + r")(?![\w-])")
    ids = {n.split("-")[0] + "-" + n.split("-")[1] for n in moved if re.match(r"^\d{3,4}-\d{2}-", n)}
    glob = re.compile(r"(roadmap/jobs/)(\d{3,4}-\d{2})(-\*\.md)")
    counts = {"links": 0, "mentions": 0, "files": 0}

    def mentioned(m):
        counts["mentions"] += 1
        return m.group(1) + "archive/" + m.group(2)

    def globbed(m):
        if m.group(2) not in ids:
            return m.group(0)
        counts["mentions"] += 1
        return m.group(1) + "archive/" + m.group(2) + m.group(3)

    for page in git(repo, "ls-files", "-co", "--exclude-standard").splitlines():
        full = os.path.join(repo, page)
        if page in EXCLUDED_FILES or page.startswith(EXCLUDED_PREFIXES) or not os.path.isfile(full):
            continue
        try:
            text = open(full, encoding="utf-8").read()
        except UnicodeDecodeError:
            continue
        original = text
        if page.endswith(".md"):
            out, fence = [], False
            for line in text.split("\n"):
                if line.lstrip().startswith(("```", "~~~")):
                    fence = not fence
                elif not fence:
                    line = INLINE.sub(lambda m: m.group(1) + fix(page, m.group(2)), line)
                    line = HTML.sub(lambda m: m.group(1) + fix(page, m.group(2)), line)
                    line = REFERENCE.sub(lambda m: m.group(1) + fix(page, m.group(2)), line)
                out.append(line)
            text = "\n".join(out)
        text = glob.sub(globbed, mention.sub(mentioned, text))
        if text != original:
            open(full, "w", encoding="utf-8").write(text)
            counts["files"] += 1
    return counts
