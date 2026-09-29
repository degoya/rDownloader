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
# Usage:
#   scripts/set-version.sh            # print the current version
#   scripts/set-version.sh --check    # every copy agrees with Cargo.toml, or exit 1 naming it
#   scripts/set-version.sh 0.9.3      # set it everywhere and update Cargo.lock
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

current() {
    # Anchored to the [workspace.package] section: several other sections have a `version` key.
    sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
        | sed -n 's/^version = "\(.*\)"/\1/p' | head -1
}

if [[ $# -eq 0 ]]; then
    current
    exit 0
fi

# Every copy, as "file<TAB>version it reports". The manifest drops a pre-release suffix.
copies() {
    python3 - <<'PY'
import json
for path, read in (
    ('web/package.json', lambda d: d['version']),
    ('extension/manifest.base.json', lambda d: d['version']),
    ('web/openapi.json', lambda d: d['info']['version']),
):
    with open(path) as handle:
        print(f'{path}\t{read(json.load(handle))}')
PY
}

if [[ "$1" == "--check" ]]; then
    want="$(current)"
    bad=0
    while IFS=$'\t' read -r path found; do
        expected="$want"
        [[ "$path" == extension/manifest.base.json ]] && expected="${want%%[-+]*}"
        if [[ "$found" != "$expected" ]]; then
            echo "!! $path reports $found, Cargo.toml $want — run scripts/set-version.sh $want" >&2
            bad=1
        fi
    done < <(copies)
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

python3 - "$version" <<'PY'
import json, pathlib, re, sys

version = sys.argv[1]

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
cargo.write_text(updated)

package = pathlib.Path('web/package.json')
data = json.loads(package.read_text())
data['version'] = version
# json.dumps drops the trailing newline npm writes; keep the file as npm would leave it.
package.write_text(json.dumps(data, indent=2) + '\n')

manifest = pathlib.Path('extension/manifest.base.json')
base = json.loads(manifest.read_text())
base['version'] = re.sub(r'[-+].*$', '', version)
manifest.write_text(json.dumps(base, indent=2) + '\n')

# The generated contract: only info.version, in place, so the rest stays byte for byte.
contract = pathlib.Path('web/openapi.json')
text = contract.read_text()
doc = json.loads(text)
updated, count = re.subn(
    # info.version is the last key of "info" (utoipa's order), at four spaces, before "paths".
    r'(\n    "version": ")[^"]+("\n  \},\n  "paths")',
    lambda match: f'{match.group(1)}{version}{match.group(2)}',
    text,
    count=1,
)
if count != 1 or json.loads(updated)['info']['version'] != version:
    raise SystemExit('could not rewrite info.version in web/openapi.json')
contract.write_text(updated)
PY

# Workspace members carry `version.workspace = true`, so only the lock needs rewriting.
cargo update --workspace --offline > /dev/null 2>&1

after="$(current)"
[[ "$after" == "$version" ]] || { echo "Cargo.toml still reports $after" >&2; exit 1; }
web_version="$(python3 -c 'import json;print(json.load(open("web/package.json"))["version"])')"
[[ "$web_version" == "$version" ]] || { echo "web/package.json reports $web_version" >&2; exit 1; }
manifest_version="$(python3 -c 'import json;print(json.load(open("extension/manifest.base.json"))["version"])')"
[[ "$manifest_version" == "${version%%[-+]*}" ]] \
    || { echo "extension/manifest.base.json reports $manifest_version" >&2; exit 1; }

"$0" --check > /dev/null || exit 1
echo "==> Cargo.toml, web/package.json, extension/manifest.base.json, web/openapi.json and Cargo.lock now report $version"
echo "    remember: CHANGELOG.md and docs/roadmap.md are written by hand"
