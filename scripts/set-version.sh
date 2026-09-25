#!/usr/bin/env bash
#
# Reads or sets the release version in the three places that carry it.
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
# Usage:
#   scripts/set-version.sh            # print the current version
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

echo "==> Cargo.toml, web/package.json, extension/manifest.base.json and Cargo.lock now report $version"
echo "    remember: CHANGELOG.md and docs/roadmap.md are written by hand"
