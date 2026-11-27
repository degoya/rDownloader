#!/usr/bin/env bash
#
# scripts/release-sbom.sh (RD-1480-14, LIC-5): over this checkout, whose SBOM has to pass its own
# floors, and over a scratch checkout with one package of each kind — what ships, what does not,
# the licences that are no SPDX expression, the checksums — and over one that lost its content,
# which is refused. release.yml's `publish` runs it before the checksums.
#
#   scripts/tests/release-sbom.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
export PYTHONDONTWRITEBYTECODE=1

# The value of Python expression $2 over the SPDX document $1 (`d`, `p`: its packages by name). The
# expressions are this file's own literals, so `eval` runs nothing from outside.
of() {
    python3 -I -c '
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
p = {each["name"]: each for each in d["packages"]}
print(eval(sys.argv[2]))' "$1" "$2"
}

run_status "$ROOT/scripts/release-sbom.sh"
expect_status "without an output" 2
expect_output "it says its usage" "usage: scripts/release-sbom.sh <output>"
run_status "$ROOT/scripts/release-sbom.sh" --help me
expect_status "an option is refused" 2

real="$SCRATCH/real.spdx.json"
# A branch that moved a lock file has a stale list until the generators run (integrate.sh starts
# them after this test), and what moved is then missing here: a warning, where the release stops.
run_status env RD_SBOM_STALE_LIST=warn SOURCE_DATE_EPOCH=1764028800 "$ROOT/scripts/release-sbom.sh" "$real"
expect_status "this checkout's SBOM passes its floors" 0
if [[ "$output" != *"renews the list"* ]]; then
    expect "the crates the service is made of are in it" "True" \
        "$(of "$real" 'all(name in p for name in ("tokio", "axum", "sqlx", "wasmtime", "vue", "@nuxt/ui"))')"
fi
expect "every package has an identifier of its own" "True" \
    "$(of "$real" 'len({each["SPDXID"] for each in d["packages"]}) == len(d["packages"])')"
expect "every package but the application depends from it, once" "True" \
    "$(of "$real" 'sorted(r["relatedSpdxElement"] for r in d["relationships"] if r["relationshipType"] == "DEPENDS_ON") == sorted(each["SPDXID"] for each in d["packages"] if each["name"] != "rdownloader")')"
expect "every third-party package has its checksum" "True" \
    "$(of "$real" 'all("checksums" in each for each in d["packages"] if each["name"] != "rdownloader")')"
expect "the time is SOURCE_DATE_EPOCH" "2025-11-25T00:00:00Z" "$(of "$real" 'd["creationInfo"]["created"]')"
env RD_SBOM_STALE_LIST=warn SOURCE_DATE_EPOCH=1764028800 "$ROOT/scripts/release-sbom.sh" \
    "$SCRATCH/again.spdx.json" > /dev/null 2>&1
expect_true "the same checkout gives the same bytes" 'cmp -s "$real" "$SCRATCH/again.spdx.json"'

tree="$SCRATCH/tree"
mkdir -p "$tree/web" "$tree/crates/rd-api/licenses"
cat > "$tree/Cargo.toml" <<'EOF'
[workspace.package]
version = "9.8.7"
license = "GPL-3.0-or-later"
repository = "https://example.invalid/app"
EOF
cat > "$tree/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "rd-core"
version = "9.8.7"

[[package]]
name = "build-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "00"

[[package]]
name = "slashed"
version = "1.0.0+extra"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"

[[package]]
name = "unlisted"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bb"
EOF
cat > "$tree/web/pnpm-lock.yaml" <<'EOF'
---
packages:

  '@pnpm/exe@12.0.0':
    resolution: {integrity: sha512-AAAA}

---
importers:

  .:
    dependencies:
      vue:
        specifier: ^3
        version: 3.5.0

packages:

  '@scope/name@1.2.3':
    resolution: {integrity: sha512-//79}
    engines: {node: '>=10'}

  vue@3.5.0:
    resolution: {integrity: sha512-AQID}

snapshots:

  '@scope/name@1.2.3': {}
EOF
cat > "$tree/crates/rd-api/licenses/third-party.json" <<'EOF'
{
  "rust": [{"name": "slashed", "version": "1.0.0+extra", "license": "MIT/Apache-2.0"}],
  "rust_not_shipped": ["build-only@1.0.0"],
  "npm": [
    {"name": "@scope/name", "version": "1.2.3", "license": "LicenseRef-Own"},
    {"name": "gone", "version": "1.0.0", "license": "MIT"},
    {"name": "vue", "version": "3.5.0", "license": "see the file"}
  ]
}
EOF
small="$SCRATCH/small.spdx.json"
run_status env RD_SBOM_ROOT="$tree" SOURCE_DATE_EPOCH=0 "$ROOT/scripts/release-sbom.sh" "$small"
expect_status "a list that lost its content is refused" 1
expect_output "with the count it found" "the SBOM names 2 cargo packages, at least 500 expected"
expect_true "and nothing is written" '[[ ! -e "$small" ]]'

run_status python3 -I "$ROOT/scripts/lib/release-sbom.py" "$tree" "$small" 0 1 2
expect_status "a listed package that left the lock file stops the release" 1
expect_output "it is named as an error" "::error::gone@1.0.0 is listed but not in web/pnpm-lock.yaml"
expect_true "and nothing is written" '[[ ! -e "$small" ]]'
run_status env RD_SBOM_STALE_LIST=warn python3 -I "$ROOT/scripts/lib/release-sbom.py" "$tree" "$small" 0 1 3
expect_status "the npm half has a floor of its own" 1
expect_output "a preflight only warns about it" "release-sbom: gone@1.0.0 is listed but not in web/pnpm-lock.yaml"
run_status env RD_SBOM_STALE_LIST=warn python3 -I "$ROOT/scripts/lib/release-sbom.py" "$tree" "$small" 0 1 2
expect_status "the scratch checkout above its floors" 0
expect "what ships, and what the licence list does not know" \
    "['@scope/name', 'rdownloader', 'slashed', 'unlisted', 'vue']" "$(of "$small" 'sorted(p)')"
expect "a slash is OR, a reference without its text and prose are no expression" \
    "MIT OR Apache-2.0 NOASSERTION NOASSERTION NOASSERTION GPL-3.0-or-later" \
    "$(of "$small" '" ".join(p[name]["licenseDeclared"] for name in ("slashed", "unlisted", "@scope/name", "vue", "rdownloader"))')"
expect "the purls are escaped" "pkg:cargo/slashed@1.0.0%2Bextra pkg:npm/%40scope/name@1.2.3" \
    "$(of "$small" '" ".join(p[name]["externalRefs"][0]["referenceLocator"] for name in ("slashed", "@scope/name"))')"
expect "npm's integrity becomes a hexadecimal checksum" "SHA512 fffefd" \
    "$(of "$small" '"{algorithm} {checksumValue}".format(**p["@scope/name"]["checksums"][0])')"
expect "the document names the version it describes" "rdownloader-9.8.7 9.8.7" \
    "$(of "$small" 'd["name"] + " " + p["rdownloader"]["versionInfo"]')"

# The release runs it, strict, and before the checksums that cover it.
release_yml="$ROOT/.github/workflows/release.yml"
expect "release.yml never relaxes the stale list" "0" "$(grep -c RD_SBOM_STALE_LIST "$release_yml" || true)"
expect "release.yml writes it before SHA256SUMS" "release-sbom.sh release-assets.sh sums" \
    "$(grep -oE 'scripts/(release-sbom\.sh|release-assets\.sh sums)' "$release_yml" | sed 's#^scripts/##' | paste -sd' ')"

finish_tests "release-sbom"
