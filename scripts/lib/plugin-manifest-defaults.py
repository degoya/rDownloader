"""Removes from bundled plugin manifests what they need not state (RD-1280-18).

Two kinds of line go: a shared field whose value is the one `plugins/metadata.toml` holds
(`rd-pack plugin package --defaults` writes it back into the packaged copy), and a `[limits]`
field whose value is the host's default (`DEFAULT_*` in crates/rd-plugin-host/src/lib.rs,
`#[serde(default)]` on every field of `PluginLimits`). Everything else stays as written,
comments included, so a rerun changes nothing and a manifest merged in from another branch is
converted by running this again.

A comment directly above a removed line goes with it when nothing of its table follows -- it
explained that line -- and stays when a field of the same table follows, which it then
introduces. A `[limits]` table left without a field goes, with the blank line before it.

Usage: plugin-manifest-defaults.py ROOT [--check] [plugin ...]
"""

import re
import sys
import tomllib
from pathlib import Path

HEADER = re.compile(r"^\[\[?([A-Za-z0-9_.\-]+)\]\]?\s*(#.*)?$")
KEY = re.compile(r"^([A-Za-z0-9_\-]+)\s*=\s*(.*)$")
DEFAULT_CONST = re.compile(r"^pub const DEFAULT_([A-Z_]+): u64 = ([0-9_ *]+);$", re.M)
LIMIT_FIELDS = {
    "MEMORY_BYTES": "memory_bytes",
    "FUEL": "fuel",
    "TIMEOUT_MILLISECONDS": "timeout_milliseconds",
    "MAX_RESPONSE_BYTES": "max_response_bytes",
    "WAIT_BUDGET_MILLISECONDS": "wait_budget_milliseconds",
}


def limit_defaults(root: Path) -> dict:
    """The five `DEFAULT_*` limits, evaluated from their constant expressions."""
    source = (root / "crates/rd-plugin-host/src/lib.rs").read_text()
    found = {}
    for name, expression in DEFAULT_CONST.findall(source):
        if name in LIMIT_FIELDS:
            value = 1
            for factor in expression.replace("_", "").split("*"):
                value *= int(factor.strip())
            found[LIMIT_FIELDS[name]] = value
    missing = sorted(set(LIMIT_FIELDS.values()) - set(found))
    if missing:
        sys.exit(f"crates/rd-plugin-host/src/lib.rs declares no default for {', '.join(missing)}")
    return found


def removable(root: Path) -> dict:
    """Per table, the field values a manifest need not state."""
    shared = tomllib.loads((root / "plugins/metadata.toml").read_text())
    metadata = shared.pop("metadata", {})
    return {"": shared, "metadata": metadata, "limits": limit_defaults(root)}


def value_of(text: str):
    """The TOML value on one line, or a marker for one that spans several."""
    try:
        return tomllib.loads(f"v = {text}")["v"]
    except tomllib.TOMLDecodeError:
        return KeyError


def convert(text: str, tables: dict) -> str:
    lines = text.split("\n")
    trailing_newline = lines and lines[-1] == ""
    if trailing_newline:
        lines.pop()
    sections, keys = [], []
    section = ""
    for line in lines:
        header = HEADER.match(line)
        if header:
            section = header.group(1)
            sections.append(section)
            keys.append(None)
            continue
        sections.append(section)
        key = KEY.match(line)
        keys.append(key.group(1) if key else None)

    removed = set()
    for index, line in enumerate(lines):
        key = keys[index]
        table = tables.get(sections[index])
        if key is None or table is None or key not in table or HEADER.match(line):
            continue
        if value_of(KEY.match(line).group(2)) == table[key]:
            removed.add(index)

    for index in sorted(removed):
        following = next((j for j in range(index + 1, len(lines)) if j not in removed), None)
        introduces = (
            following is not None
            and keys[following] is not None
            and sections[following] == sections[index]
            and not HEADER.match(lines[following])
        )
        if introduces:
            continue
        above = index - 1
        while above >= 0 and lines[above].lstrip().startswith("#") and above not in removed:
            removed.add(above)
            above -= 1

    # A [limits] table with no field left goes, with the blank line in front of it.
    for index, line in enumerate(lines):
        header = HEADER.match(line)
        if not header or header.group(1) != "limits" or index in removed:
            continue
        body = [j for j in range(index + 1, len(lines)) if sections[j] == "limits"
                and not HEADER.match(lines[j])]
        if all(j in removed or lines[j].strip() == "" for j in body):
            removed.update([index, *body])

    kept = [line for index, line in enumerate(lines) if index not in removed]
    tidy = []
    for line in kept:
        if line.strip() == "" and (not tidy or tidy[-1].strip() == ""):
            continue
        tidy.append(line)
    while tidy and tidy[-1].strip() == "":
        tidy.pop()
    return "\n".join(tidy) + "\n"


def main() -> int:
    arguments = sys.argv[1:]
    root = Path(arguments.pop(0))
    check = "--check" in arguments
    names = [argument for argument in arguments if argument != "--check"]
    tables = removable(root)
    if names:
        manifests = [root / "plugins" / name / "manifest.toml" for name in names]
    else:
        manifests = sorted((root / "plugins").glob("*/manifest.toml"))
    changed = []
    for manifest in manifests:
        if not manifest.is_file():
            sys.exit(f"no plugin manifest at {manifest}")
        text = manifest.read_text()
        converted = convert(text, tables)
        tomllib.loads(converted)
        if converted != text:
            changed.append(manifest.parent.name)
            if not check:
                manifest.write_text(converted)
    for name in changed:
        print(f"{'states a default' if check else 'converted'}: plugins/{name}/manifest.toml")
    return 1 if check and changed else 0


if __name__ == "__main__":
    sys.exit(main())
