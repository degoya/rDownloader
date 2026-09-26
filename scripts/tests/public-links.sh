#!/usr/bin/env bash
#
# The link guard of the public exports (scripts/lib/public-links.py) against hand-written cases.
#
# Each case writes one file into an otherwise empty tree, runs the guard in repository or wiki
# mode against a small exclude list and compares the number of findings. The cases pin what
# must be refused — a Markdown, HTML or reference link into an excluded folder or file, from the
# root or from a subfolder, and a GitHub address of the repository in any text file — and what
# must not: a link inside a code fence, a path that only starts like an excluded one, a wiki
# page link, the wiki's own address.
#
# Pure python3 and bash: it runs in a second. check.sh runs it when scripts/lib/ or
# scripts/tests/ change, and under --full.
#
#   scripts/tests/public-links.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/lib/public-links.py"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

cat > "$SCRATCH/exclude.txt" <<'LIST'
# The shape of scripts/public-exclude.txt: comments, a folder, a file.
docs/
AGENTS.md   # a trailing comment
LIST

failures=0
passed=0
FENCE='```'
# Built from pieces, so that this file does not link into docs/ itself: it is exported too.
REPO='https://github.com/degoya/rDownloader'
RAW='https://raw.githubusercontent.com/degoya/rDownloader'

# check <name> <repo|wiki> <file> <expected findings> <content>
check() {
    local name="$1" mode="$2" file="$3" expected="$4" content="$5" found output
    rm -rf "$SCRATCH/tree"
    mkdir -p "$SCRATCH/tree/$(dirname "$file")"
    printf '%s\n' "$content" > "$SCRATCH/tree/$file"
    local args=("$SCRATCH/tree" "$SCRATCH/exclude.txt")
    [[ "$mode" == wiki ]] && args+=(--wiki)
    output="$(python3 "$GUARD" "${args[@]}")" && status=0 || status=$?
    found="$(grep -c . <<< "$output" || true)"
    if [[ "$found" -eq "$expected" && ( ( "$expected" -eq 0 && "$status" -eq 0 ) || ( "$expected" -gt 0 && "$status" -eq 1 ) ) ]]; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: expected $expected finding(s), got $found (exit $status)"
        [[ -z "$output" ]] || printf '     %s\n' "$output"
        failures=$((failures + 1))
    fi
}

check "no links"                         repo README.md 0 "Plain text naming docs/plugins.md in prose."
check "markdown link into a folder"      repo README.md 1 "See [the plugins](docs/plugins.md#manifest)."
check "html image into a folder"         repo README.md 1 '<img src="docs/images/a.png" alt="">'
check "html anchor"                      repo README.md 1 '<a href="docs/README.md">Documentation</a>'
check "link from a subfolder"            repo sdk/README.md 1 "Read [this](../docs/plugins.md)."
check "root-relative link"               repo sdk/README.md 1 "Read [this](/docs/plugins.md)."
check "reference definition"             repo README.md 1 "[plugins]: docs/plugins.md"
check "excluded file"                    repo CONTRIBUTING.md 1 "The [card](AGENTS.md)."
check "wrapped link text"                repo README.md 1 "The [plugin
reference](docs/plugins.md) says so."
check "two links on one line"            repo README.md 2 "[a](docs/a.md) and [b](docs/b.md)"
check "inside a code fence"              repo README.md 0 "$FENCE
[a](docs/a.md)
$FENCE"
check "a path that only starts alike"    repo README.md 0 "[a](docs-public/a.md) and [b](AGENTS.md.bak)"
check "a public path"                    repo README.md 0 "[s](sdk/README.md) and [c](CHANGELOG.md)"
check "outside the tree"                 repo README.md 0 "[w](../other/docs/a.md)"
check "github address in yaml"           repo .github/ISSUE_TEMPLATE/x.yml 1 "see [p]($REPO/blob/main/docs/plugins.md) now"
check "github tree address"              repo README.md 1 "<$REPO/tree/v1.3.0/docs/adr>"
check "raw address"                      repo src/lib.rs 1 "// $RAW/main/docs/a.md"
check "public github address"            repo README.md 0 "[s]($REPO/blob/main/sdk/README.md)"
check "the wiki's address"               repo README.md 0 "[w]($REPO/wiki/plugin-reference#remote-jobs)"
check "wiki: a page link"                wiki plugins/a.md 0 "[Overview](overview) and [x](docs/y)"
check "wiki: a repository address"       wiki plugins/a.md 1 "[plugins.md]($REPO/blob/main/docs/plugins.md)"
check "wiki: a public address"           wiki plugins/a.md 0 "[sdk]($REPO/tree/main/sdk)"

echo
echo "$passed passed, $failures failed"
[[ "$failures" -eq 0 ]]
