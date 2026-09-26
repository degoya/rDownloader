#!/usr/bin/env python3
"""Moves finished roadmap job files into docs/roadmap/jobs/archive/ (RD-140-19).

    archive-jobs.py <repo> [--check] [--release X.Y.Z]

A job file directly in docs/roadmap/jobs/ is due when its first `- **Status:**` line begins with
`Implemented` or `Blocked/No-Go`. A working file without a status line — `<NNN>-00-*.md` such as
`120-00-release-koordination.md` or `108-00-abnahme-checkliste.md` — is due once its release is
tagged (`v1.2.0` for `120`), or is the release named by --release, which the release pipeline
passes because it runs this before its own tag exists.

For each due file, in one pass:

  * `git mv` into archive/, the file name unchanged.
  * Every relative Markdown link in every tracked .md file (inline, reference definition, HTML
    href/src) is resolved against the page's location before the move and re-expressed from its
    location after it, so a moved page's own links and every link to a moved page stay right.
    A link whose target did not exist before is left as it was: it is not a file link (`/login`)
    or it was already dead. Plain path mentions `roadmap/jobs/<file>` in any tracked text file
    get `archive/` inserted.
  * Its row leaves the catalog of docs/roadmap/jobs/README.md for the same milestone's table in
    archive/README.md; a list item in the index's Open Work is dropped; a milestone left without
    rows or items loses its heading; the Job Inventory table is recounted.

Left alone on purpose: docs/ideas_and_infos.md (the owner's notepad), crates/rd-db/migrations/
(sqlx checksums every byte) and plugins/ (a changed plugin source needs a version bump and a
rebuild, which a documentation step must not cause).

With nothing due it writes nothing and exits 0. --check writes nothing either: it names what is
due, and every file in archive/ whose status is open again (`Open`, `In progress`, `Partial` —
moving such a job back is left to a person, with its row), and exits 1 when there is either.
scripts/check.sh runs it that way, so the layout cannot drift in either direction.

Exit 2 is a refusal: uncommitted changes under docs/roadmap/jobs/ that this run would mix into,
or a jobs directory without its index. A tree without docs/roadmap/jobs/ at all — the public
export — has nothing to archive and exits 0.
"""

import os
import posixpath
import re
import subprocess
import sys
import urllib.parse

JOBS = "docs/roadmap/jobs"
ARCHIVE = f"{JOBS}/archive"
FINISHED_WORDS = ("Implemented", "Blocked/No-Go")
STATUS_WORDS = ("Blocked/No-Go", "In progress", "Implemented", "Partial", "Open")
STATUS_PREFIX = "- **Status:**"
EXCLUDED_PREFIXES = ("crates/rd-db/migrations/", "plugins/")
EXCLUDED_FILES = ("docs/ideas_and_infos.md",)
ARCHIVE_HEADER = """# rDownloader Roadmap Jobs — Archive

The finished job files, moved here by `scripts/archive-jobs.sh`. What is still open is in
[the index](../README.md).
"""

INLINE = re.compile(r"(\]\(\s*<?)([^)\s>]+)")
REFERENCE = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*<?)(\S+?)(?=>?(?:\s|$))")
HTML = re.compile(r"(\b(?:href|src)\s*=\s*[\"'])([^\"']+)", re.IGNORECASE)
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
WORKING_FILE = re.compile(r"^(\d)(\d)(\d)-00-.*\.md$")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, check=True).stdout


def status_line(text):
    return next((l.strip() for l in text.splitlines() if l.strip().startswith(STATUS_PREFIX)), None)


def status_word(line):
    value = line[len(STATUS_PREFIX):].strip()
    return next((w for w in STATUS_WORDS if value.startswith(w)), value.split(" ")[0])


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


# ---------------------------------------------------------------------------------------------
# Links
# ---------------------------------------------------------------------------------------------

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
    ids = {n.split("-")[0] + "-" + n.split("-")[1] for n in moved if re.match(r"^\d{3}-\d{2}-", n)}
    glob = re.compile(r"(roadmap/jobs/)(\d{3}-\d{2})(-\*\.md)")
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


# ---------------------------------------------------------------------------------------------
# The two indexes
# ---------------------------------------------------------------------------------------------

def milestone_key(heading):
    """`### Milestone 1.0.8 — Title` -> `Milestone 1.0.8`; `### Research Backlog` stays whole."""
    return heading.lstrip("#").strip().split(" — ")[0].strip()


def milestone_order(key):
    m = re.match(r"^Milestone (\d+(?:\.\d+)*)$", key)
    return tuple(int(p) for p in m.group(1).split(".")) if m else (10**6,)


def section_bounds(lines, title):
    start = lines.index(title)
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return start, end


def read_blocks(lines):
    """The `### ` blocks of a catalog: [(heading, [lines])], preamble under key None."""
    blocks, current = [(None, [])], None
    for line in lines:
        if line.startswith("### "):
            blocks.append((line, []))
        else:
            blocks[-1][1].append(line)
    return blocks


def new_row(repo, name):
    """A catalog row for a finished job the index had no row for."""
    text = open(os.path.join(repo, ARCHIVE, name), encoding="utf-8").read()
    title = next((l[2:].strip() for l in text.splitlines() if l.startswith("# ")), name)
    priority = next((l.split(":**", 1)[1].strip() for l in text.splitlines()
                     if l.startswith("- **Priority:**")), "—")
    line = status_line(text)
    return f"| [{title}](./{name}) | {priority} | {status_word(line) if line else 'Arbeitsdatei'} |"


def job_milestone(repo, name):
    text = open(os.path.join(repo, ARCHIVE, name), encoding="utf-8").read()
    m = re.search(r"^- \*\*Milestone:\*\*\s*(\d+(?:\.\d+)*)", text, re.MULTILINE)
    return f"Milestone {m.group(1)}" if m else None


def update_indexes(repo, moved):
    index_path = os.path.join(repo, JOBS, "README.md")
    archive_path = os.path.join(repo, ARCHIVE, "README.md")
    lines = open(index_path, encoding="utf-8").read().split("\n")
    archived_link = re.compile(r"\]\(\./archive/([^)#]+)")

    # Open Work: drop the items of moved jobs, then a milestone label left with nothing under it.
    if "## Open Work" in lines:
        start, end = section_bounds(lines, "## Open Work")
        body = [l for l in lines[start:end]
                if not (l.startswith("- [") and archived_link.search(l)
                        and archived_link.search(l).group(1) in moved)]
        kept, i = [], 0
        while i < len(body):
            if body[i].startswith("**") and i > 0:
                j = i + 1
                while j < len(body) and not body[j].startswith("**"):
                    j += 1
                if not any(l.strip() for l in body[i + 1:j]) and j < len(body):
                    i = j
                    continue
            kept.append(body[i])
            i += 1
        lines = lines[:start] + kept + lines[end:]

    # Catalog: collect the moved rows per milestone and drop a milestone left without a row.
    rows = {}
    start, end = section_bounds(lines, "## Catalog")
    catalog = []
    for heading, body in read_blocks(lines[start:end]):
        remaining = []
        for line in body:
            m = archived_link.search(line) if line.startswith("| [") else None
            if m and m.group(1) in moved:
                rows.setdefault(milestone_key(heading), (heading, []))[1].append(
                    fix_status(repo, m.group(1), line.replace("](./archive/", "](./")))
            else:
                remaining.append(line)
        if heading is None:
            catalog += remaining
        elif any(l.startswith("| [") for l in remaining):
            catalog += [heading] + remaining
    lines = lines[:start] + catalog + lines[end:]

    seen = {re.search(r"\]\(\./([^)#]+)", r).group(1) for _, rs in rows.values() for r in rs}
    for name in sorted(set(moved) - seen):
        key = job_milestone(repo, name) or "Milestone ?"
        rows.setdefault(key, (f"### {key}", []))[1].append(new_row(repo, name))

    # The archive's catalog: append to the milestone's table, or add the milestone in order.
    if os.path.exists(archive_path):
        archive = open(archive_path, encoding="utf-8").read().rstrip("\n").split("\n")
    else:
        archive = ARCHIVE_HEADER.rstrip("\n").split("\n")
    for key, (heading, new_rows) in sorted(rows.items(), key=lambda kv: milestone_order(kv[0])):
        at = next((i for i, l in enumerate(archive)
                   if l.startswith("### ") and milestone_key(l) == key), None)
        if at is not None:
            last = at
            for i in range(at + 1, len(archive)):
                if archive[i].startswith("### "):
                    break
                if archive[i].startswith("|"):
                    last = i
            archive[last + 1:last + 1] = new_rows
            continue
        block = ["", heading, "", "| Job | Priority | Status |", "| --- | --- | --- |"] + new_rows
        before = next((i for i, l in enumerate(archive) if l.startswith("### ")
                       and milestone_order(milestone_key(l)) > milestone_order(key)), None)
        if before is None:
            archive += block
        else:
            while before > 0 and not archive[before - 1].strip():
                before -= 1
            archive[before:before] = block
    open(archive_path, "w", encoding="utf-8").write("\n".join(archive) + "\n")

    lines = recount(lines, archive)
    open(index_path, "w", encoding="utf-8").write("\n".join(lines))
    return sum(len(rs) for _, rs in rows.values())


def fix_status(repo, name, row):
    """The job file is the status truth: a row whose word disagrees takes the file's word."""
    line = status_line(open(os.path.join(repo, ARCHIVE, name), encoding="utf-8").read())
    if line is None:
        return row
    cells = row.split(" | ")
    if not cells[-1].startswith(status_word(line)):
        cells[-1] = status_word(line) + " |"
    return " | ".join(cells)


def count_rows(lines):
    counts, key = {}, None
    for line in lines:
        if line.startswith("### "):
            key = milestone_key(line)
            counts.setdefault(key, 0)
        elif line.startswith("| [") and key:
            counts[key] += 1
    return counts


def recount(lines, archive):
    """The Job Inventory table: open rows here, archived rows there, and their sum. Rows keep
    their order and their area text; a milestone the table does not name yet is added last."""
    if "## Job Inventory" not in lines:
        return lines
    start, end = section_bounds(lines, "## Job Inventory")
    cat_start, cat_end = section_bounds(lines, "## Catalog")
    open_rows, archived_rows = count_rows(lines[cat_start:cat_end]), count_rows(archive)
    table = [i for i in range(start, end) if lines[i].startswith("| ")]
    if len(table) < 3 or "Open" not in lines[table[0]]:
        return lines
    areas = [lines[i].strip("|").split("|")[0].strip() for i in table[2:]]
    areas = [a for a in areas if a != "**Total**"]
    known = {milestone_key(a) for a in areas}
    areas += [k for k in sorted(set(open_rows) | set(archived_rows), key=milestone_order)
              if k not in known]
    rows = []
    for area in areas:
        o, a = open_rows.get(milestone_key(area), 0), archived_rows.get(milestone_key(area), 0)
        rows.append(f"| {area} | {o} | {a} | {o + a} |")
    o, a = sum(open_rows.values()), sum(archived_rows.values())
    rows.append(f"| **Total** | **{o}** | **{a}** | **{o + a}** |")
    return lines[:table[2]] + rows + lines[table[-1] + 1:]


# ---------------------------------------------------------------------------------------------

def main(argv):
    args = argv[1:]
    check = "--check" in args
    release = args[args.index("--release") + 1] if "--release" in args else None
    repo = next((a for a in args if not a.startswith("--") and a != release), None)
    if repo is None:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    if not os.path.isdir(os.path.join(repo, JOBS)):
        # The public export leaves docs/ out (RD-130-23); check.sh runs there too.
        print(f"no {JOBS}/ in this tree; nothing to archive")
        return 0
    if not os.path.isfile(os.path.join(repo, JOBS, "README.md")):
        print(f"no {JOBS}/README.md under {repo}", file=sys.stderr)
        return 2

    due = due_files(repo, release)
    misplaced = misplaced_files(repo)
    for name, word in misplaced:
        print(f"open again: {ARCHIVE}/{name} ({word}) — move it and its row back to {JOBS}/ by hand",
              file=sys.stdout if check else sys.stderr)
    if check:
        for name, reason in due:
            print(f"due: {JOBS}/{name} ({reason}) — run scripts/archive-jobs.sh")
        print(f"{len(due)} job file(s) due for {ARCHIVE}/" if due else "nothing to archive")
        return 1 if due or misplaced else 0
    if not due:
        print("nothing to archive")
        return 0

    dirty = git(repo, "status", "--porcelain", "--", JOBS)
    if dirty.strip():
        print(f"refusing: uncommitted changes under {JOBS}/ — commit or set them aside first",
              file=sys.stderr)
        print(dirty.rstrip("\n"), file=sys.stderr)
        return 2

    os.makedirs(os.path.join(repo, ARCHIVE), exist_ok=True)
    moved = [name for name, _ in due]
    for name, reason in due:
        git(repo, "mv", f"{JOBS}/{name}", f"{ARCHIVE}/{name}")
        print(f"moved: {JOBS}/{name} -> {ARCHIVE}/ ({reason})")
    counts = rewrite_references(repo, set(moved))
    rows = update_indexes(repo, set(moved))
    print(f"{len(moved)} moved; {counts['links']} link(s) and {counts['mentions']} path mention(s) "
          f"rewritten in {counts['files']} file(s); {rows} catalog row(s) moved to {ARCHIVE}/README.md")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
