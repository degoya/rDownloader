#!/usr/bin/env python3
"""Writes or checks the release facts the documentation repeats (RD-140-24).

    doc-facts.py <repo> [--check] [--wiki DIR] [--date YYYY-MM-DD]

Three facts move with a release and are read from their source, never from a document:

  version  the workspace version, `[workspace.package]` in Cargo.toml
  plugins  the number of plugins/*/manifest.toml the bundle ships: the examples
           (plugins/example-*) are built but not bundled (RD-150-20)
  wit      the plugin contract, `package rdownloader:plugin@X.Y.Z;` in
           crates/rd-plugin-api/wit/rdownloader.wit

Each sentence that states one is an anchor below: a file and a pattern whose named group is the
value. Every anchor has to match at least once; one that no longer does was reworded, and the run
refuses (exit 2) rather than guess — adjust the pattern here in the same change as the sentence.
A historical mention ("the contract moved to …@0.7.0") is not an anchor, which is why the
patterns carry their sentence's wording rather than matching every `rdownloader:plugin@`.

The feature list's header carries the date as well: when the version it names changes, the date
becomes today's (or --date); when the version is already right, the date is left as it is, so a
second run changes nothing.

--wiki DIR takes the user wiki in as well (`~/projects/rdownloader.wiki`). The wiki describes the
current release only, so there every `rdownloader:plugin@X.Y.Z` on any page is the contract.

Writes by default and prints what it changed. --check writes nothing, prints every stale value
with its place, and exits 1 when there is one. The release pipeline runs the first as its
`doc-facts` step and the second inside `docs-gate`.
"""

import datetime
import os
import re
import sys

WIT = "crates/rd-plugin-api/wit/rdownloader.wit"


def words(pattern):
    """A sentence may be rewrapped: a space in an anchor matches any run of whitespace."""
    return pattern.replace(" ", r"\s+")


# (file, pattern) — the named group is the fact the pattern states.
REPO_ANCHORS = [
    ("docs/feature-list.md",
     r"^> As of (?P<date>[A-Z][a-z]+ \d{1,2}, \d{4}) · Source version (?P<version>\d+\.\d+\.\d+)\."),
    ("docs/feature-list.md", words(r"\| Bundled plugins \| (?P<plugins>\d+) signed")),
    ("docs/feature-list.md", words(r"Versioned WIT interface `rdownloader:plugin@(?P<wit>[^`]+)`")),
    ("docs/plugins.md", words(r"all (?P<plugins>\d+) signed components")),
    ("docs/plugins.md", words(r"the plugin package is `rdownloader:plugin@(?P<wit>[^`]+)`")),
    ("docs/development.md", words(r"WIT interface `rdownloader:plugin@(?P<wit>[^`]+)`")),
    ("README.md", words(r"versioned contract `rdownloader:plugin@(?P<wit>[^`]+)`")),
    ("sdk/README.md", words(r"The current package is `rdownloader:plugin@(?P<wit>[^`]+)`")),
]

WIKI_ANCHORS = [
    ("home.md", words(r"\| Bundled plugins \| (?P<plugins>\d+) signed")),
]
WIKI_EVERYWHERE = r"rdownloader:plugin@(?P<wit>\d+\.\d+\.\d+)"


def read_facts(repo):
    cargo = open(os.path.join(repo, "Cargo.toml"), encoding="utf-8").read()
    section = re.search(r"^\[workspace\.package\]\n(.*?)(?=^\[|\Z)", cargo, re.MULTILINE | re.DOTALL)
    version = re.search(r'^version = "([^"]+)"', section.group(1), re.MULTILINE) if section else None
    wit = re.search(r"^package rdownloader:plugin@(\S+);",
                    open(os.path.join(repo, WIT), encoding="utf-8").read(), re.MULTILINE)
    plugins = os.path.join(repo, "plugins")
    count = sum(os.path.isfile(os.path.join(plugins, d, "manifest.toml")) and not d.startswith("example-")
                for d in os.listdir(plugins))
    if not version or not wit:
        raise SystemExit("could not read the workspace version or the WIT package line")
    return {"version": version.group(1), "plugins": str(count), "wit": wit.group(1)}


def long_date(iso):
    d = datetime.date.fromisoformat(iso)
    return f"{d.strftime('%B')} {d.day}, {d.year}"


class Run:
    def __init__(self, repo, facts, check, date):
        self.repo, self.facts, self.check, self.date = repo, facts, check, date
        self.stale, self.changed, self.missing = [], [], []

    def apply(self, base, rel, pattern, required=True):
        full = os.path.join(base, rel)
        path = rel if base == self.repo else full
        if not os.path.isfile(full):
            self.missing.append(f"{path}: file missing")
            return
        text = open(full, encoding="utf-8").read()
        found = [0]

        def replace(m):
            found[0] += 1
            out, cursor = [], m.start()
            version_moved = "version" in m.groupdict() and m.group("version") != self.facts["version"]
            for name in m.groupdict():
                have = m.group(name)
                if name == "date":
                    want = long_date(self.date) if version_moved else have
                else:
                    want = self.facts[name]
                if have != want and name != "date":
                    line = text.count("\n", 0, m.start(name)) + 1
                    self.stale.append(f"{path}:{line}: {name} is {have}, the source says {want}")
                out.append(text[cursor:m.start(name)] + want)
                cursor = m.end(name)
            out.append(text[cursor:m.end()])
            return "".join(out)

        updated = re.sub(pattern, replace, text, flags=re.MULTILINE)
        if required and not found[0]:
            self.missing.append(f"{path}: no match for the anchor /{pattern}/ — reworded? "
                                "adjust scripts/lib/doc-facts.py with the sentence")
        if updated != text and not self.check:
            open(full, "w", encoding="utf-8").write(updated)
            self.changed.append(path)


def main(argv):
    args = argv[1:]
    check = "--check" in args
    wiki = args[args.index("--wiki") + 1] if "--wiki" in args else None
    date = args[args.index("--date") + 1] if "--date" in args else datetime.date.today().isoformat()
    repo = next((a for a in args if not a.startswith("--") and a not in (wiki, date)), None)
    if repo is None:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2

    facts = read_facts(repo)
    run = Run(repo, facts, check, date)
    for rel, pattern in REPO_ANCHORS:
        run.apply(repo, rel, pattern)
    if wiki:
        for rel, pattern in WIKI_ANCHORS:
            run.apply(wiki, rel, pattern)
        for top, dirs, files in os.walk(wiki):
            dirs[:] = sorted(d for d in dirs if not d.startswith("."))
            for name in sorted(files):
                if name.endswith(".md"):
                    run.apply(wiki, os.path.relpath(os.path.join(top, name), wiki),
                              WIKI_EVERYWHERE, required=False)

    summary = f"version {facts['version']}, {facts['plugins']} plugins, rdownloader:plugin@{facts['wit']}"
    for line in run.missing:
        print(line, file=sys.stderr)
    if check:
        for line in run.stale:
            print(f"stale: {line}")
        if run.stale:
            print(f"{len(run.stale)} stale value(s) — run scripts/doc-facts.sh"
                  + (f" --wiki {wiki}" if wiki else ""))
        elif not run.missing:
            print(f"doc facts current: {summary}" + (" (with the wiki)" if wiki else ""))
    else:
        for line in run.stale:
            print(f"updated: {line}")
        print(f"doc facts written: {summary}; {len(run.changed)} file(s) changed")
    if run.missing:
        return 2
    return 1 if check and run.stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
