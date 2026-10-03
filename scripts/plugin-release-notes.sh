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
#
#   scripts/plugin-release-notes.sh --missing <ref> [plugin...]
#
# Before a release: names every plugin whose manifest version differs from the one at <ref> (the
# last release tag) and that no entry names yet, by default over the plugins the bundle ships
# (`build-plugins.sh --list-packageable`). Each line is one version with its plugins in the
# changelog's notation — "`a`, `b` and `c` 0.1.8" — ready to end an entry with; exit 1 while any
# is left, 0 when none. v1.6.0 shipped two raised plugins without notes this way (RD-160-09).
set -euo pipefail

usage() {
    echo "usage: scripts/plugin-release-notes.sh <plugin> <version> [changelog]" >&2
    echo "       scripts/plugin-release-notes.sh --missing <ref> [plugin...]" >&2
    exit 2
}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

manifest_version() { awk -F'"' '/^version = "/ { print $2; exit }'; }

if [[ "${1:-}" == "--missing" ]]; then
    [[ $# -ge 2 ]] || usage
    ref="$2"
    shift 2
    if ! git -C "$ROOT" rev-parse --verify -q "$ref^{commit}" > /dev/null; then
        echo "plugin-release-notes: no commit $ref" >&2
        exit 2
    fi
    plugins=("$@")
    [[ ${#plugins[@]} -gt 0 ]] || mapfile -t plugins < <("$ROOT/scripts/build-plugins.sh" --list-packageable)
    missing=()
    for plugin in "${plugins[@]}"; do
        version="$(manifest_version < "$ROOT/plugins/$plugin/manifest.toml")"
        # A plugin new since <ref> has no version there and counts as raised.
        before="$(git -C "$ROOT" show "$ref:plugins/$plugin/manifest.toml" 2> /dev/null | manifest_version || true)"
        [[ "$version" != "$before" ]] || continue
        [[ -z "$("$0" "$plugin" "$version" "$ROOT/CHANGELOG.md")" ]] || continue
        missing+=("$version $plugin")
    done
    if [[ ${#missing[@]} -eq 0 ]]; then
        echo "plugin-release-notes: every plugin version raised since $ref has its entry" >&2
        exit 0
    fi
    echo "plugin-release-notes: ${#missing[@]} plugin versions raised since $ref have no entry; end one with:" >&2
    printf '%s\n' "${missing[@]}" | sort -k1,1V -k2,2 | awk '
        function flush() {
            if (n == 0) return
            line = "`" names[1] "`"
            for (i = 2; i < n; i++) line = line ", `" names[i] "`"
            if (n > 1) line = line " and `" names[n] "`"
            print line " " current
            n = 0
        }
        $1 != current { flush(); current = $1 }
        { names[++n] = $2 }
        END { flush() }'
    exit 1
fi

[[ $# -ge 2 && $# -le 3 ]] || usage
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
