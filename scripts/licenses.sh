#!/usr/bin/env bash
#
# Regenerates crates/rd-api/licenses/third-party.json, the dependency licences the About page
# lists (RD-130-12).
#
# Generated, never kept by hand, from two sources:
#   * `cargo tree`: everything a shipped artefact contains — the crates the service, the capture
#     agent and the other host crates link on each platform a package is built for, and the
#     crates the plugin components link on wasm32-unknown-unknown. Features are resolved as a
#     build resolves them and a `cfg()` is followed along the whole path, one target at a time,
#     and the union of the targets is listed; proc macros, build scripts and tests run on the
#     build machine and ship nowhere. Every other crate of Cargo.lock (from `cargo metadata`,
#     which also has the declared licences) is listed by name as not shipped, so the test below
#     can tell a crate new to Cargo.lock from one this list forgot.
#   * web/pnpm-lock.yaml: every package the production dependencies of web/package.json reach,
#     optional ones for other platforms included. Its licence is read from the installed
#     package.json in web/node_modules, or from the registry for a package this platform does
#     not install — so it needs `pnpm install --dir web --frozen-lockfile` first, and the
#     network for those.
# A package that declares no licence at all takes its entry from
# crates/rd-api/licenses/overrides.json, which names where the licence was read; without one
# this script refuses to write the list.
#
# Three tests in `about::tests` (rd-api's library) keep it honest and name this script: every
# crate of Cargo.lock and every production package of pnpm-lock.yaml is listed, and every
# listed entry has a licence. Neither is run here.
#
# Usage:
#   scripts/licenses.sh            # rewrite the list
#   scripts/licenses.sh --check    # fail if it would change anything
#
# Needs no build lock: `cargo metadata` and `cargo tree` compile nothing. They may download the
# manifests of crates for other platforms the first time, and the script asks the npm registry
# for the manifests of the packages for other platforms every time.
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
import concurrent.futures
import json
import pathlib
import subprocess
import sys
import urllib.parse
import urllib.request

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
# The targets a package or an image is built for; the plugins are one component for all of them.
HOST_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
)
PLUGIN_TARGET = "wasm32-unknown-unknown"

packages = {package["id"]: package for package in metadata["packages"]}
registry = {
    f"{package['name']}@{package['version']}": package
    for package in packages.values()
    if package.get("source") is not None  # no source: a workspace member, rDownloader itself
}
members = [packages[member] for member in metadata["workspace_members"]]
plugins = [member["id"] for member in members if "/plugins/" in member["manifest_path"]]
hosts = [member["id"] for member in members if member["id"] not in plugins]


def linked(target, member_ids):
    """`name@version` of every crate the members link when built together for `target`."""
    command = ["cargo", "tree", "--locked", "--target", target, "--edges", "normal,no-proc-macro",
               "--prefix", "none", "--format", "{p}"]
    for member_id in member_ids:
        command += ["--package", member_id]
    output = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True).stdout
    for line in output.splitlines():
        fields = line.split()
        if len(fields) >= 2:
            yield f"{fields[0]}@{fields[1].removeprefix('v')}"


shipped = set()
for host_target in HOST_TARGETS:
    shipped.update(linked(host_target, hosts))
shipped.update(linked(PLUGIN_TARGET, plugins))

rust = []
rust_not_shipped = []
for key, package in registry.items():
    if key in shipped:
        # A crate with only a `license-file` counts as undeclared: overrides.json records the
        # SPDX name of that file, so every entry reads the same way.
        rust.append({
            "name": package["name"],
            "version": package["version"],
            "license": licence("rust", package["name"], package["version"], package.get("license")),
        })
    else:
        rust_not_shipped.append(key)

# --- npm ---------------------------------------------------------------------------------------
# The same walk as `locked_npm_packages` in about.rs: from the importer's `dependencies` and
# `optionalDependencies` through each snapshot's own. A snapshot lists the peers pnpm resolved
# for it among its dependencies; an optional peer is skipped, because it resolves only to a
# package something else installs — a development one, if nothing in production reaches it.
# That is npm's `devOptional`, which the list never held. A bundled dependency has no entry of
# its own in the lockfile, so it is not listed; the only ones are inside
# @tailwindcss/oxide-wasm32-wasi, which no build of ours installs.
def unquote(text):
    return text[1:-1] if text[:1] == "'" and text[-1:] == "'" else text


def snapshot(name, value):
    """The snapshot key a dependency entry points at: `name@version(peers)`, or `alias: real@version`."""
    if value.startswith("link:"):
        return None
    return f"{name}@{value}" if value[:1].isdigit() else value


def without_peers(key):
    return key.split("(", 1)[0]


# pnpm writes the lockfile as two YAML documents, pnpm's own install first, the project second;
# only the project's importer has `dependencies`, so the documents need no telling apart.
roots, edges, optional_peers = [], {}, {}
section = group = entry = dependency = None
for line in pathlib.Path("web/pnpm-lock.yaml").read_text().splitlines():
    text = line.strip()
    if not text or text.startswith("#"):
        continue
    indent = len(line) - len(line.lstrip(" "))
    if indent == 0:
        section = text.rstrip(":")
    elif indent == 2:
        entry = unquote(text.removesuffix(" {}").rstrip(":"))
        if section == "snapshots":
            edges[entry] = []
    elif indent == 4:
        group = text.rstrip(":")
    elif section == "importers" and group in ("dependencies", "optionalDependencies"):
        if indent == 6:
            dependency = unquote(text.rstrip(":"))
        elif indent == 8 and text.startswith("version: "):
            key = snapshot(dependency, text.removeprefix("version: "))
            if key:
                roots.append(key)
    elif section == "packages" and group == "peerDependenciesMeta":
        if indent == 6:
            dependency = unquote(text.rstrip(":"))
        elif indent == 8 and text == "optional: true":
            optional_peers.setdefault(entry, set()).add(dependency)
    elif section == "snapshots" and group in ("dependencies", "optionalDependencies") and indent == 6:
        name, _, value = text.partition(": ")
        name = unquote(name)
        key = snapshot(name, value)
        if key and name not in optional_peers.get(without_peers(entry), ()):
            edges[entry].append(key)

production = set()
queue = list(roots)
while queue:
    key = queue.pop()
    if key not in production:
        production.add(key)
        queue.extend(edges[key])

modules = pathlib.Path("web/node_modules")
if not (modules / ".modules.yaml").is_file():
    print("!! web/node_modules is no pnpm install; run: pnpm install --dir web --frozen-lockfile",
          file=sys.stderr)
    sys.exit(1)
# Every installed package once. The layout is the flat one web/pnpm-workspace.yaml asks for,
# as npm's was: a package in node_modules/, a second version in node_modules/<dependent>/node_modules/.
declared = {}
directories = [modules]
while directories:
    directory = directories.pop()
    for child in directory.iterdir():
        if child.name.startswith(".") or child.is_symlink() or not child.is_dir():
            continue
        if child.name.startswith("@"):
            directories.append(child)
            continue
        manifest = child / "package.json"
        if manifest.is_file():
            package = json.loads(manifest.read_text())
            declared[(package["name"], package["version"])] = package.get("license")
        if (child / "node_modules").is_dir():
            directories.append(child / "node_modules")


def from_registry(key):
    name, version = key
    url = f"https://registry.npmjs.org/{urllib.parse.quote(name, safe='@')}/{version}"
    with urllib.request.urlopen(url, timeout=30) as response:
        return key, json.load(response).get("license")


locked = {without_peers(key).rpartition("@")[::2] for key in production}
with concurrent.futures.ThreadPoolExecutor(8) as pool:
    declared.update(pool.map(from_registry, sorted(locked - declared.keys())))

npm = {}
for name, version in locked:
    npm[(name, version)] = {
        "name": name,
        "version": version,
        "license": licence("npm", name, version, declared[(name, version)]),
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
