#!/usr/bin/env python3
"""French typography for the web catalogues (RD-1480-15, design.md).

    python3 scripts/lib/fr_typography.py [--check] [directory]

U+202F before `;`, `!` and `?`, U+00A0 before `:` and inside `« »`, and the typographic
apostrophe. Placeholders, literal interpolations, code spans, addresses and the literals below
stay as they are. Idempotent: run it after a merge that brought French strings written without
it; `scripts/i18n-key.sh` applies it to every French value it writes, and
`web/src/i18n/frenchTypography.test.ts` holds the result.
"""

import json
import pathlib
import re
import sys

NARROW = "\u202F"
WIDE = "\u00A0"

# Text of a format, a header or another program, where a French space would be wrong.
LITERALS = ("all: true", "Contents: read", "SHA256:", "Cookie: nom=valeur;")

PROTECTED = re.compile(
    "|".join(
        [r"\{'[^']*'\}", r"\{\w+\}", r"`[^`]*`", r"\w+://[^\s)»]+", r"magnet:\S+"]
        + [re.escape(literal) for literal in LITERALS]
    )
)
SPACES = "[ \u00A0\u202F]*"
END = "(?=\\s|$|[)\\]»”|])"
LETTER = "[A-Za-zÀ-ÖØ-öø-ÿŒœ]"
KEPT = 0xE000


def normalise(text: str) -> str:
    """The same sentence with French spaces and apostrophes."""
    kept: list[str] = []

    def keep(match: re.Match) -> str:
        kept.append(match.group(0))
        return chr(KEPT + len(kept) - 1)

    text = PROTECTED.sub(keep, text)
    text = re.sub(f"(?<={LETTER})'(?=[\\w\uE000-\uF8FF«])", "’", text)
    text = re.sub(f"«{SPACES}", f"«{WIDE}", text)
    text = re.sub(f"{SPACES}»", f"{WIDE}»", text)
    text = re.sub(f"(?<=[^\\s;!?]){SPACES}([;!?]+){END}", f"{NARROW}\\1", text)
    text = re.sub(f"(?<=[^\\s:]){SPACES}:{END}", f"{WIDE}:", text)
    return re.sub("[\uE000-\uF8FF]", lambda match: kept[ord(match.group(0)) - KEPT], text)


def walk(value):
    if isinstance(value, str):
        return normalise(value)
    if isinstance(value, dict):
        return {key: walk(child) for key, child in value.items()}
    return value


def main() -> int:
    arguments = sys.argv[1:]
    check = "--check" in arguments
    rest = [argument for argument in arguments if argument != "--check"]
    default = pathlib.Path(__file__).resolve().parents[2] / "web/src/locales/fr"
    directory = pathlib.Path(rest[0]) if rest else default
    changed = 0
    for path in sorted(directory.glob("*.json")):
        before = path.read_text(encoding="utf-8")
        after = json.dumps(walk(json.loads(before)), ensure_ascii=False, indent=2) + "\n"
        if before == after:
            continue
        changed += 1
        print(path)
        if not check:
            path.write_text(after, encoding="utf-8")
    return 1 if check and changed else 0


if __name__ == "__main__":
    sys.exit(main())
