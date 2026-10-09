#!/usr/bin/env python3
"""The `rd-changelog` merge driver for CHANGELOG.md (RD-1220-01), which .gitattributes names and
scripts/integrate.sh registers.

    changelog-merge.py <base> <ours> <theirs> [<path>]    # git's %O %A %B %P

Until 1.21 the file had git's built-in `union`, which is right for two branches adding entries
under `## [Unreleased]` and wrong across a release: the release commit moves the entries of
`[Unreleased]` under `## [X.Y.Z]`, and a branch that forked before it brings its new entries
beside those old ones, so union put them into the released section (1.21: three entries inside
`[1.20.0]` under a second `### Added`, `[Unreleased]` empty).

This driver merges by section instead. Each released section (`## [X.Y.Z] …`) and the text before
the first section are taken from the side that changed them; changed on both sides differently is
a conflict. `[Unreleased]` is merged by entry — a `- ` line with what follows it indented, or any
other paragraph: the result is our entries less those the other side removed, plus the other
side's new ones, each in its `###` group. An entry one side moved into a released section is
therefore not brought back by the other, and a new entry never lands in a released section.

Where that cannot be decided — one side released an entry the other edited or removed, a file
without `[Unreleased]`, a section twice — it merges as text with conflict markers for a person,
never a silent guess. Writes the result into <ours>; exit 0 when merged, 1 on a conflict.
"""

import re
import subprocess
import sys

ORDER = ["### Added", "### Changed", "### Deprecated", "### Removed", "### Fixed", "### Security"]
UNRELEASED = "Unreleased"


class Conflict(Exception):
    pass


def parse(text):
    """(head lines, [(key, heading, body lines)]); the key is the bracketed name of the heading."""
    head, sections = [], []
    for line in text.split("\n"):
        if line.startswith("## "):
            m = re.match(r"^## \[([^\]]+)\]", line)
            sections.append((m.group(1) if m else line, line, []))
        elif sections:
            sections[-1][2].append(line)
        else:
            head.append(line)
    keys = [k for k, _, _ in sections]
    if len(set(keys)) != len(keys) or UNRELEASED not in keys:
        raise Conflict("a section twice, or no [Unreleased]")
    return head, sections


def entries(body):
    """(groups in order, [(group, entry text)]) of a section body: a group is its `###` heading
    (None before the first), an entry a non-indented line with the indented lines after it."""
    groups, found, group, current = [None], [], None, None

    def flush():
        if current is not None:
            while current and not current[-1].strip():
                current.pop()
            found.append((group, "\n".join(current)))

    for line in body:
        if line.startswith("### "):
            flush()
            current, group = None, line
            groups.append(line)
        elif not line.strip():
            if current is not None:
                current.append(line)
        elif line[0] in " \t" and current is not None:
            current.append(line)
        else:
            flush()
            current = [line]
    flush()
    return groups, found


def pick(ours, base, theirs):
    """The three-way rule for a whole part: the side that changed it; both alike; else conflict."""
    if ours == theirs or theirs == base:
        return ours
    if ours == base:
        return theirs
    raise Conflict("changed on both sides")


def same(text):
    """An entry as compared: scripts/archive-jobs.sh rewrites a job's path once it archives it,
    released sections included, and the entry stays the same."""
    return text.replace("roadmap/jobs/archive/", "roadmap/jobs/")


def released_entries(sections):
    return {same(text) for key, _, body in sections if key != UNRELEASED
            for _, text in entries(body)[1]}


def merge_unreleased(ours, base, theirs, newly_released_ours, newly_released_theirs):
    """The result's [Unreleased] body lines, or ours unchanged."""
    o_groups, o = entries(ours)
    _, b = entries(base)
    t_groups, t = entries(theirs)
    o_texts, b_texts, t_texts = ({same(x) for _, x in s} for s in (o, b, t))
    ours_gone, theirs_gone = b_texts - o_texts, b_texts - t_texts
    # Released by one side, edited or dropped by the other: which text is right is a person's call.
    if (ours_gone - newly_released_ours) & newly_released_theirs \
            or (theirs_gone - newly_released_theirs) & newly_released_ours:
        raise Conflict("an entry released on one side and changed on the other")
    result = [(g, x) for g, x in o if same(x) not in theirs_gone]
    result += [(g, x) for g, x in t if same(x) not in b_texts and same(x) not in o_texts]
    if result == o:
        return ours
    if result == t:
        return theirs
    rank = {g: (ORDER.index(g) if g in ORDER else len(ORDER)) for g in o_groups + t_groups if g}
    groups = [None] + sorted(dict.fromkeys(g for g in o_groups + [g for g, _ in result] if g),
                             key=lambda g: rank[g])
    body = [""]
    for group in groups:
        texts = [x for g, x in result if g == group]
        if group is not None:
            body += [group, ""]
        for text in texts:
            body += [text, ""]
    return body


def merge(base, ours, theirs):
    b_head, b = parse(base)
    o_head, o = parse(ours)
    t_head, t = parse(theirs)
    b_map, o_map, t_map = ({k: (h, body) for k, h, body in s} for s in (b, o, t))
    head = pick(o_head, b_head, t_head)
    new_o = released_entries(o) - released_entries(b)
    new_t = released_entries(t) - released_entries(b)

    # The order: ours, with a section only theirs has placed before the one it precedes there.
    order = [k for k, _, _ in o]
    following = None
    for key, _, _ in reversed(t):
        if key not in order:
            order.insert(order.index(following) if following else len(order), key)
        following = key

    out = list(head)
    for key in order:
        if key == UNRELEASED:
            heading = pick(o_map[key][0], b_map.get(key, (None,))[0], t_map[key][0])
            body = merge_unreleased(o_map[key][1], b_map[key][1], t_map[key][1], new_o, new_t)
            out += [heading] + body
            continue
        chosen = pick(o_map.get(key), b_map.get(key), t_map.get(key))
        if chosen is not None:
            out += [chosen[0]] + chosen[1]
    return "\n".join(out)


def main(argv):
    base_path, ours_path, theirs_path = argv[1:4]
    path = argv[4] if len(argv) > 4 else "CHANGELOG.md"
    texts = [open(p, encoding="utf-8").read() for p in (base_path, ours_path, theirs_path)]
    try:
        merged = merge(*texts)
    except (Conflict, KeyError) as reason:
        print(f"rd-changelog: {path}: {reason}; merged as text", file=sys.stderr)
        subprocess.run(["git", "merge-file", "-L", "ours", "-L", "base", "-L", "theirs",
                        ours_path, base_path, theirs_path], check=False)
        return 1
    open(ours_path, "w", encoding="utf-8").write(merged)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
