#!/usr/bin/env python3
"""Every released section of CHANGELOG.md and RELEASE-NOTES.md as its release left it
(RD-1220-01).

    release-sections.py <repo>

A section of version X.Y.Z — `## [X.Y.Z] - date` in CHANGELOG.md, `## X.Y.Z` in RELEASE-NOTES.md —
is compared with the same section in the file at the tag vX.Y.Z, which is the release commit.
A difference is what a merge across a release does when it puts a branch's new entries into the
section the release made of [Unreleased] (1.21: three entries inside [1.20.0]); the rd-changelog
merge driver prevents it, this names what got past it. Read alike: a `roadmap/jobs/archive/` path
and the `roadmap/jobs/` one it was, since scripts/archive-jobs.sh rewrites every mention of a job
it archives, released sections included.

ACCEPTED pins the sections that already differed when this check came, by the digest of their
text now — entries that landed in a released section before 1.21 and stay where they are (no
history is rewritten). It may only shrink; a further change to one of them is named all the same.

A version whose tag this clone does not have is skipped (CI checks out without tags), and so is a
file the tag does not have yet. Prints every finding; exit 1 while there is any.
"""

import difflib
import hashlib
import re
import subprocess
import sys

FILES = (("CHANGELOG.md", re.compile(r"^## \[(\d[^\]]*)\]")),
         ("RELEASE-NOTES.md", re.compile(r"^## (\d\S*)\s*$")))
ACCEPTED = {
    # Three corrections reported after 1.0.1 and a manifest change, added after the tag.
    ("CHANGELOG.md", "1.0.1"): "af3580a338bf17a9a3fb6243a81636042c05befec9f6a234664172b8f233a518",
    # A link to docs/postprocessing.md became a code span (the public export has no docs/).
    ("CHANGELOG.md", "1.0.5"): "ffd1711ca84db52262e52e986aa3edfa48c60e13ae84c0a0994dd5839315b7d3",
    # The setup wizard's bundled services (RD-180-14), entered after the tag.
    ("CHANGELOG.md", "1.8.0-beta.1"): "6b5e406bf415390a274eab7b229299efaa720f819ec092f29dd51d266a26326b",
    # The MCP tool count fix of doc-facts.py, entered after the tag.
    ("CHANGELOG.md", "1.18.0"): "7c60b3460a83753af713970cf7d4cb03022bc23a5ee077a986c1f1f6145e2221",
}


def git(repo, *args):
    run = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, check=False)
    return run.stdout if run.returncode == 0 else None


def sections(text, heading):
    """{version: section text, archive paths read as the paths they were}."""
    found, version, lines = {}, None, []
    for line in text.split("\n") + ["## end"]:
        if line.startswith("## "):
            if version is not None:
                found[version] = "\n".join(lines).rstrip("\n").replace(
                    "roadmap/jobs/archive/", "roadmap/jobs/")
            m = heading.match(line)
            version, lines = (m.group(1) if m else None), []
        if version is not None:
            lines.append(line)
    return found


def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def main(argv):
    repo = argv[1] if len(argv) > 1 else "."
    tags = git(repo, "tag", "-l", "v*")
    if tags is None:
        print("no git repository here; nothing to compare")
        return 0
    tags = set(tags.split())
    findings = checked = skipped = 0
    for path, heading in FILES:
        try:
            now = sections(open(f"{repo}/{path}", encoding="utf-8").read(), heading)
        except FileNotFoundError:
            continue
        for version, text in now.items():
            if f"v{version}" not in tags:
                skipped += 1
                continue
            released = git(repo, "show", f"v{version}:{path}")
            if released is None:
                continue
            then = sections(released, heading).get(version)
            checked += 1
            if text == then or ACCEPTED.get((path, version)) == digest(text):
                continue
            findings += 1
            if then is None:
                print(f"!! {path}: the section {version} is not in the file at v{version}")
                continue
            print(f"!! {path}: the section {version} differs from its release commit v{version} "
                  "— a merge across the release put text there; move it under the coming version:")
            diff = list(difflib.unified_diff(then.split("\n"), text.split("\n"),
                                             f"v{version}:{path}", path, n=1, lineterm=""))
            print("\n".join(f"   {line}" for line in diff[:40]))
            if len(diff) > 40:
                print(f"   … {len(diff) - 40} more diff lines")
    print(f"released sections: {checked} compared with their tag, {skipped} without a tag here, "
          f"{findings} differ")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
