#!/usr/bin/env python3
"""The release documents' mechanics (RD-1270-15), run by scripts/release-docs.sh.

    release-docs.py cut <repo> <version> <date> <previous tag or "">
    release-docs.py plugins <repo> <previous tag or "">
    release-docs.py next-draft <repo> <version>

cut: CHANGELOG.md's `## [Unreleased]` becomes `## [<version>] - <date>` under a new, empty
[Unreleased]; when plugin versions changed since the previous tag, the section's `### Changed`
(made before `### Deprecated`/`Removed`/`Fixed`/`Security` when there is none) ends with one point
naming them, wrapped at 100 columns. RELEASE-NOTES.md's section of the version (a beta's falls
back to its release's) loses its `draft` comment. Refused, with nothing written: a version cut
already, no [Unreleased] heading, or nothing under it. Named, not refused: a notes section that is
missing or has no points yet, and beta sections of the version left to consolidate by hand.

plugins: the sentence alone — "The versions this release raises: `a` and `b` 0.1.13; `c` 0.2.1."
— every `plugins/<dir>/manifest.toml` whose `version` differs from the previous tag's (a new
plugin included), grouped by version in ascending order, names (directory names) sorted. Nothing
without a previous tag or without a change.

next-draft: RELEASE-NOTES.md gets the next minor version's section (1.26.0 and 1.26.1 → 1.27.0)
on top, holding only the draft comment, unless it has that section already; a beta adds none,
its stable release does.
"""

import re
import subprocess
import sys
import textwrap
import tomllib
from pathlib import Path

DRAFT = "<!-- draft: the notes of the version in the making -->"
LATER_KINDS = ("### Deprecated", "### Removed", "### Fixed", "### Security")
WIDTH = 100


def version_key(version):
    return [(0, int(part), "") if part.isdigit() else (1, 0, part) for part in re.split(r"[.-]", version)]


def manifest_version(text):
    version = tomllib.loads(text).get("version")
    return str(version) if version is not None else None


def raised_plugins(root, previous):
    """{plugin directory: version} of every manifest whose version moved since `previous`."""
    raised = {}
    if not previous:
        return raised
    for manifest in sorted(Path(root, "plugins").glob("*/manifest.toml")):
        name = manifest.parent.name
        now = manifest_version(manifest.read_text(encoding="utf-8"))
        shown = subprocess.run(["git", "-C", root, "show", f"{previous}:plugins/{name}/manifest.toml"],
                               capture_output=True, text=True, check=False)
        then = manifest_version(shown.stdout) if shown.returncode == 0 else None
        if now is not None and now != then:
            raised[name] = now
    return raised


def joined(names):
    quoted = [f"`{name}`" for name in names]
    return quoted[0] if len(quoted) == 1 else ", ".join(quoted[:-1]) + " and " + quoted[-1]


def plugin_sentence(raised):
    if not raised:
        return ""
    groups = {}
    for name, version in raised.items():
        groups.setdefault(version, []).append(name)
    parts = [f"{joined(sorted(groups[version]))} {version}" for version in sorted(groups, key=version_key)]
    return "The versions this release raises: " + "; ".join(parts) + "."


def bullet(sentence):
    return textwrap.wrap(f"**Plugin versions.** {sentence}", width=WIDTH, initial_indent="- ",
                         subsequent_indent="  ", break_long_words=False, break_on_hyphens=False)


def with_changed_point(body, point):
    """The [Unreleased] body with `point` as the last entry of its `### Changed`."""
    start = next((i for i, line in enumerate(body) if line.strip() == "### Changed"), None)
    if start is None:
        at = next((i for i, line in enumerate(body) if line.strip() in LATER_KINDS), len(body))
        before = [""] if at > 0 and body[at - 1].strip() else []
        after = [""] if at < len(body) else []
        return body[:at] + before + ["### Changed", ""] + point + after + body[at:]
    end = next((i for i in range(start + 1, len(body)) if body[i].startswith("### ")), len(body))
    while end > start + 1 and not body[end - 1].strip():
        end -= 1
    return body[:end] + [""] + point + body[end:]


def candidates(version):
    base = version.split("-", 1)[0]
    return [version] if base == version else [version, base]


def notes_sections(lines):
    """{version: (heading index, end index)} of RELEASE-NOTES.md."""
    found, starts = {}, [i for i, line in enumerate(lines) if line.startswith("## ")]
    for n, start in enumerate(starts):
        end = starts[n + 1] if n + 1 < len(starts) else len(lines)
        found.setdefault(lines[start][3:].strip(), (start, end))
    return found


def without_draft(lines, start, end):
    """The lines with the `draft` comments of lines[start:end] removed, and how many were."""
    kept, removed, i = lines[:start], 0, start
    while i < end:
        if lines[i].lstrip().startswith("<!--"):
            j = i
            while j < end - 1 and "-->" not in lines[j]:
                j += 1
            if any("draft" in line.lower() for line in lines[i:j + 1]):
                removed += 1
                i = j + 1
                if i < end and not lines[i].strip() and kept and not kept[-1].strip():
                    i += 1
                continue
        kept.append(lines[i])
        i += 1
    return kept + lines[end:], removed


def has_points(lines):
    return any(line.startswith(("- ", "* ")) for line in lines)


def write(path, lines):
    Path(path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def cut(root, version, date, previous):
    changelog_path, notes_path = Path(root, "CHANGELOG.md"), Path(root, "RELEASE-NOTES.md")
    lines = changelog_path.read_text(encoding="utf-8").splitlines()
    if any(line.startswith(f"## [{version}]") for line in lines):
        print(f"release-docs: CHANGELOG.md has a [{version}] section already; nothing written", file=sys.stderr)
        return 1
    top = next((i for i, line in enumerate(lines) if line.strip() == "## [Unreleased]"), None)
    if top is None:
        print("release-docs: CHANGELOG.md has no '## [Unreleased]' heading; nothing written", file=sys.stderr)
        return 1
    end = next((i for i in range(top + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    body = lines[top + 1:end]
    while body and not body[-1].strip():
        body.pop()
    if not any(line.strip() for line in body):
        print(f"release-docs: [Unreleased] is empty; nothing to release as {version}", file=sys.stderr)
        return 1
    sentence = plugin_sentence(raised_plugins(root, previous))
    if sentence:
        body = with_changed_point(body, bullet(sentence))
    while body and not body[0].strip():
        body.pop(0)
    rest = lines[end:]
    write(changelog_path, lines[:top + 1] + ["", f"## [{version}] - {date}", ""] + body + ([""] if rest else []) + rest)
    print(f"CHANGELOG.md: [Unreleased] is now [{version}] - {date}")
    print(f"CHANGELOG.md: {sentence}" if sentence else
          f"CHANGELOG.md: no plugin version changed since {previous or 'the beginning (no previous tag)'}")
    if "-" not in version:
        betas = [line[3:].split("]")[0] + "]" for line in lines if line.startswith(f"## [{version}-")]
        if betas:
            print(f"release-docs: consolidate {', '.join(betas)} into [{version}] by hand", file=sys.stderr)

    if not notes_path.exists():
        print("release-docs: RELEASE-NOTES.md is missing; the docs gate wants its section", file=sys.stderr)
        return 0
    notes = notes_path.read_text(encoding="utf-8").splitlines()
    sections = notes_sections(notes)
    name = next((candidate for candidate in candidates(version) if candidate in sections), None)
    if name is None:
        print(f"release-docs: RELEASE-NOTES.md has no section '## {version}'; write it before the chain", file=sys.stderr)
        return 0
    start, end = sections[name]
    notes, removed = without_draft(notes, start, end)
    if removed:
        write(notes_path, notes)
        print(f"RELEASE-NOTES.md: the draft comment of {name} removed")
    else:
        print(f"RELEASE-NOTES.md: {name} has no draft comment")
    start, end = notes_sections(notes)[name]
    if not has_points(notes[start + 1:end]):
        print(f"release-docs: RELEASE-NOTES.md: {name} has no points yet; write them before the chain", file=sys.stderr)
    return 0


def next_draft(root, version):
    if "-" in version:
        print(f"RELEASE-NOTES.md: no next draft for the pre-release {version}; its release adds it")
        return 0
    major, minor, _ = version.split(".")
    following = f"{major}.{int(minor) + 1}.0"
    path = Path(root, "RELEASE-NOTES.md")
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else ["# Release notes"]
    if following in notes_sections(lines):
        print(f"RELEASE-NOTES.md: {following} has its section already")
        return 0
    at = next((i for i, line in enumerate(lines) if line.startswith("## ")), None)
    section = [f"## {following}", "", DRAFT]
    if at is None:
        while lines and not lines[-1].strip():
            lines.pop()
        lines = lines + [""] + section
    else:
        lines = lines[:at] + section + [""] + lines[at:]
    write(path, lines)
    print(f"RELEASE-NOTES.md: {following} started as a draft")
    return 0


def main(argv):
    mode = argv[1]
    if mode == "cut":
        return cut(argv[2], argv[3], argv[4], argv[5])
    if mode == "plugins":
        sentence = plugin_sentence(raised_plugins(argv[2], argv[3]))
        if sentence:
            print(sentence)
        return 0
    if mode == "next-draft":
        return next_draft(argv[2], argv[3])
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
