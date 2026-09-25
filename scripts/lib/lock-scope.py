#!/usr/bin/env python3
"""Which workspace crates a change to Cargo.lock or the root Cargo.toml reaches (RD-130-17).

    lock-scope.py OLD NEW

OLD and NEW are two trees holding the root Cargo.toml, Cargo.lock and the Cargo.toml of every
workspace member at its member path: a checkout, or the manifests of a commit unpacked by
`rd_scope_lock_crates` in scripts/lib/scope.sh. The answer on stdout is either the one line
`WIDE <reason>`, or one line per affected member, `<package> <dir> <why>`, sorted by package.
No line at all means that no member's resolved tree changed. Anything this script cannot read
is an error (exit 1), and scripts/check.sh treats an error as WIDE.

Until RD-130-17 any change to either file widened a branch check to the whole workspace. On
2026-09-24 three of eight branch checks did, one only because rd-postprocess took
`tracing-subscriber` as a dev-dependency. The rules instead:

  * A lock package changed when it was added or removed (a version bump is both), or when its
    checksum or its resolved dependency set differs. Packages are keyed by name, version and
    source; dependencies are compared resolved, so a `"foo"` that becomes `"foo 1.0.2"` only
    because a second version of foo arrived is no change.
  * A member is affected when its dependency closure, in the old or in the new lock graph,
    contains a changed package. From the member itself every edge is followed, its
    dev-dependencies included; from any other member reached on the way only its normal and
    build edges, because another crate's dev-dependencies never enter this one's build. The lock
    does not say which edge is which; the member manifests do.
  * A member whose own lock entry changed only in its dev-dependencies is affected itself and
    carries the change no further.
  * The root Cargo.toml: a change inside [workspace.dependencies] adds the members that take the
    changed dependency with `workspace = true` (features are not in the lock). Any other change
    there — members, [workspace.package], [workspace.lints], [profile.*], [patch], [replace],
    anything — is WIDE, and so is a change to the lock's own format or a member set that differs.

The toolchain, nextest and deny configuration are WIDE by path; check.sh decides those.
"""

import glob
import os
import sys
import tomllib

DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


class Wide(Exception):
    """The change governs the whole build; the message says why."""


def load(path):
    with open(path, "rb") as handle:
        return tomllib.load(handle)


def dep_tables(manifest):
    """(table name, table) for every dependency table, the target-specific ones included."""
    for table in DEP_TABLES:
        yield table, manifest.get(table, {})
    for target in manifest.get("target", {}).values():
        for table in DEP_TABLES:
            yield table, target.get(table, {})


class Tree:
    def __init__(self, root):
        self.manifest = load(os.path.join(root, "Cargo.toml"))
        self.lock = load(os.path.join(root, "Cargo.lock"))
        workspace = self.manifest.get("workspace", {})
        self.workspace_deps = workspace.get("dependencies", {})

        # package name -> (member directory, member manifest)
        self.members = {}
        excluded = set(workspace.get("exclude", []))
        for pattern in workspace.get("members", []):
            matches = sorted(glob.glob(os.path.join(root, pattern)))
            if not matches and not glob.has_magic(pattern):
                raise ValueError(f"{root}: member {pattern} has no directory")
            for directory in matches:
                relative = os.path.relpath(directory, root)
                path = os.path.join(directory, "Cargo.toml")
                if relative in excluded or not os.path.isfile(path):
                    continue
                manifest = load(path)
                self.members[manifest["package"]["name"]] = (relative, manifest)

        self.entries = {}
        by_name = {}
        for entry in self.lock.get("package", []):
            key = (entry["name"], entry["version"], entry.get("source", ""))
            self.entries[key] = entry
            by_name.setdefault(entry["name"], []).append(key)
        self.deps = {
            key: frozenset(resolve(spec, by_name) for spec in entry.get("dependencies", []))
            for key, entry in self.entries.items()
        }
        self.member_keys = {
            key[0]: key for key in self.entries if not key[2] and key[0] in self.members
        }
        self._kinds = {}

    def kinds(self, member):
        """Dependency name -> the set of {"normal", "dev"} it is declared as by `member`."""
        if member not in self._kinds:
            kinds = {}
            for table, deps in dep_tables(self.members[member][1]):
                for key, spec in deps.items():
                    name = key
                    if isinstance(spec, dict):
                        if spec.get("workspace"):
                            inherited = self.workspace_deps.get(key, {})
                            if isinstance(inherited, dict):
                                name = inherited.get("package", key)
                        name = spec.get("package", name)
                    kinds.setdefault(name, set()).add(
                        "dev" if table == "dev-dependencies" else "normal"
                    )
            self._kinds[member] = kinds
        return self._kinds[member]

    def build_edges(self, key):
        """The edges of `key` that reach a dependant's build: a member's dev edges do not."""
        if self.member_keys.get(key[0]) != key:
            return self.deps[key]
        kinds = self.kinds(key[0])
        return frozenset(d for d in self.deps[key] if kinds.get(d[0]) != {"dev"})

    def closure(self, member):
        start = self.member_keys[member]
        seen = {start}
        pending = [start]
        while pending:
            key = pending.pop()
            edges = self.deps[key] if key == start else self.build_edges(key)
            for dep in edges - seen:
                seen.add(dep)
                pending.append(dep)
        return seen


def resolve(spec, by_name):
    """A lock dependency string, `name`, `name version` or `name version (source)`, to its key."""
    parts = spec.split(" ", 2)
    candidates = by_name.get(parts[0], [])
    if len(parts) > 1:
        candidates = [key for key in candidates if key[1] == parts[1]]
    if len(parts) > 2:
        candidates = [key for key in candidates if key[2] == parts[2].strip("()")]
    if len(candidates) != 1:
        raise ValueError(f"lock dependency {spec!r} resolves to {len(candidates)} packages")
    return candidates[0]


def changed_workspace_deps(old, new):
    """The [workspace.dependencies] names that changed; WIDE for any other root change."""
    for key in sorted(set(old.manifest) | set(new.manifest)):
        if key != "workspace" and old.manifest.get(key) != new.manifest.get(key):
            raise Wide(f"[{key}] in the root Cargo.toml changed")
    old_ws = old.manifest.get("workspace", {})
    new_ws = new.manifest.get("workspace", {})
    for key in sorted(set(old_ws) | set(new_ws)):
        if key != "dependencies" and old_ws.get(key) != new_ws.get(key):
            raise Wide(f"[workspace.{key}] in the root Cargo.toml changed")
    for key in sorted(set(old.lock) | set(new.lock)):
        if key != "package" and old.lock.get(key) != new.lock.get(key):
            raise Wide(f"the Cargo.lock field `{key}` changed")
    if set(old.members) != set(new.members):
        raise Wide("a workspace member was added or removed")
    old_deps, new_deps = old.workspace_deps, new.workspace_deps
    return sorted(k for k in set(old_deps) | set(new_deps) if old_deps.get(k) != new_deps.get(k))


def scope(old, new):
    """{member: [reasons]} for every affected member."""
    reasons = {}

    for name in changed_workspace_deps(old, new):
        for tree in (old, new):
            for member, (_, manifest) in tree.members.items():
                for _, deps in dep_tables(manifest):
                    spec = deps.get(name)
                    if isinstance(spec, dict) and spec.get("workspace"):
                        reasons.setdefault(member, set()).add(f"workspace dependency {name}")

    # `primary` is what arrived, left or was republished; `carried` is an entry whose resolved
    # dependencies changed. A member is told about the first where it reaches one, since that
    # is the cause, and about the second only otherwise.
    primary = set(old.entries) ^ set(new.entries)
    carried = set()
    for key in set(old.entries) & set(new.entries):
        if old.entries[key].get("checksum") != new.entries[key].get("checksum"):
            primary.add(key)
        elif old.deps[key] != new.deps[key]:
            if old.build_edges(key) == new.build_edges(key):
                # Only a member's dev-dependencies moved: its own tests, nobody else's build.
                reasons.setdefault(key[0], set()).add(f"{key[0]} (dev-dependencies)")
            else:
                carried.add(key)

    def describe(name):
        gone = sorted(k[1] for k in primary if k[0] == name and k not in new.entries)
        came = sorted(k[1] for k in primary if k[0] == name and k not in old.entries)
        if gone and came:
            return f"{name} {','.join(gone)} -> {','.join(came)}"
        if came:
            return f"{name} {','.join(came)} added"
        if gone:
            return f"{name} {','.join(gone)} removed"
        return f"{name} (checksum)"

    reached = {}
    for tree in (old, new):
        for member in tree.member_keys:
            reached.setdefault(member, set()).update(tree.closure(member) & (primary | carried))
    for member, changed in reached.items():
        causes = changed & primary
        if causes:
            reasons.setdefault(member, set()).update(describe(k[0]) for k in causes)
        elif changed:
            reasons.setdefault(member, set()).update(f"{k[0]} (dependencies)" for k in changed)
    return reasons


def main(argv):
    if len(argv) != 3:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    try:
        old, new = Tree(argv[1]), Tree(argv[2])
        reasons = scope(old, new)
    except Wide as wide:
        print(f"WIDE {wide}")
        return 0
    except (OSError, KeyError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"lock-scope.py: {error!r}", file=sys.stderr)
        return 1
    for member in sorted(reasons):
        directory = (new.members.get(member) or old.members[member])[0]
        why = sorted(reasons[member])
        text = ", ".join(why[:3]) + (f" and {len(why) - 3} more" if len(why) > 3 else "")
        print(f"{member} {directory} {text}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
