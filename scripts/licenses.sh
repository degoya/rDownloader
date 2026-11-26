#!/usr/bin/env bash
#
# Regenerates crates/rd-api/licenses/third-party.json, the dependency licences the About page
# lists (RD-130-12), and THIRD-PARTY-NOTICES.txt beside it, their licence and copyright texts in
# full, which every package carries and the About page offers (RD-1480-12).
#
# Generated, never kept by hand. The list, from two sources:
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
#     not install and the current list does not record yet — so it needs `pnpm install --dir web
#     --frozen-lockfile` first, and the network for those only, each named when it is asked.
# A package that declares no licence at all takes its entry from
# crates/rd-api/licenses/overrides.json, which names where the licence was read; without one
# this script refuses to write the list. It refuses as well when a licence is outside deny.toml's
# `allow` (scripts/lib/licence_policy.py).
#
# The texts (scripts/lib/third_party_notices.py), never from the network:
#   * a crate's licence, copyright and NOTICE files from its sources in cargo's registry, an npm
#     package's from web/node_modules, the Rust standard library's from the toolchain's own
#     COPYRIGHT-library.html (component `rust-docs`), and overrides.json `texts` for a package that
#     carries a project's work without its licence text.
#   * a package without a licence file gets the standard text of what it declares, from
#     crates/rd-api/licenses/texts/, and no copyright line nobody wrote. So do the platform
#     packages of the build tools, which are installed on one platform each and ship nowhere.
#
# The tests in `about::tests` (rd-api's library) keep both honest and name this script: every
# crate of Cargo.lock and every production package of pnpm-lock.yaml is listed, every listed
# entry has a licence and a source, and the texts name every entry. None is run here.
#
# Usage:
#   scripts/licenses.sh            # rewrite both files
#   scripts/licenses.sh --check    # fail if it would change anything
#
# Needs no build lock: `cargo metadata` and `cargo tree` compile nothing. They may download the
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
import concurrent.futures
import json
import pathlib
import re
import subprocess
import sys
import urllib.parse
import urllib.request

sys.dont_write_bytecode = True
sys.path.insert(0, "scripts/lib")
from licence_policy import NPM_ONLY, allowed_licences, is_allowed
from third_party_notices import Component, read_text, render, rust_standard_library

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text())
check_only = sys.argv[2] == "1"
licences = pathlib.Path("crates/rd-api/licenses")
target = licences / "third-party.json"
notices = licences / "THIRD-PARTY-NOTICES.txt"
overrides = json.loads((licences / "overrides.json").read_text())
recorded = json.loads(target.read_text()) if target.exists() else {}
missing = []
refused = []
allowed = {"rust": allowed_licences(), "npm": allowed_licences() | NPM_ONLY}


def licence(ecosystem, name, version, declared):
    override = overrides.get(ecosystem, {}).get(f"{name}@{version}")
    if override:
        declared = override["license"]
    if isinstance(declared, str) and declared.strip():
        if not is_allowed(declared.strip(), allowed[ecosystem]):
            refused.append(f"{ecosystem}: {name}@{version}: {declared.strip()}")
        return declared.strip()
    missing.append(f"{ecosystem}: {name}@{version}")
    return ""


def people(names):
    """The names alone: an address a manifest carries is no part of a copyright notice."""
    plain = (re.sub(r"\s*[<(].*", "", name).strip() for name in names)
    return tuple(filter(None, plain))


def noted(ecosystem, name):
    """The texts overrides.json adds to a package, as (label, text)."""
    note = overrides.get("texts", {}).get(ecosystem, {}).get(name)
    return ((note["label"], read_text(licences / "texts" / note["file"])),) if note else ()


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
crates = []
for key, package in registry.items():
    if key in shipped:
        # A crate with only a `license-file` counts as undeclared: overrides.json records the
        # SPDX name of that file, so every entry reads the same way.
        rust.append({
            "name": package["name"],
            "version": package["version"],
            "license": licence("rust", package["name"], package["version"], package.get("license")),
            # The version's own page, which offers exactly these sources (MPL-2.0 section 3.2).
            "source": f"https://crates.io/crates/{package['name']}/{package['version']}",
        })
        directory = pathlib.Path(package["manifest_path"]).parent
        named = package.get("license_file")
        crates.append(Component(
            **rust[-1], directory=directory, authors=people(package.get("authors") or ()),
            files=(directory / named,) if named and (directory / named).is_file() else (),
            texts=noted("rust", package["name"]),
        ))
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
# `plain` is the walk without any `optionalDependencies`: what it misses are the platform packages
# of the build tools (native binaries, wasm fallbacks), of which each machine installs its own.
roots, plain_roots, edges, plain, optional_peers = [], [], {}, {}, {}
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
            edges[entry], plain[entry] = [], []
    elif indent == 4:
        group = text.rstrip(":")
    elif section == "importers" and group in ("dependencies", "optionalDependencies"):
        if indent == 6:
            dependency = unquote(text.rstrip(":"))
        elif indent == 8 and text.startswith("version: "):
            key = snapshot(dependency, text.removeprefix("version: "))
            if key:
                roots.append(key)
                if group == "dependencies":
                    plain_roots.append(key)
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
            if group == "dependencies":
                plain[entry].append(key)


def reached(start, graph):
    seen, queue = set(), list(start)
    while queue:
        key = queue.pop()
        if key not in seen:
            seen.add(key)
            queue.extend(graph[key])
    return seen


production = reached(roots, edges)
everywhere = {without_peers(key).rpartition("@")[::2] for key in reached(plain_roots, plain)}

modules = pathlib.Path("web/node_modules")
if not (modules / ".modules.yaml").is_file():
    print("!! web/node_modules is no pnpm install; run: pnpm install --dir web --frozen-lockfile",
          file=sys.stderr)
    sys.exit(1)
# Every installed package once. The layout is the flat one web/pnpm-workspace.yaml asks for,
# as npm's was: a package in node_modules/, a second version in node_modules/<dependent>/node_modules/.
declared, installed = {}, {}
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
            installed[(package["name"], package["version"])] = (child, package)
        if (child / "node_modules").is_dir():
            directories.append(child / "node_modules")


def from_registry(key):
    name, version = key
    print(f"    asking registry.npmjs.org for the licence of {name}@{version}", file=sys.stderr)
    url = f"https://registry.npmjs.org/{urllib.parse.quote(name, safe='@')}/{version}"
    with urllib.request.urlopen(url, timeout=30) as response:
        return key, json.load(response).get("license")


locked = {without_peers(key).rpartition("@")[::2] for key in production}
# A published version never changes its licence, so what the list records is not asked again.
for entry in recorded.get("npm", ()):
    declared.setdefault((entry["name"], entry["version"]), entry["license"])
with concurrent.futures.ThreadPoolExecutor(8) as pool:
    declared.update(pool.map(from_registry, sorted(locked - declared.keys())))

npm = {}
interface, platform_packages = [], []
for name, version in locked:
    npm[(name, version)] = {
        "name": name,
        "version": version,
        "license": licence("npm", name, version, declared[(name, version)]),
        "source": f"https://www.npmjs.com/package/{name}/v/{version}",
    }
    if (name, version) not in everywhere:
        platform_packages.append(Component(**npm[(name, version)]))
        continue
    if (name, version) not in installed:
        print(f"!! {name}@{version} is not in web/node_modules; run: "
              "pnpm install --dir web --frozen-lockfile", file=sys.stderr)
        sys.exit(1)
    directory, package = installed[(name, version)]
    author = package.get("author")
    author = author.get("name", "") if isinstance(author, dict) else author or ""
    interface.append(Component(
        **npm[(name, version)], directory=directory, authors=people([author]),
        texts=noted("npm", name),
    ))

if missing:
    print("!! these packages declare no licence; record each in", file=sys.stderr)
    print("   crates/rd-api/licenses/overrides.json with the place it was read:", file=sys.stderr)
    for entry in missing:
        print(f"   {entry}", file=sys.stderr)
    sys.exit(1)
if refused:
    print("!! these licences are outside deny.toml's `allow` (scripts/lib/licence_policy.py):",
          file=sys.stderr)
    for entry in refused:
        print(f"   {entry}", file=sys.stderr)
    sys.exit(1)

# --- Neither crate nor npm package ---------------------------------------------------------------
toolchain = re.search(r'^channel = "([^"]+)"', pathlib.Path("rust-toolchain.toml").read_text(), re.M)[1]
sysroot = subprocess.run(["rustc", f"+{toolchain}", "--print", "sysroot"], check=True,
                         stdout=subprocess.PIPE, text=True).stdout.strip()
copyright_page = pathlib.Path(sysroot) / "share/doc/rust/COPYRIGHT-library.html"
if not copyright_page.is_file():
    print(f"!! {copyright_page} is missing; run: rustup component add rust-docs --toolchain {toolchain}",
          file=sys.stderr)
    sys.exit(1)
standard_library = rust_standard_library(copyright_page.read_text(), toolchain, licences / "texts")
other = [{field: getattr(standard_library[0], field)
          for field in ("name", "version", "license", "source")}]

def rows(entries):
    return ",\n".join("    " + json.dumps(entry, ensure_ascii=False) for entry in entries)


by_name = lambda entry: (entry["name"], entry["version"])
rust.sort(key=by_name)
text = (
    "{\n"
    f'  "rust": [\n{rows(rust)}\n  ],\n'
    f'  "rust_not_shipped": [\n{rows(sorted(set(rust_not_shipped)))}\n  ],\n'
    f'  "npm": [\n{rows(sorted(npm.values(), key=by_name))}\n  ],\n'
    f'  "other": [\n{rows(other)}\n  ]\n'
    "}\n"
)
try:
    texts = render(
        (licences / "texts" / "preamble.txt").read_text(),
        [
            ("Rust crates", "The crates the service, the capture agent and the other programs link "
             "on Linux, Windows and macOS, and the crates the plugins link.", crates),
            ("The Rust standard library", "What rustc links into every program and plugin, and the "
             "crates it is built from.", standard_library),
            ("npm packages of the web interface", "Everything the interface's dependencies reach, "
             "its fonts and icons included. The interface itself contains the part of them it "
             "uses.", interface),
            ("Platform packages of the interface's build tools", "Native programs the interface is "
             "built with, one per platform. They run on the build machine and are part of no "
             "package; they are listed because the interface's lock file names them.",
             platform_packages),
        ],
        licences / "texts",
    )
except LookupError as error:
    print(f"!! crates/rd-api/licenses/texts has no standard text of {', '.join(error.args[0])}",
          file=sys.stderr)
    sys.exit(1)

stale = False
for path, wanted in ((target, text), (notices, texts)):
    current = path.read_bytes().decode() if path.exists() else ""
    if current == wanted:
        print(f"==> {path} is current")
    elif check_only:
        print(f"!! {path} is out of date; run scripts/licenses.sh", file=sys.stderr)
        stale = True
    else:
        path.write_bytes(wanted.encode())
        print(f"==> wrote {path}")
print(f"    {len(rust)} crates ({len(set(rust_not_shipped))} more not shipped), {len(npm)} npm packages")
sys.exit(1 if stale else 0)
PY
