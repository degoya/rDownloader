#!/usr/bin/env bash
#
# The release notes of one plugin version, read from the plugin's CHANGES.md (RD-1140-03).
#
#   scripts/plugin-release-notes.sh <plugin> <version> [changes-file]
#
# Every plugin keeps `plugins/<plugin>/CHANGES.md`: a `## <version>` section per version, newest
# first, under it one to three English sentences of what a user notices since the previous
# version — or, when nothing does, the fixed sentence "Maintenance release: internal changes only,
# no change in behaviour." The section of <version> is printed as one line of plain text: bold and
# link targets dropped, wrapped lines joined. Nothing is printed for a version without a section,
# or a plugin without the file; the plugin index then carries no notes for it.
#
# Until 1.13 the notes were every CHANGELOG.md entry naming the version (RD-160-09); an entry that
# raised every plugin at once became 2000 characters of developer prose on each plugin's card.
# CHANGELOG.md stays the developers' record. CHANGES.md is neither a source the component is built
# from (plugin-stamp.sh hashes `*.rs`, `*.wit`, `Cargo.toml` and `manifest.toml` only) nor a member
# of the signed package, so writing a section raises no version by itself.
#
# The output stays within the index's limit of 2000 characters (MAX_RELEASE_NOTES_CHARS in
# crates/rd-plugin-host/src/index.rs), cut with an ellipsis beyond it — --check keeps a section
# far below. The release workflow writes it to `dist/plugin-notes/<plugin>-<version>.txt`
# (scripts/ci-plugins.sh), which `rd-pack plugin index build --notes` reads.
#
#   scripts/plugin-release-notes.sh --check [plugin...]
#
# The rules, over the plugins the bundle ships (`build-plugins.sh --list-packageable`) and the
# examples beside them (`--list-examples`; never bundled, but the reference an author copies,
# RD-1190-10), or the ones named: every plugin has its CHANGES.md and a section for the version its manifest declares, so
# a raised version without notes fails here; and every section of the file is at most 300
# characters, ends a sentence, is English (no umlauts), and names no job (`RD-…`, `PL-…`), no
# path, no Rust identifier (code spans, `::`, snake_case) and no list of other plugins (two or
# more of their directory names). Every finding is printed; exit 1 while any is left. Run by
# check.sh (its file checks, so the preflight too) and the release chain's docs-gate.
set -euo pipefail

usage() {
    echo "usage: scripts/plugin-release-notes.sh <plugin> <version> [changes-file]" >&2
    echo "       scripts/plugin-release-notes.sh --check [plugin...]" >&2
    exit 2
}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NOTES_PY="$(cat <<'PY'
import re
import sys

LIMIT = 2000
SHORT = 300
MAINTENANCE = "Maintenance release: internal changes only, no change in behaviour."
HEADING = re.compile(r"^##\s+(\S+)\s*$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?$")


def sections(path):
    """[(version, plain text, line number)] of a CHANGES.md, in file order."""
    found = []
    current = None
    with open(path, encoding="utf-8") as changes:
        for number, line in enumerate(changes, 1):
            heading = HEADING.match(line)
            if heading:
                current = [heading.group(1), [], number]
                found.append(current)
            elif line.startswith("#"):
                current = None
            elif current is not None and line.strip():
                current[1].append(line.strip())
    return [(version, plain(" ".join(body)), number) for version, body, number in found]


def plain(text):
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.replace("**", "").replace("__", "")
    return re.sub(r"\s+", " ", text).strip()


def problems(text, others):
    if not text:
        yield "has no text"
        return
    if len(text) > SHORT:
        yield f"is {len(text)} characters, at most {SHORT}"
    if not re.search(r"[.!?]$", text):
        yield "does not end a sentence"
    if text.startswith("Maintenance release") and text != MAINTENANCE:
        yield f"says maintenance in other words than \"{MAINTENANCE}\""
    if re.search(r"[äöüÄÖÜß]", text):
        yield "is not English"
    if re.search(r"\b[A-Z]{2,5}-[0-9]+\b", text):
        yield "names a job"
    if re.search(r"(?:^|[\s(])[\w.~-]*/[\w./-]*\.(?:rs|toml|md|sh|json|wit|ts|vue|ya?ml|py|txt)\b", text) \
            or re.search(r"\b(?:crates|plugins|scripts|docs|sdk|src|web)/", text):
        yield "names a path"
    if "`" in text or "::" in text or re.search(r"\b[a-z][a-z0-9]*_[a-z0-9_]+\b", text):
        yield "names a Rust identifier"
    named = sorted(name for name in others if re.search(rf"(?<![\w-]){re.escape(name)}(?![\w-])", text))
    if len(named) >= 2:
        yield "lists other plugins (" + ", ".join(named) + ")"


def print_notes(version, path):
    for found, text, _ in sections(path):
        if found == version and text:
            print(text if len(text) <= LIMIT else text[: LIMIT - 1].rstrip() + "…")
            return


def check(root, names, packageable):
    findings = []
    for name in names:
        path = f"{root}/plugins/{name}/CHANGES.md"
        try:
            with open(f"{root}/plugins/{name}/manifest.toml", encoding="utf-8") as manifest:
                declared = re.search(r'^version = "(.*)"', manifest.read(), re.M)
        except FileNotFoundError:
            findings.append(f"plugins/{name}: no such plugin")
            continue
        declared = declared.group(1) if declared else "?"
        try:
            found = sections(path)
        except FileNotFoundError:
            findings.append(f"plugins/{name}/CHANGES.md: missing; start it with \"## {declared}\"")
            continue
        if declared not in (version for version, _, _ in found):
            findings.append(f"plugins/{name}/CHANGES.md: no section \"## {declared}\" for the version the manifest declares")
        others = [other for other in packageable if other != name]
        seen = set()
        for version, text, number in found:
            where = f"plugins/{name}/CHANGES.md:{number}: {version}"
            if not VERSION.match(version):
                findings.append(f"{where} is not a version")
            if version in seen:
                findings.append(f"{where} has a second section")
            seen.add(version)
            findings.extend(f"{where} {problem}" for problem in problems(text, others))
    for finding in findings:
        print(finding, file=sys.stderr)
    return 1 if findings else 0


if sys.argv[1] == "--check":
    root = sys.argv[2]
    packageable = sys.argv[3].split()
    examples = sys.argv[4].split()
    sys.exit(check(root, sys.argv[5:] or packageable + examples, packageable))
print_notes(sys.argv[2], sys.argv[3])
PY
)"

if [[ "${1:-}" == "--check" ]]; then
    shift
    packageable="$("$ROOT/scripts/build-plugins.sh" --list-packageable | tr '\n' ' ')"
    examples="$("$ROOT/scripts/build-plugins.sh" --list-examples | tr '\n' ' ')"
    if python3 -c "$NOTES_PY" --check "$ROOT" "$packageable" "$examples" "$@"; then
        echo "plugin-release-notes: every plugin version has its notes, short and for users" >&2
        exit 0
    fi
    echo "plugin-release-notes: write the section in the plugin's CHANGES.md: one to three English sentences of what a user notices, or \"Maintenance release: internal changes only, no change in behaviour.\"" >&2
    exit 1
fi

[[ $# -ge 2 && $# -le 3 ]] || usage
CHANGES="${3:-$ROOT/plugins/$1/CHANGES.md}"
[[ -f "$CHANGES" ]] || exit 0
python3 -c "$NOTES_PY" "$1" "$2" "$CHANGES"
