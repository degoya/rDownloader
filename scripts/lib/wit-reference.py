#!/usr/bin/env python3
"""Generates the plugin contract reference from the WIT (RD-160-04).

    wit-reference.py <repo> [--print] [--check] [--wiki DIR]

The contract, crates/rd-plugin-api/wit/rdownloader.wit, is the one source of what a plugin can
call and must export. The user wiki's plugin reference (`plugins/plugin-reference.md`) carries a
generated part between two markers, and this script writes it:

    <!-- BEGIN wit-reference -->
    ...
    <!-- END wit-reference -->

Every world with its imports, exports and scaffold; every interface with its functions, records,
variants and enums, each with the doc comment the WIT gives it. Nothing in the generated part is
edited by hand: a contract change is followed by a run of this script, in the same change.

  --print        the Markdown to stdout
  --wiki DIR     write it into DIR/plugins/plugin-reference.md between the markers
  --wiki DIR --check
                 write nothing; exit 1 when the page's generated part differs from the WIT
  --check        (without --wiki) parse the contract strictly and write nothing: exit 0 when
                 every line is understood. CI runs this, since the wiki is a repository of its
                 own; a construct the reader does not know (a `resource`, a `flags`, a type
                 alias) is exit 2, so the reference cannot silently leave part of the contract out.

Exit 2 also when the page has no markers, or they are out of order: the run refuses rather than
guess where the part belongs.

The wiki names no job ids and no ADRs, the WIT's comments do: a parenthesis holding only those,
`(RD-150-03)` or `(RD-110-33, ADR 0011)`, is left out of the page, and a job id anywhere else in a
doc comment is exit 2, since the sentence around it would not survive losing it.
"""

import os
import re
import sys

WIT = "crates/rd-plugin-api/wit/rdownloader.wit"
PAGE = "plugins/plugin-reference.md"
BEGIN = "<!-- BEGIN wit-reference -->"
END = "<!-- END wit-reference -->"


# `(RD-150-03)`, `(RD-103-00, RD-106-01)`, `(RD-110-33, ADR 0011)`, with the space before it,
# also when the WIT broke the line right before the parenthesis.
TRACE_ID = r"(?:RD-\d{3,4}-\d{2}|ADR \d{4})"
TRACE = re.compile(rf"[ \t]*(?:\n[ \t]*)?\({TRACE_ID}(?:,\s*{TRACE_ID})*\)")
JOB_ID = re.compile(r"\bRD-\d{3,4}-\d{2}\b")


class Refusal(Exception):
    """A contract or a page this script will not guess about."""


# --- reading ------------------------------------------------------------------------------


def parse(text):
    """The contract as plain data: package, interfaces and worlds, in file order."""
    contract = {"package": None, "interfaces": [], "worlds": []}
    lines = text.split("\n")
    docs = []
    index = 0

    def take_docs():
        nonlocal docs
        taken, docs = docs, []
        return taken

    while index < len(lines):
        raw = lines[index]
        line = raw.strip()
        index += 1
        if not line:
            continue
        if line.startswith("///"):
            docs.append(doc_text(line))
            continue
        match = re.fullmatch(r"package ([a-z0-9:-]+@[0-9.]+);", line)
        if match:
            contract["package"] = match.group(1)
            take_docs()
            continue
        match = re.fullmatch(r"(interface|world) ([a-z0-9-]+) \{", line)
        if not match:
            raise Refusal(f"{WIT}:{index}: not understood at the top level: {line}")
        kind, name = match.groups()
        item = {"name": name, "docs": take_docs()}
        if kind == "interface":
            item.update({"uses": [], "types": [], "functions": []})
            index = parse_interface(lines, index, item)
            contract["interfaces"].append(item)
        else:
            item.update({"imports": [], "exports": []})
            index = parse_world(lines, index, item)
            contract["worlds"].append(item)
    if contract["package"] is None:
        raise Refusal(f"{WIT}: no package line")
    return contract


def doc_text(line):
    """The text of one `///` line, with the single space after the slashes removed."""
    text = line[3:]
    return text[1:] if text.startswith(" ") else text


def parse_interface(lines, index, interface):
    docs = []
    while index < len(lines):
        line = lines[index].strip()
        index += 1
        if not line:
            continue
        if line == "}":
            return index
        if line.startswith("///"):
            docs.append(doc_text(line))
            continue
        match = re.fullmatch(r"use ([a-z0-9-]+)\.\{([a-z0-9, -]+)\};", line)
        if match:
            names = [name.strip() for name in match.group(2).split(",")]
            interface["uses"].append((match.group(1), names))
            docs = []
            continue
        match = re.fullmatch(r"(record|variant|enum) ([a-z0-9-]+) \{", line)
        if match:
            kind, name = match.groups()
            members, index = parse_members(lines, index, kind)
            interface["types"].append(
                {"kind": kind, "name": name, "docs": docs, "members": members})
            docs = []
            continue
        match = re.fullmatch(r"([a-z0-9-]+): func\((.*)\)(?: -> (.+))?;", line)
        if match:
            name, params, result = match.groups()
            interface["functions"].append(
                {"name": name, "params": params, "result": result, "docs": docs})
            docs = []
            continue
        raise Refusal(f"{WIT}:{index}: not understood in interface "
                      f"`{interface['name']}`: {line}")
    raise Refusal(f"{WIT}: interface `{interface['name']}` is never closed")


def parse_members(lines, index, kind):
    """The fields of a record or the cases of a variant or an enum, each with its docs."""
    members, docs = [], []
    pattern = {
        "record": r"([a-z0-9-]+): (.+),",
        "variant": r"([a-z0-9-]+)(?:\((.+)\))?,",
        "enum": r"([a-z0-9-]+)(),",
    }[kind]
    while index < len(lines):
        line = lines[index].strip()
        index += 1
        if not line:
            continue
        if line == "}":
            return members, index
        if line.startswith("///"):
            docs.append(doc_text(line))
            continue
        match = re.fullmatch(pattern, line)
        if not match:
            raise Refusal(f"{WIT}:{index}: not understood in a {kind}: {line}")
        members.append({"name": match.group(1), "type": match.group(2) or None, "docs": docs})
        docs = []
    raise Refusal(f"{WIT}: a {kind} is never closed")


def parse_world(lines, index, world):
    while index < len(lines):
        line = lines[index].strip()
        index += 1
        if not line or line.startswith("///"):
            continue
        if line == "}":
            return index
        match = re.fullmatch(r"(import|export) ([a-z0-9-]+);", line)
        if not match:
            raise Refusal(f"{WIT}:{index}: not understood in world `{world['name']}`: {line}")
        world[match.group(1) + "s"].append(match.group(2))
    raise Refusal(f"{WIT}: world `{world['name']}` is never closed")


# --- writing ------------------------------------------------------------------------------


def cell(docs):
    """Doc lines as one table cell: joined, pipes escaped."""
    return " ".join(line.strip() for line in docs if line.strip()).replace("|", "\\|")


def code(text):
    return f"`{text}`"


def render(contract):
    out = [
        f"<!-- Generated by scripts/wit-reference.sh from {WIT}. Edit the WIT, not this part. -->",
        "",
        f"### Contract reference: `{contract['package']}`",
        "",
        "Every world a plugin can be built against, and every interface in the contract, as the WIT",
        "states them. A world's name is also the scaffold's: `rdownloader plugin new --type <name>`",
        "for the world `<name>-plugin`.",
        "",
        "#### Worlds",
        "",
        "| World | Imports | Exports | Scaffold |",
        "| --- | --- | --- | --- |",
    ]
    for world in contract["worlds"]:
        template = world["name"].removesuffix("-plugin")
        out.append(
            f"| {code(world['name'])} | {', '.join(map(code, world['imports']))} "
            f"| {', '.join(map(code, world['exports']))} | `--type {template}` |")
    described = [world for world in contract["worlds"] if world["docs"]]
    if described:
        out.append("")
        for world in described:
            out.append(f"- {code(world['name'])}: {cell(world['docs'])}")
    for interface in contract["interfaces"]:
        out += ["", f"#### Interface `{interface['name']}`", ""]
        if interface["docs"]:
            out += interface["docs"] + [""]
        for source, names in interface["uses"]:
            out += [f"Uses {', '.join(map(code, names))} from {code(source)}.", ""]
        for function in interface["functions"]:
            signature = f"{function['name']}: func({function['params']})"
            if function["result"]:
                signature += f" -> {function['result']}"
            out.append(f"- `{signature}`")
            if function["docs"]:
                out.append("")
                out += [f"  {line}" if line else "" for line in function["docs"]]
                out.append("")
        if interface["functions"] and out[-1] != "":
            out.append("")
        for item in interface["types"]:
            out.append(f"**{item['kind']} `{item['name']}`**")
            out.append("")
            if item["docs"]:
                out += item["docs"] + [""]
            if item["kind"] == "record":
                out += ["| Field | Type | Meaning |", "| --- | --- | --- |"]
            elif item["kind"] == "variant":
                out += ["| Case | Payload | Meaning |", "| --- | --- | --- |"]
            else:
                out += ["| Case | Meaning |", "| --- | --- |"]
            for member in item["members"]:
                if item["kind"] == "enum":
                    out.append(f"| {code(member['name'])} | {cell(member['docs'])} |")
                else:
                    payload = code(member["type"]) if member["type"] else ""
                    out.append(
                        f"| {code(member['name'])} | {payload} | {cell(member['docs'])} |")
            out.append("")
    while out[-1] == "":
        out.pop()
    return without_traces("\n".join(out) + "\n")


def without_traces(text):
    """`text` without the job-id and ADR parentheses of the WIT's comments."""
    text = TRACE.sub("", text)
    left = JOB_ID.search(text)
    if left:
        line = text[text.rfind("\n", 0, left.start()) + 1 : text.find("\n", left.end())]
        raise Refusal(f"{WIT}: a job id outside a parenthesis of its own: {line.strip()}")
    return text


def splice(page, generated, where):
    """`page` with the part between the markers replaced by `generated`."""
    begin, end = page.find(BEGIN), page.find(END)
    if begin < 0 or end < 0 or end < begin or page.count(BEGIN) != 1 or page.count(END) != 1:
        raise Refusal(f"{where}: needs exactly one `{BEGIN}` followed by one `{END}`")
    return page[: begin + len(BEGIN)] + "\n" + generated + page[end:]


def main(argv):
    if not argv or argv[0].startswith("-"):
        print(__doc__, file=sys.stderr)
        return 2
    repo, args = argv[0], argv[1:]
    check = "--check" in args
    show = "--print" in args
    wiki = None
    if "--wiki" in args:
        position = args.index("--wiki")
        if position + 1 >= len(args):
            print("--wiki needs a directory", file=sys.stderr)
            return 2
        wiki = args[position + 1]
    known = {"--check", "--print", "--wiki", wiki}
    unknown = [arg for arg in args if arg not in known]
    if unknown:
        print(f"unknown argument: {unknown[0]}", file=sys.stderr)
        return 2
    try:
        with open(os.path.join(repo, WIT), encoding="utf-8") as handle:
            contract = parse(handle.read())
        generated = render(contract)
        if show:
            sys.stdout.write(generated)
        if wiki is None:
            if check:
                print(f"wit-reference: {WIT} read completely "
                      f"({len(contract['worlds'])} worlds, "
                      f"{len(contract['interfaces'])} interfaces)")
            return 0
        path = os.path.join(wiki, PAGE)
        with open(path, encoding="utf-8") as handle:
            page = handle.read()
        updated = splice(page, generated, path)
    except (Refusal, OSError) as refusal:
        print(f"wit-reference: {refusal}", file=sys.stderr)
        return 2
    if updated == page:
        print(f"wit-reference: {path} is current")
        return 0
    if check:
        print(f"wit-reference: {path} differs from {WIT}; run scripts/wit-reference.sh --wiki {wiki}")
        return 1
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(updated)
    print(f"wit-reference: wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
