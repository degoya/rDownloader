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
# And the versions a template states have to fit together (audit K2, K4):
#
#   - `wit-bindgen` is the workspace's own requirement, the one the bundled plugins build with;
#     two templates sat on 0.61 beside eleven on 0.62.
#   - `api_version` is one this core accepts (`SUPPORTED_API_VERSIONS`, rd-plugin-host).
#   - `min_app_version` is the first release that accepts that `api_version`, from the table
#     below; a contract version without a row is a finding naming the row to add. Templates said
#     0.7.0, 0.9.0 and 1.0.7 beside an `api_version` only 1.9.0 accepts.
#   - The SDK workflows (sdk/ci/*.yml) pin `RDOWNLOADER_VERSION` to the workspace version, the
#     release this tree becomes and the SDK is exported with (scripts/set-version.sh writes it),
#     and that version is not older than `min_app_version`: the pinned release accepts what the
#     templates declare. The pin said 1.5.2, which knows API 0.9.0 only.
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

# shellcheck source=lib/workspace-version.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/workspace-version.sh"

# The first release that accepts a plugin `api_version`. A contract bump adds its row.
api_first_release() {
    case "$1" in
        0.10.0) echo 1.9.0 ;;
    esac
}

# Whether X.Y.Z $1 is not newer than version $2. A pre-release of $1's own version is older
# than $1, as semantic versioning and the core's `min_app_version` check have it.
version_at_most() {
    local core="${2%%[-+]*}"
    [[ "$1" == "$core" && "$2" != "$core" ]] && return 1
    [[ "$(printf '%s\n%s\n' "$1" "$core" | sort -V | head -n 1)" == "$1" ]]
}

# The value of `key = "…"` in TOML file $2, the first one.
toml_value() { sed -n "s/^$1 = \"\(.*\)\"\$/\1/p" "$2" 2> /dev/null | head -n 1; }

workspace_version="$(rd_workspace_version < "$ROOT/Cargo.toml")"
bindgen="$(sed -n '/^\[workspace\.dependencies\]/,/^\[/p' "$ROOT/Cargo.toml" | sed -n 's/^wit-bindgen = "\(.*\)"$/\1/p')"
[[ -n "$workspace_version" && -n "$bindgen" ]] \
    || { echo "!! no workspace version or wit-bindgen requirement in $ROOT/Cargo.toml" >&2; exit 1; }
supported="$(sed -n 's/^pub const SUPPORTED_API_VERSIONS: &\[&str\] = &\[\(.*\)\];$/\1/p' \
    "$ROOT/crates/rd-plugin-host/src/manifest.rs" | tr -d '" ' | tr ',' ' ')"
[[ -n "$supported" ]] || { echo "!! no SUPPORTED_API_VERSIONS in crates/rd-plugin-host/src/manifest.rs" >&2; exit 1; }
newest_minimum=""

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

    found="$(toml_value wit-bindgen "$dir/Cargo.toml")"
    [[ "$found" == "$bindgen" ]] \
        || finding "sdk/templates/${world}/Cargo.toml asks for wit-bindgen ${found:-nothing}, the workspace for $bindgen"
    api="$(toml_value api_version "$dir/manifest.toml")"
    [[ " $supported " == *" $api "* ]] \
        || finding "sdk/templates/${world}/manifest.toml declares api_version ${api:-nothing}, this core accepts $supported"
    first="$(api_first_release "$api")"
    minimum="$(toml_value min_app_version "$dir/manifest.toml")"
    if [[ -z "$first" ]]; then
        finding "api_version ${api:-nothing} has no row in api_first_release (scripts/check-sdk-templates.sh): add the release that first accepts it"
    elif [[ "$minimum" != "$first" ]]; then
        finding "sdk/templates/${world}/manifest.toml says min_app_version ${minimum:-nothing}, api_version $api needs $first"
    elif [[ -z "$newest_minimum" ]] || ! version_at_most "$minimum" "$newest_minimum"; then
        newest_minimum="$minimum"
    fi
done

for workflow in "$ROOT"/sdk/ci/*.yml; do
    name="sdk/ci/$(basename "$workflow")"
    pin="$(sed -n 's/^  RDOWNLOADER_VERSION: //p' "$workflow")"
    if [[ "$pin" != "$workspace_version" ]]; then
        finding "$name pins rDownloader ${pin:-nothing}, the workspace is $workspace_version (scripts/set-version.sh $workspace_version)"
    elif [[ -n "$newest_minimum" ]] && ! version_at_most "$newest_minimum" "$pin"; then
        finding "$name pins rDownloader $pin, older than the templates' min_app_version $newest_minimum"
    fi
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
