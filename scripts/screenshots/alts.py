#!/usr/bin/env python3
"""The wiki's alt texts for its screenshots (RD-1270-16), kept in alts.json beside this file.

    alts.py --lint                 every text in alts.json names no number; exit 1 naming each
    alts.py WIKI_DIR [--check]     write each text into every `![...](.../<view>.png)` of the
                                   wiki's pages; --check writes nothing and exits 1 where a page
                                   differs

A caption or alt text names no number — no count, version or size — so a release never has to
touch it (owner, 2026-10-12). A digit inside a name such as S3 is not a number; one standing on
its own (4K, 2026, 1.27) or a version (v1) is. alts.json holds the texts of the views that have
one; an image it does not name keeps the text its page has.
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
NUMBER = re.compile(r"(?<![A-Za-z])\d|\bv\d")
IMAGE = re.compile(r"!\[([^\]]*)\]\(([^)]*)\)")


def load():
    with open(os.path.join(HERE, "alts.json"), encoding="utf-8") as handle:
        return json.load(handle)


def lint(alts):
    bad = [f"{view}: '{m.group(0)}' in \"{text}\"" for view, text in alts.items()
           for m in [NUMBER.search(text)] if m]
    for line in bad:
        print(f"alt text names a number: {line}")
    return 1 if bad else 0


def apply(alts, wiki, check):
    changed, used = [], set()
    for top, dirs, files in os.walk(wiki):
        dirs[:] = sorted(d for d in dirs if not d.startswith("."))
        for name in sorted(f for f in files if f.endswith(".md")):
            full = os.path.join(top, name)
            with open(full, encoding="utf-8") as handle:
                text = handle.read()

            def replace(m):
                view = os.path.basename(m.group(2)).removesuffix(".png")
                if view not in alts:
                    return m.group(0)
                used.add(view)
                return f"![{alts[view]}]({m.group(2)})"

            updated = IMAGE.sub(replace, text)
            if updated != text:
                changed.append(os.path.relpath(full, wiki))
                if not check:
                    with open(full, "w", encoding="utf-8") as handle:
                        handle.write(updated)
    for page in changed:
        print(f"{'differs' if check else 'written'}: {page}")
    for view in sorted(set(alts) - used):
        print(f"note: no page shows {view}.png")
    return 1 if check and changed else 0


def main(args):
    alts = load()
    if args == ["--lint"]:
        return lint(alts)
    paths = [a for a in args if not a.startswith("--")]
    if len(paths) != 1 or not os.path.isdir(paths[0]) or set(args) - {paths[0], "--check"}:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    if lint(alts):
        return 1
    return apply(alts, paths[0], "--check" in args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
