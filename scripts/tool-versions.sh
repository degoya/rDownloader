#!/usr/bin/env bash
#
# The development tools' pins, one source each (RD-1280-12): Rust in rust-toolchain.toml,
# wasm-tools in scripts/lib/plugin-stamp.sh, nextest, sqlx-cli, cargo-xwin, shellcheck and
# actionlint in scripts/lib/tool-versions.sh. Scripts read those; a workflow step and a document
# cannot, so `--check` holds every copy to its source, as `scripts/set-version.sh --check` does
# for the application's version. Before it, the same version stood in up to four places with
# nothing comparing them.
#
# A copy is a line one of the rules below matches. A rule that matches nothing fails as well:
# the copy moved, and the rule has to follow it. Documents are read where present (the public
# export has no docs/).
#
# Usage:
#   scripts/tool-versions.sh                 # every tool and its version
#   scripts/tool-versions.sh --check         # every copy agrees with its source, or exit 1
#   scripts/tool-versions.sh --check --root DIR
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() { echo "usage: scripts/tool-versions.sh [--check [--root DIR]]" >&2; exit 2; }

check=0
tree="$ROOT"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --check) check=1 ;;
        --root) tree="${2:?--root needs a directory}"; shift ;;
        -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
        *) usage ;;
    esac
    shift
done

# The sources, read from the tree under check.
# shellcheck source=lib/tool-versions.sh
source "$tree/scripts/lib/tool-versions.sh"
declare -A pin=(
    [rust]="$(rd_rust_version "$tree")"
    [wasm-tools]="$(rd_wasm_tools_pin "$tree")"
    [nextest]="$RD_NEXTEST_VERSION"
    [sqlx-cli]="$RD_SQLX_CLI_VERSION"
    [cargo-xwin]="$RD_CARGO_XWIN_VERSION"
    [shellcheck]="$RD_SHELLCHECK_VERSION"
    [actionlint]="$RD_ACTIONLINT_VERSION"
)
order=(rust wasm-tools nextest sqlx-cli cargo-xwin shellcheck actionlint)

if [[ "$check" -eq 0 ]]; then
    for tool in "${order[@]}"; do printf '%-11s %s\n' "$tool" "${pin[$tool]}"; done
    exit 0
fi

V='([0-9]+\.[0-9]+\.[0-9]+)'
DOCS=(AGENTS.md docs/development.md docs/architecture.md docs/reproducible-builds.md sdk/README.md)
copies=0
wrong=0

# The copies of tool $1 that extended regex $3 (the version as its last group) matches in files
# $4...: each must say the tool's pin. $2 is what it takes when none matches: `required` fails,
# `docs` fails while docs/ exists, `any` passes.
copies_of() {
    local tool="$1" need="$2" regex="$3" expected="${pin[$1]}" file hit value found=0 groups
    shift 3
    groups="${regex//[^(]/}"
    if [[ -z "$expected" ]]; then
        echo "!! no ${tool} version at its source" >&2
        wrong=$((wrong + 1))
        return
    fi
    for file in "$@"; do
        [[ -f "$file" ]] || continue
        while IFS= read -r hit; do
            found=$((found + 1))
            value="$(sed -E "s/.*${regex}.*/\\${#groups}/" <<< "${hit#*:}")"
            if [[ "$value" != "$expected" ]]; then
                echo "!! ${file}:${hit%%:*}: ${tool} ${value}, the source says ${expected}" >&2
                wrong=$((wrong + 1))
            fi
        done < <(grep -nE -- "$regex" "$file" || true)
    done
    copies=$((copies + found))
    if [[ "$found" -eq 0 ]] && [[ "$need" == required || ( "$need" == docs && -d docs ) ]]; then
        echo "!! no copy of ${tool} matches /${regex}/: the copy moved, and its rule in scripts/tool-versions.sh follows it" >&2
        wrong=$((wrong + 1))
    fi
}

cd "$tree"
shopt -s nullglob
WORKFLOWS=(.github/workflows/*.yml .github/actions/*/action.yml)
copies_of rust required "components-$V-wt" "${WORKFLOWS[@]}"
copies_of rust docs "^\\| Rust \\| $V" docs/development.md
copies_of rust required "^FROM rust:$V-" docker/Dockerfile
copies_of rust any "(rust:|RUSTUP_TOOLCHAIN=)$V" "${DOCS[@]}"
copies_of wasm-tools required "tool: wasm-tools@$V" "${WORKFLOWS[@]}" sdk/ci/*.yml
copies_of wasm-tools required "components-[0-9.]+-wt$V-" "${WORKFLOWS[@]}"
copies_of wasm-tools docs "^\\| wasm-tools \\| $V" docs/development.md
copies_of wasm-tools any "wasm-tools(\`| --version)? $V" "${DOCS[@]}"
copies_of nextest required "tool: nextest@$V" "${WORKFLOWS[@]}"
copies_of nextest docs "^\\| cargo-nextest \\| $V" docs/development.md
copies_of nextest any "cargo install cargo-nextest --version $V" "${DOCS[@]}"
copies_of sqlx-cli required "tool: sqlx-cli@$V" "${WORKFLOWS[@]}"
copies_of sqlx-cli docs "^\\| sqlx-cli \\| $V" docs/development.md
copies_of sqlx-cli any "(cargo install sqlx-cli --version|\`sqlx-cli\`) $V" "${DOCS[@]}"
copies_of cargo-xwin docs "^\\| cargo-xwin \\| $V" docs/development.md
copies_of cargo-xwin any "(cargo install cargo-xwin --version|\`cargo-xwin\`) $V" "${DOCS[@]}"
copies_of shellcheck docs "^\\| shellcheck \\| $V" docs/development.md
copies_of shellcheck any "shellcheck-py==$V" "${DOCS[@]}"
copies_of actionlint docs "^\\| actionlint \\| $V" docs/development.md

if [[ "$wrong" -gt 0 ]]; then
    echo "!! ${wrong} tool pin(s) disagree with their source (scripts/lib/tool-versions.sh, rust-toolchain.toml, scripts/lib/plugin-stamp.sh)" >&2
    exit 1
fi
echo "    ${copies} copies of ${#order[@]} tool pins agree with their sources"
