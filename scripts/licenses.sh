#!/usr/bin/env bash
#
# Regenerates crates/rd-api/licenses/third-party.json, the dependency licences the About page
# lists (RD-130-12).
#
# Generated, never kept by hand, from two sources:
#   * `cargo metadata`: every crate a workspace member reaches through normal dependencies, on
#     any platform — what ends up in the binaries and the plugin components. The crates only
#     tests and build scripts reach are listed by name as not shipped, so the test below can
#     tell a crate new to Cargo.lock from one this list forgot.
#   * web/package-lock.json: every package npm does not mark as a development one.
# A package that declares no licence at all takes its entry from
# crates/rd-api/licenses/overrides.json, which names where the licence was read; without one
# this script refuses to write the list.
#
# Three tests in `about::tests` (rd-api's library) keep it honest and name this script: every
# crate of Cargo.lock and every production package of package-lock.json is listed, and every
# listed entry has a licence. Neither is run here.
#
# Usage:
#   scripts/licenses.sh            # rewrite the list
#   scripts/licenses.sh --check    # fail if it would change anything
#
# Needs no build lock: `cargo metadata` compiles nothing. It may download the manifests of
# crates for other platforms the first time.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

check_only=0
for argument in "$@"; do
    case "$argument" in
        --check) check_only=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

metadata="$(mktemp)"
trap 'rm -f "$metadata"' EXIT
cargo metadata --format-version 1 --locked > "$metadata"

python3 - "$metadata" "$check_only" <<'PY'
import json
import pathlib
import sys

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text())
check_only = sys.argv[2] == "1"
target = pathlib.Path("crates/rd-api/licenses/third-party.json")
overrides = json.loads(pathlib.Path("crates/rd-api/licenses/overrides.json").read_text())
missing = []


def licence(ecosystem, name, version, declared):
    override = overrides.get(ecosystem, {}).get(f"{name}@{version}")
    if override:
        return override["license"]
    if isinstance(declared, str) and declared.strip():
        return declared.strip()
    missing.append(f"{ecosystem}: {name}@{version}")
    return ""


# --- Rust --------------------------------------------------------------------------------------
packages = {package["id"]: package for package in metadata["packages"]}
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
members = set(metadata["workspace_members"])
reached = set(members)
queue = list(members)
while queue:
    for dependency in nodes[queue.pop()]["deps"]:
        # `kind` null is a normal dependency; "dev" and "build" never reach an artefact.
        if not any(kind["kind"] is None for kind in dependency["dep_kinds"]):
            continue
        if dependency["pkg"] not in reached:
            reached.add(dependency["pkg"])
            queue.append(dependency["pkg"])

rust = []
rust_not_shipped = []
for package_id, package in packages.items():
    if package.get("source") is None:
        continue  # a workspace member: rDownloader itself
    if package_id in reached:
        # A crate with only a `license-file` counts as undeclared: overrides.json records the
        # SPDX name of that file, so every entry reads the same way.
        rust.append({
            "name": package["name"],
            "version": package["version"],
            "license": licence("rust", package["name"], package["version"], package.get("license")),
        })
    else:
        rust_not_shipped.append(f"{package['name']}@{package['version']}")

# --- npm ---------------------------------------------------------------------------------------
lock = json.loads(pathlib.Path("web/package-lock.json").read_text())
npm = {}
for key, entry in lock["packages"].items():
    if "node_modules/" not in key:
        continue  # the root project
    if entry.get("dev") or entry.get("devOptional") or entry.get("link"):
        continue
    name = entry.get("name") or key.rsplit("node_modules/", 1)[1]
    version = entry["version"]
    npm[(name, version)] = {
        "name": name,
        "version": version,
        "license": licence("npm", name, version, entry.get("license")),
    }

if missing:
    print("!! these packages declare no licence; record each in", file=sys.stderr)
    print("   crates/rd-api/licenses/overrides.json with the place it was read:", file=sys.stderr)
    for entry in missing:
        print(f"   {entry}", file=sys.stderr)
    sys.exit(1)


def rows(entries):
    return ",\n".join("    " + json.dumps(entry, ensure_ascii=False) for entry in entries)


by_name = lambda entry: (entry["name"], entry["version"])
rust.sort(key=by_name)
text = (
    "{\n"
    f'  "rust": [\n{rows(rust)}\n  ],\n'
    f'  "rust_not_shipped": [\n{rows(sorted(set(rust_not_shipped)))}\n  ],\n'
    f'  "npm": [\n{rows(sorted(npm.values(), key=by_name))}\n  ]\n'
    "}\n"
)

current = target.read_text() if target.exists() else ""
if check_only:
    if current != text:
        print(f"!! {target} is out of date; run scripts/licenses.sh", file=sys.stderr)
        sys.exit(1)
    print(f"==> {target} is current")
elif current == text:
    print(f"==> {target} was already current")
else:
    target.write_text(text)
    print(f"==> wrote {target}: {len(rust)} crates ({len(set(rust_not_shipped))} more not shipped), "
          f"{len(npm)} npm packages")
PY
