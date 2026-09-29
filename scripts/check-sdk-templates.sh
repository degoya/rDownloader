#!/usr/bin/env bash
#
# Every world of the plugin contract has an SDK template, and every template belongs to a world
# (RD-160-04). For each `world <name>-plugin` in crates/rd-plugin-api/wit/rdownloader.wit,
# sdk/templates/<name>/ has to exist and carry:
#
#   - Cargo.toml building that very world (`world = "<name>-plugin"`),
#   - manifest.toml with a `plugin_type`,
#   - wit/rdownloader.wit, byte for byte the contract,
#   - README.md, pointing its reader at the handbook,
#   - at least one `#[test]` under src/, so `cargo test` in a fresh scaffold proves something.
#
# A template directory no world names is refused too: `plugin new --type` would offer a
# scaffold for a world the contract no longer has. Whether the template *compiles* against the
# contract is the CI scaffold step's question (.github/workflows/ci.yml, `components` job);
# whether `plugin new` accepts every name is `crates/rd-pack/src/plugin.rs`'s unit test.
#
# Pure bash, no build. Exit 1 on the first run of findings, each named.
#
#   scripts/check-sdk-templates.sh [repo]
#
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
WIT="$ROOT/crates/rd-plugin-api/wit/rdownloader.wit"
TEMPLATES="$ROOT/sdk/templates"

findings=0
finding() { echo "!! $1" >&2; findings=$((findings + 1)); }

worlds=()
while read -r world; do
    worlds+=("$world")
done < <(sed -n 's/^world \([a-z0-9-]*\)-plugin {$/\1/p' "$WIT")
[[ "${#worlds[@]}" -gt 0 ]] || { echo "!! no world found in $WIT" >&2; exit 1; }

for world in "${worlds[@]}"; do
    dir="$TEMPLATES/$world"
    if [[ ! -d "$dir" ]]; then
        finding "world ${world}-plugin has no template at sdk/templates/${world}"
        continue
    fi
    grep -qx "world = \"${world}-plugin\"" "$dir/Cargo.toml" 2> /dev/null \
        || finding "sdk/templates/${world}/Cargo.toml does not build world ${world}-plugin"
    grep -q '^plugin_type = "' "$dir/manifest.toml" 2> /dev/null \
        || finding "sdk/templates/${world}/manifest.toml declares no plugin_type"
    cmp -s "$WIT" "$dir/wit/rdownloader.wit" \
        || finding "sdk/templates/${world}/wit/rdownloader.wit is not the current contract"
    [[ -f "$dir/README.md" ]] || finding "sdk/templates/${world} has no README.md"
    grep -rqF '#[test]' "$dir/src" 2> /dev/null \
        || finding "sdk/templates/${world} has no unit test under src/"
done

for dir in "$TEMPLATES"/*/; do
    name="$(basename "$dir")"
    known=0
    for world in "${worlds[@]}"; do
        [[ "$world" == "$name" ]] && known=1
    done
    [[ "$known" -eq 1 ]] || finding "sdk/templates/${name} belongs to no world of the contract"
done

if [[ "$findings" -gt 0 ]]; then
    echo "!! ${findings} finding(s) in the SDK templates" >&2
    exit 1
fi
echo "==> ${#worlds[@]} worlds, ${#worlds[@]} templates, each complete"
