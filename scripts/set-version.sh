#!/usr/bin/env bash
#
# Reads, sets or checks the release version. There is one source: `[workspace.package] version` in
# Cargo.toml. Every other place that carries it is a copy this script writes, and `--check`
# fails when one of them disagrees (run by scripts/check.sh on every branch and by CI), so a
# copy cannot lag behind again — web/openapi.json said 1.4.2 through 1.5.0 and 1.5.1 (2026-09-28).
#
# The workspace version and web/package.json have to agree: the OpenAPI document reports the
# Cargo version, and a plugin's `min_app_version` is checked against it at packaging time, so a
# mismatch shows up as a confusing packaging failure rather than as a version problem.
#
# extension/manifest.base.json is the third: the browser extension is a release artifact of its
# own, and a store refuses an update whose version is not greater than the published one. It sat
# on 0.1.0 through every release until RD-109-17. A pre-release suffix is dropped on the way in,
# because a browser manifest version is dot-separated integers only.
#
# web/openapi.json is the fourth: it is generated (scripts/api-contract.sh), and its `info.version`
# is the Cargo version at generation time. The bump writes that one field the way the generator
# would, so the release does not need a compile to regenerate the document.
#
# The SDK's two workflows, sdk/ci/plugin.yml and sdk/ci/repository.yml, are the fifth and sixth:
# their `RDOWNLOADER_VERSION` is the release a third party's CI downloads to package, check and
# index with. It sat on 1.5.2, which knows plugin API 0.9.0 only, while every template declared
# 0.10.0, so a copied workflow was red from its first run (audit K2). The SDK is exported with
# the release, so the pin is the version being released; scripts/check-sdk-templates.sh holds
# the templates' `api_version` to what this version accepts.
#
# The SDK templates' `min_app_version` (sdk/templates/*/manifest.toml) are the last: a scaffold
# needs the release its SDK came with, the one whose contract its `wit/` copy is. A fixed value
# could not be right on both sides of a contract step -- 1.25.0 for contract 0.11.0 is newer than
# the tree that introduces it, so CI's scaffold failed its own packaging check against 1.24.0
# (RD-1250-01) -- while the tree's own version is always a core that accepts the copy.
#
# Usage:
#   scripts/set-version.sh            # print the current version
#   scripts/set-version.sh --check    # every copy agrees with Cargo.toml, or exit 1 naming it
#   scripts/set-version.sh 0.9.3      # set it everywhere and update Cargo.lock
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# shellcheck source=lib/workspace-version.sh
source "$ROOT/scripts/lib/workspace-version.sh"
current() { rd_workspace_version < Cargo.toml; }

if [[ $# -eq 0 ]]; then
    current
    exit 0
fi

# Every copy, as "file<TAB>version it reports", for each entry of RD_VERSION_COPIES
# (lib/workspace-version.sh) — the list the bump writes and lib/verified.sh reads. The manifest
# drops a pre-release suffix. Python on Windows ends its lines with CRLF; the CR would make every
# copy disagree.
copies() {
    python3 - "${RD_VERSION_COPIES[@]}" <<'PY' | tr -d '\r'
import fnmatch, glob, json, re, sys

def single(pattern, what):
    def read(text):
        found = re.findall(pattern, text, re.MULTILINE)
        return found[0] if len(found) == 1 else f'no single {what}'
    return read

READERS = {
    'web/package.json': lambda text: json.loads(text)['version'],
    'extension/manifest.base.json': lambda text: json.loads(text)['version'],
    'web/openapi.json': lambda text: json.loads(text)['info']['version'],
    'sdk/ci/*.yml': single(r'^  RDOWNLOADER_VERSION: (\S+)$', 'RDOWNLOADER_VERSION'),
    'sdk/templates/*/manifest.toml': single(r'^min_app_version = "([^"]*)"$', 'min_app_version'),
}
for entry in sys.argv[1:]:
    # Python under Git Bash on Windows globs `sdk/templates\auth\manifest.toml`; the rules
    # name paths with `/`, so the entry and every match are compared that way.
    entry = entry.replace('\\', '/')
    paths = sorted(glob.glob(entry)) if any(c in entry for c in "*?[") else [entry]
    for path in (match.replace('\\', '/') for match in paths):
        read = next((r for p, r in READERS.items() if fnmatch.fnmatchcase(path, p)), None)
        if read is None:
            raise SystemExit(f'set-version.sh has no rule for the copy {path}')
        with open(path) as handle:
            print(f'{path}\t{read(handle.read())}')
PY
}

if [[ "$1" == "--check" ]]; then
    want="$(current)"
    bad=0
    # Read whole first: a copy that cannot be read fails the check instead of dropping out of it.
    reported="$(copies)" || exit 1
    while IFS=$'\t' read -r path found; do
        expected="$want"
        [[ "$path" == extension/manifest.base.json ]] && expected="${want%%[-+]*}"
        if [[ "$found" != "$expected" ]]; then
            echo "!! $path reports $found, Cargo.toml $want — run scripts/set-version.sh $want" >&2
            bad=1
        fi
    done <<< "$reported"
    [[ "$bad" -eq 0 ]] && echo "    every copy reports $want"
    exit "$bad"
fi

version="$1"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
    echo "not a semantic version: $version" >&2
    exit 2
fi

before="$(current)"
if [[ -z "$before" ]]; then
    echo "could not read the current version from Cargo.toml" >&2
    exit 1
fi
echo "==> $before -> $version"

python3 - "$version" "${RD_VERSION_COPIES[@]}" <<'PY'
import fnmatch, glob, json, pathlib, re, sys

version = sys.argv[1]

# A file whose content does not change is not written (RD-1130-01): a bump to the version the tree
# already carries, as a re-started release chain does, moved every mtime, and
# scripts/web-dist-stale.sh then took web/package.json for a source newer than web/dist.
def write(path, text):
    if path.read_text() != text:
        path.write_text(text)

cargo = pathlib.Path('Cargo.toml')
text = cargo.read_text()
# Replace only inside [workspace.package], and only the first `version` key in it.
updated, count = re.subn(
    r'(\[workspace\.package\]\n(?:[^\[]*?\n)?version = ")[^"]+(")',
    lambda match: f'{match.group(1)}{version}{match.group(2)}',
    text,
    count=1,
)
if count != 1:
    raise SystemExit('could not rewrite the workspace version in Cargo.toml')
write(cargo, updated)

# A JSON file's top-level `version`; json.dumps drops the trailing newline npm writes, so it is
# kept the way npm would leave the file.
def json_version(value):
    def rewrite(path):
        data = json.loads(path.read_text())
        data['version'] = value
        write(path, json.dumps(data, indent=2) + '\n')
    return rewrite

# The generated contract: only info.version, in place, so the rest stays byte for byte.
def contract(path):
    text = path.read_text()
    updated, count = re.subn(
        # info.version is the last key of "info" (utoipa's order), at four spaces, before "paths".
        r'(\n    "version": ")[^"]+("\n  \},\n  "paths")',
        lambda match: f'{match.group(1)}{version}{match.group(2)}',
        text,
        count=1,
    )
    if count != 1 or json.loads(updated)['info']['version'] != version:
        raise SystemExit(f'could not rewrite info.version in {path}')
    write(path, updated)

# One line of the form `pattern`, whose first group is kept and whose version follows it.
def single_line(pattern, suffix, what):
    def rewrite(path):
        updated, count = re.subn(
            pattern,
            lambda match: f'{match.group(1)}{version}{suffix}',
            path.read_text(),
            flags=re.MULTILINE,
        )
        if count != 1:
            raise SystemExit(f'could not rewrite {what} in {path}')
        write(path, updated)
    return rewrite

WRITERS = {
    'web/package.json': json_version(version),
    # A browser manifest version is dot-separated integers only.
    'extension/manifest.base.json': json_version(re.sub(r'[-+].*$', '', version)),
    'web/openapi.json': contract,
    # The SDK workflows' release pin: one `RDOWNLOADER_VERSION:` under `env:` each.
    'sdk/ci/*.yml': single_line(r'^(  RDOWNLOADER_VERSION: )\S+$', '', 'RDOWNLOADER_VERSION'),
    # The SDK templates' `min_app_version`: one under `[metadata]` each.
    'sdk/templates/*/manifest.toml': single_line(r'^(min_app_version = ")[^"]*"$', '"', 'min_app_version'),
}
for entry in sys.argv[2:]:
    # Python under Git Bash on Windows globs `sdk/templates\auth\manifest.toml`; the rules
    # name paths with `/`, so the entry and every match are compared that way.
    entry = entry.replace('\\', '/')
    paths = sorted(glob.glob(entry)) if any(c in entry for c in "*?[") else [entry]
    for path in (match.replace('\\', '/') for match in paths):
        rewrite = next((r for p, r in WRITERS.items() if fnmatch.fnmatchcase(path, p)), None)
        if rewrite is None:
            raise SystemExit(f'set-version.sh has no rule for the copy {path}')
        rewrite(pathlib.Path(path))
PY

# Workspace members carry `version.workspace = true`, so only the lock needs rewriting.
cargo update --workspace --offline > /dev/null 2>&1

after="$(current)"
[[ "$after" == "$version" ]] || { echo "Cargo.toml still reports $after" >&2; exit 1; }
web_version="$(python3 -c 'import json;print(json.load(open("web/package.json"))["version"])' | tr -d '\r')"
[[ "$web_version" == "$version" ]] || { echo "web/package.json reports $web_version" >&2; exit 1; }
manifest_version="$(python3 -c 'import json;print(json.load(open("extension/manifest.base.json"))["version"])' | tr -d '\r')"
[[ "$manifest_version" == "${version%%[-+]*}" ]] \
    || { echo "extension/manifest.base.json reports $manifest_version" >&2; exit 1; }

"$0" --check > /dev/null || exit 1
echo "==> Cargo.toml, web/package.json, extension/manifest.base.json, web/openapi.json, sdk/ci/*.yml, the SDK templates and Cargo.lock now report $version"
echo "    remember: CHANGELOG.md and docs/roadmap.md are written by hand"
