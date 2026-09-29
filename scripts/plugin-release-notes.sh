#!/usr/bin/env bash
#
# The release notes of one plugin version, read from CHANGELOG.md (RD-160-09).
#
#   scripts/plugin-release-notes.sh <plugin> <version> [changelog]
#
# An entry that changes a plugin ends with the plugin's directory name in backticks and the
# version it raises it to — "… `realdebrid` 0.2.2.", or "`premiumize-transfers` and
# `torbox-jobs` 0.2.2." for several at once — the convention the CHANGELOG follows since 1.5.
# Every entry that names `<plugin>` <version> that way is printed as plain text, one line each:
# bold, backticks and link targets dropped, wrapped lines joined. Nothing is printed for a
# version no entry names; the plugin index then carries no notes for it, as before.
#
# The output stays within the index's limit of 2000 characters (MAX_RELEASE_NOTES_CHARS in
# crates/rd-plugin-host/src/index.rs): whole entries while they fit, and a first entry longer
# than that cut with an ellipsis. The release workflow writes it to
# `dist/plugin-notes/<plugin>-<version>.txt`, which `rd-pack plugin index build --notes` reads.
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "usage: scripts/plugin-release-notes.sh <plugin> <version> [changelog]" >&2
    exit 2
fi
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG="${3:-$ROOT/CHANGELOG.md}"
if [[ ! -f "$CHANGELOG" ]]; then
    echo "plugin-release-notes: no changelog at $CHANGELOG" >&2
    exit 2
fi

python3 - "$1" "$2" "$CHANGELOG" <<'PY'
import re
import sys

plugin, version, path = sys.argv[1:4]
LIMIT = 2000

entries = []
current = None
with open(path, encoding="utf-8") as changelog:
    for line in changelog:
        line = line.rstrip("\n")
        if line.startswith("- "):
            if current is not None:
                entries.append(current)
            current = [line[2:]]
        elif current is not None and line.startswith(" ") and line.strip():
            current.append(line.strip())
        else:
            if current is not None:
                entries.append(current)
            current = None
if current is not None:
    entries.append(current)

# One name or a list of them ("`a`, `b` and `c`"), then the version, which ends where it ends:
# 0.2.2 is neither 0.2.21 nor 0.2.2-rc.1.
named = re.compile(
    r"((?:`[a-z0-9-]+`(?:,?\s+and\s+|,\s+))*`[a-z0-9-]+`)\s+"
    r"([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?)"
    r"(?![0-9A-Za-z+-]|\.[0-9A-Za-z])"
)


def names_version(text):
    for match in named.finditer(text):
        if match.group(2) == version and plugin in re.findall(r"`([a-z0-9-]+)`", match.group(1)):
            return True
    return False


def plain(text):
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.replace("**", "").replace("`", "")
    return re.sub(r"\s+", " ", text).strip()


notes = []
for entry in entries:
    text = " ".join(entry)
    if names_version(text):
        notes.append(plain(text))

lines = []
used = 0
for note in notes:
    line = note if len(notes) == 1 else "- " + note
    extra = len(line) + (1 if lines else 0)
    if used + extra > LIMIT:
        if not lines:
            lines.append(line[: LIMIT - 1].rstrip() + "…")
        break
    lines.append(line)
    used += extra
if lines:
    print("\n".join(lines))
PY
