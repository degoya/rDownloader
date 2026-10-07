#!/usr/bin/env bash
#
# The application's release notes for users, read from RELEASE-NOTES.md (RD-1150-02).
#
#   scripts/release-notes.sh <version> [file]
#
# RELEASE-NOTES.md keeps a `## X.Y.Z` section per version, newest first: up to eight `- ` points
# in English of what a user notices — what is new, what works better, what is fixed — or, when
# nothing does, the one sentence "Maintenance release: internal changes only, no change in
# behaviour." Prints the version's points as `- point` lines of plain text (bold and link targets
# dropped, wrapped lines joined), or the sentence; a pre-release falls back to its release's
# section; nothing for a version without one. The release workflow puts them at the top of the
# GitHub release (scripts/release-publish.sh app-release); `rd-pack update manifest build
# --release-notes` reads the same section into the signed update manifest, under the same rules.
#
# Until 1.14 the update dialog showed the CHANGELOG section's headlines: job numbers, crate
# visibility, and a headline over two lines as an open "**A file waiting …". CHANGELOG.md stays
# the developers' record; the dialog and the release link its section at the tag as the full
# changes.
#
#   scripts/release-notes.sh --anchor <version> [changelog]
#
# GitHub's anchor of the version's `## [X.Y.Z] - YYYY-MM-DD` heading in CHANGELOG.md (lower case,
# punctuation but `-` and `_` dropped, each space a hyphen: `1150---2026-10-10`); nothing without
# the heading. The same rule as rd-pack's, which writes it into the manifest.
#
#   scripts/release-notes.sh --check [--version <version>]
#
# The rules, over every section of RELEASE-NOTES.md: each heading a version, once; each section
# one to eight points, or the maintenance sentence alone; each point at most 200 characters,
# ending a sentence, English (no umlauts), and naming no job (`RD-…`, `CR-…`, `PL-…`), no path and
# no code identifier (code spans, `::`, snake_case). A section with an HTML comment saying `draft`
# is named as a draft and passes — the notes of the version in the making are written during its
# wave and finished at the release. With --version (the release chain's docs-gate) that version's
# section must exist and must not be a draft. Every finding is printed; exit 1 while any is left.
# Run by check.sh's file checks (so the preflight too) and the docs-gate.
set -euo pipefail

usage() {
    echo "usage: scripts/release-notes.sh <version> [file]" >&2
    echo "       scripts/release-notes.sh --anchor <version> [changelog]" >&2
    echo "       scripts/release-notes.sh --check [--version <version>]" >&2
    exit 2
}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NOTES_PY="$(cat <<'PY'
import re
import sys

MAX_POINTS = 8
MAX_POINT_CHARS = 200
MAINTENANCE = "Maintenance release: internal changes only, no change in behaviour."
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?$")
DIRECTORIES = ("crates/", "plugins/", "scripts/", "docs/", "sdk/", "src/", "web/")
EXTENSIONS = ("rs", "toml", "md", "sh", "json", "wit", "ts", "vue", "yml", "yaml", "py", "txt", "sql")


def sections(path):
    """[(version, body lines, line number)] of RELEASE-NOTES.md, in file order."""
    found = []
    current = None
    with open(path, encoding="utf-8") as notes:
        for number, line in enumerate(notes, 1):
            line = line.rstrip("\n")
            if line.startswith("## "):
                current = [line[3:].strip(), [], number]
                found.append(current)
            elif line.startswith("# "):
                current = None
            elif current is not None:
                current[1].append(line)
    return found


def plain(text):
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.replace("**", "").replace("__", "")
    return re.sub(r"\s+", " ", text).strip()


def parse(lines):
    """(points, paragraph or None, draft): rd-pack's reading of a section."""
    points, paragraph, draft, in_comment = [], [], False, False
    for line in lines:
        if in_comment or line.lstrip().startswith("<!--"):
            draft = draft or "draft" in line.lower()
            in_comment = "-->" not in line
            continue
        if not line.strip():
            continue
        if line.startswith("- ") or line.startswith("* "):
            points.append(line[2:].strip())
        elif line.startswith(" ") and points:
            points[-1] += " " + line.strip()
        else:
            paragraph.append(line.strip())
    return [plain(point) for point in points], plain(" ".join(paragraph)) if paragraph else None, draft


def is_path(word):
    word = word.strip('(),;:"')
    word = word[:-1] if word.endswith(".") else word
    if any(word.startswith(directory) or "/" + directory in word for directory in DIRECTORIES):
        return True
    return "/" in word and "." in word and word.rsplit(".", 1)[1] in EXTENSIONS


def is_snake_case(word):
    word = re.sub(r"^[^A-Za-z0-9_]+|[^A-Za-z0-9_]+$", "", word)
    return bool(re.fullmatch(r"[a-z][a-z0-9_]*_[a-z0-9_]*[a-z0-9]", word))


def point_problems(point):
    if len(point) > MAX_POINT_CHARS:
        yield f"is {len(point)} characters, at most {MAX_POINT_CHARS}"
    if not re.search(r"[.!?]$", point):
        yield "does not end a sentence"
    if re.search(r"[äöüÄÖÜß]", point):
        yield "is not English"
    if re.search(r"(?<![A-Za-z0-9])[A-Z]{2,5}-[0-9]", point):
        yield "names a job"
    if any(is_path(word) for word in point.split()):
        yield "names a path"
    if "`" in point or "::" in point or any(is_snake_case(word) for word in point.split()):
        yield "names a code identifier"


def problems(points, paragraph):
    if paragraph is None and not points:
        yield "has no points"
    elif paragraph is not None and not points:
        if paragraph != MAINTENANCE:
            yield f"is not a list of points; a version without visible change says \"{MAINTENANCE}\""
    elif paragraph is not None:
        yield "mixes points with other text"
    elif len(points) > MAX_POINTS:
        yield f"has {len(points)} points, at most {MAX_POINTS}"
    for index, point in enumerate(points, 1):
        for problem in point_problems(point):
            yield f"point {index} {problem}"


def candidates(version):
    base = version.split("-", 1)[0]
    return [version] if base == version else [version, base]


def print_notes(version, path):
    found = {name: lines for name, lines, _ in sections(path)}
    for candidate in candidates(version):
        if candidate in found:
            points, paragraph, _ = parse(found[candidate])
            text = paragraph if not points else "\n".join(f"- {point}" for point in points)
            if text:
                print(text)
            return


def anchor(version, path):
    with open(path, encoding="utf-8") as changelog:
        lines = changelog.read().splitlines()
    for candidate in candidates(version):
        for line in lines:
            if line.startswith(f"## [{candidate}]"):
                heading = line[3:].strip().lower()
                print("".join("-" if c == " " else c for c in heading if c.isalnum() or c in "-_ "))
                return


def check(path, version):
    findings = []
    try:
        found = sections(path)
    except FileNotFoundError:
        print(f"{path}: missing; start it with a \"## X.Y.Z\" section", file=sys.stderr)
        return 1
    seen = set()
    drafts = []
    for name, lines, number in found:
        where = f"RELEASE-NOTES.md:{number}: {name}"
        if not VERSION.match(name):
            findings.append(f"{where} is not a version")
        if name in seen:
            findings.append(f"{where} has a second section")
        seen.add(name)
        points, paragraph, draft = parse(lines)
        if draft:
            drafts.append(name)
        findings.extend(f"{where} {problem}" for problem in problems(points, paragraph))
    if version:
        names = [candidate for candidate in candidates(version) if candidate in seen]
        if not names:
            findings.append(f"RELEASE-NOTES.md: no section \"## {version}\" for the release")
        elif names[0] in drafts:
            findings.append(f"RELEASE-NOTES.md: the section {names[0]} is still a draft; finish it and remove the draft comment")
    for name in drafts:
        if not version or name not in candidates(version):
            print(f"RELEASE-NOTES.md: {name} is a draft (finished at its release)", file=sys.stderr)
    for finding in findings:
        print(finding, file=sys.stderr)
    return 1 if findings else 0


mode = sys.argv[1]
if mode == "--check":
    sys.exit(check(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else ""))
if mode == "--anchor":
    anchor(sys.argv[2], sys.argv[3])
else:
    print_notes(sys.argv[2], sys.argv[3])
PY
)"

case "${1:-}" in
    --check)
        shift
        version=""
        if [[ $# -gt 0 ]]; then
            [[ "$1" == "--version" && $# -eq 2 ]] || usage
            version="${2#v}"
        fi
        if python3 -c "$NOTES_PY" --check "$ROOT/RELEASE-NOTES.md" "$version"; then
            echo "release-notes: every section of RELEASE-NOTES.md is short and for users" >&2
            exit 0
        fi
        echo "release-notes: write RELEASE-NOTES.md for users: up to eight short English points of what is new, better or fixed, without job numbers, paths or code; or \"Maintenance release: internal changes only, no change in behaviour.\"" >&2
        exit 1
        ;;
    --anchor)
        shift
        [[ $# -ge 1 && $# -le 2 ]] || usage
        changelog="${2:-$ROOT/CHANGELOG.md}"
        [[ -f "$changelog" ]] || exit 0
        python3 -c "$NOTES_PY" --anchor "${1#v}" "$changelog"
        ;;
    -*|"") usage ;;
    *)
        [[ $# -le 2 ]] || usage
        notes="${2:-$ROOT/RELEASE-NOTES.md}"
        [[ -f "$notes" ]] || exit 0
        python3 -c "$NOTES_PY" --print "${1#v}" "$notes"
        ;;
esac
