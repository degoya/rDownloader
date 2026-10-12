#!/usr/bin/env python3
"""Copies taken views into every place targets.json names (RD-1270-16).

    deliver.py --views a,b [--targets wiki,website,readme] [--line X.Y] [--out DIR]
               [--wiki DIR] [--website DIR] [--repo DIR] [--dry-run]

Only the views named are delivered: a release replaces the screens that changed, never all of
them. A view is a name of views.mjs or its number (05). Per target (targets.json):

  wiki     <wiki>/images/screenshots/<view>.png from <out>/1x; a view new to the wiki is refused
           when another image already has its number
  website  <website>/public/screenshots/<name>.webp from <out>/2x, through ImageMagick's `magick`
           — only the views the website shows
  readme   <repo>/.github/readme/downloads.png from <out>/1x

Every source is checked first — present, the PNG at targets.json's size times its scale — and
nothing is written unless all of them hold. --out defaults to <RD_SHOTS_WORK>/out; --wiki to
RD_WIKI_SRC (~/projects/rdownloader.wiki), --website to RD_WEBSITE_DIR
(~/projects/rdownloader-website), each with -X.Y appended under --line X.Y: the release worktrees
of scripts/release-branches.sh; --repo to the checkout this script lies in.
"""

import json
import os
import re
import shutil
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def view_names():
    with open(os.path.join(HERE, "views.mjs"), encoding="utf-8") as handle:
        return re.findall(r"^\s*(?:view|settings)\((?:'[a-z]+', )?'(\d{2}-[a-z0-9-]+)'", handle.read(), re.M)


def png_size(path):
    with open(path, "rb") as handle:
        head = handle.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    return struct.unpack(">II", head[16:24])


def option(args, name, default=None):
    if name in args:
        at = args.index(name)
        if at + 1 >= len(args) or args[at + 1].startswith("--"):
            raise SystemExit(f"{name} needs a value")
        return args[at + 1]
    return default


def roots(args):
    line = option(args, "--line")
    if line is not None and not re.fullmatch(r"\d+\.\d+", line):
        raise SystemExit(f"--line takes X.Y, not '{line}'")
    suffix = f"-{line}" if line else ""
    home = os.path.expanduser("~")
    wiki = os.environ.get("RD_WIKI_SRC") or os.path.join(home, "projects/rdownloader.wiki")
    site = os.environ.get("RD_WEBSITE_DIR") or os.path.join(home, "projects/rdownloader-website")
    return {
        "wiki": option(args, "--wiki", wiki + suffix),
        "website": option(args, "--website", site + suffix),
        "repo": option(args, "--repo", REPO),
    }


def plan(manifest, views, chosen, root_of, out):
    """The copies to make as (kind, source, destination, scale), and every refusal."""
    copies, refusals = [], []
    capture = manifest["capture"]
    for target in chosen:
        spec = manifest["targets"][target]
        root = root_of[spec["root"]]
        if not os.path.isdir(root):
            refusals.append(f"{target}: {root} is not a directory")
            continue
        folder = os.path.join(root, spec["dir"])
        for view in views:
            mapping = spec["views"]
            if mapping != "*" and view not in mapping:
                continue
            name = view if mapping == "*" else mapping[view]
            source = os.path.join(out, f"{spec['scale']}x", f"{view}.png")
            dest = os.path.join(folder, f"{name}.{spec['format']}")
            want = (capture["width"] * spec["scale"], capture["height"] * spec["scale"])
            size = png_size(source) if os.path.isfile(source) else None
            if size is None:
                refusals.append(f"{target}: no PNG {source}")
                continue
            if size != want:
                refusals.append(f"{target}: {source} is {size[0]}x{size[1]}, {target} wants {want[0]}x{want[1]}")
                continue
            if target == "wiki" and not os.path.exists(dest) and os.path.isdir(folder):
                number = view.split("-", 1)[0] + "-"
                taken = [f for f in os.listdir(folder) if f.startswith(number)]
                if taken:
                    refusals.append(f"wiki: {view} is new, but {taken[0]} already has its number")
                    continue
            copies.append((target, spec, source, dest))
    return copies, refusals


def main(args):
    if not args or "-h" in args or "--help" in args:
        print(__doc__)
        return 0 if args else 2
    with open(os.path.join(HERE, "targets.json"), encoding="utf-8") as handle:
        manifest = json.load(handle)
    known = view_names()
    wanted = [v.strip() for v in (option(args, "--views") or "").split(",") if v.strip()]
    if not wanted:
        print("--views is required: name the views that changed", file=sys.stderr)
        return 2
    views, refusals = [], []
    for entry in wanted:
        match = [n for n in known if n == entry or n.startswith(f"{entry}-")]
        hand = [n for n in manifest["by_hand"] if n == entry or n.startswith(f"{entry}-")]
        if match:
            views += [n for n in match if n not in views]
        elif hand:
            refusals.append(f"{hand[0]} is taken by hand: {manifest['by_hand'][hand[0]]}")
        else:
            refusals.append(f"no view '{entry}'")
    chosen = (option(args, "--targets") or ",".join(manifest["targets"])).split(",")
    refusals += [f"no target '{t}'" for t in chosen if t not in manifest["targets"]]
    out = option(args, "--out") or (os.path.join(os.environ["RD_SHOTS_WORK"], "out")
                                     if os.environ.get("RD_SHOTS_WORK") else None)
    if out is None:
        refusals.append("no --out and no RD_SHOTS_WORK")
    if refusals:
        for line in refusals:
            print(f"refused: {line}", file=sys.stderr)
        return 1
    copies, refusals = plan(manifest, views, chosen, roots(args), out)
    if refusals:
        for line in refusals:
            print(f"refused: {line}", file=sys.stderr)
        print("nothing delivered", file=sys.stderr)
        return 1
    if any(spec["format"] == "webp" for _, spec, _, _ in copies) and not shutil.which("magick"):
        print("refused: the website's WebP needs ImageMagick's `magick`", file=sys.stderr)
        return 1
    dry = "--dry-run" in args
    for target, spec, source, dest in copies:
        if not dry:
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            if spec["format"] == "webp":
                subprocess.run(["magick", source, "-quality", str(spec.get("quality", 85)), dest], check=True)
            else:
                shutil.copyfile(source, dest)
        print(f"{'would write' if dry else 'written'} {target:8} {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
