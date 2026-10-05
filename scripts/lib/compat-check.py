#!/usr/bin/env python3
"""The breaking-change detector behind scripts/compat-check.sh (RD-170-08).

Compares two versions of the public contracts — the REST API (`web/openapi.json`) and the plugin
contract (`crates/rd-plugin-api/wit/rdownloader.wit`) — and prints one line per break. A break
passes when scripts/compat-breaks.toml acknowledges it for a release after the base, or, for the
WIT, when the package version moved far enough (a major bump; before 1.0, a minor one).

Every finding is `<area>:<kind>:<location>`, and that exact string is what an acknowledgement
names. Additions are never findings, except the ones the component model does not treat as
additions (docs/plugins.md#compatibility): a new record field, variant case, enum case or flag,
a new function on an interface a world exports, a new export of an existing world.

  compat-check.py --base-version 1.5.2 [--acks FILE]
                  [--old-openapi A --new-openapi B] [--old-wit C --new-wit D]

Exit 0 when every break is acknowledged, 1 when one is not, 2 on unusable input.
"""

import argparse
import json
import sys
import tomllib

# The two modules beside this one would otherwise leave a __pycache__ in the checkout.
sys.dont_write_bytecode = True
from compat_rest import Rest  # noqa: E402
from compat_wit import WitParser, compare_wit, version_tuple, wit_bump_covers  # noqa: E402


# --- acknowledgements and the verdict ------------------------------------------------------------


def load_acks(path, base_version):
    """{finding: release} for every acknowledgement of a release after the base."""
    if not path:
        return {}
    try:
        with open(path, "rb") as handle:
            data = tomllib.load(handle)
    except FileNotFoundError:
        return {}
    except tomllib.TOMLDecodeError as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)
    acks = {}
    base = version_tuple(base_version) or (0, 0, 0)
    for index, entry in enumerate(data.get("break", [])):
        release, finding, reason = (entry.get(k) for k in ("release", "finding", "reason"))
        if not (isinstance(release, str) and version_tuple(release)
                and isinstance(finding, str) and finding
                and isinstance(reason, str) and reason.strip()):
            print(f"compat-check: {path}: [[break]] #{index + 1} needs release (X.Y.Z), "
                  "finding and a reason", file=sys.stderr)
            sys.exit(2)
        if version_tuple(release) > base:
            acks[finding] = release
    return acks


def read_json(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)


def read_text(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError as error:
        print(f"compat-check: {path}: {error}", file=sys.stderr)
        sys.exit(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--base-version", required=True)
    parser.add_argument("--acks")
    for name in ("old-openapi", "new-openapi", "old-wit", "new-wit"):
        parser.add_argument(f"--{name}")
    args = parser.parse_args()

    findings, covered = [], []
    if args.old_openapi and args.new_openapi:
        findings += Rest(read_json(args.old_openapi), read_json(args.new_openapi)).run()
    if args.old_wit and args.new_wit:
        old = WitParser(read_text(args.old_wit)).parse()
        new = WitParser(read_text(args.new_wit)).parse()
        wit = compare_wit(old, new)
        if wit and wit_bump_covers(old.version, new.version):
            covered = wit
            print(f"wit: rdownloader:plugin@{old.version} -> @{new.version} versions "
                  f"{len(wit)} break(s)")
        else:
            findings += wit
            if old.version != new.version:
                print(f"wit: @{old.version} -> @{new.version} is not a major bump "
                      "(before 1.0: a minor one); it versions nothing")

    acks = load_acks(args.acks, args.base_version)
    unacknowledged = 0
    for key, message in covered:
        print(f"versioned  {key}  ({message})")
    for key, message in findings:
        if key in acks:
            print(f"accepted   {key}  ({message}; acknowledged for {acks[key]})")
        else:
            print(f"BREAK      {key}  ({message})")
            unacknowledged += 1
    found = {key for key, _ in findings}
    for key, release in sorted(acks.items()):
        if key not in found:
            print(f"note: the acknowledgement for {release} matches no finding: {key}")
    print(f"compat-check: {len(findings) + len(covered)} break(s) against {args.base_version}, "
          f"{unacknowledged} unacknowledged")
    return 1 if unacknowledged else 0


if __name__ == "__main__":
    sys.exit(main())
