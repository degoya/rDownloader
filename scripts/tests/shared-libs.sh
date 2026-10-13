#!/usr/bin/env bash
#
# The helpers RD-1280-15 moved into scripts/lib/ from the scripts that each kept a copy:
# rd_render_template fills every `@NAME@` and refuses a template that keeps one, writing nothing;
# rd_bundle_version reads the workspace version, not the first `version` of Cargo.toml, and
# drops a pre-release; `pub` runs git in the clone rd_public_prepare_clone prepared, and refuses
# before it.
#
#   scripts/tests/shared-libs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/render-template.sh
source "$ROOT/scripts/lib/render-template.sh"
# shellcheck source=../lib/macos-helper.sh
source "$ROOT/scripts/lib/macos-helper.sh"
# shellcheck source=../lib/public.sh
source "$ROOT/scripts/lib/public.sh"

printf 'url @BASE_URL@/x, key @KEY@, kept @@ and a=b=c: @PAIR@\n' > "$SCRATCH/t.in"
run_status rd_render_template "$SCRATCH/t.in" "$SCRATCH/t" BASE_URL=https://example.invalid KEY=AB PAIR=a=b
expect_status "a template with every value renders" 0
expect "every placeholder filled, a value keeps its own =" \
    "url https://example.invalid/x, key AB, kept @@ and a=b=c: a=b" "$(< "$SCRATCH/t")"

run_status rd_render_template "$SCRATCH/t.in" "$SCRATCH/u" BASE_URL=https://example.invalid
expect_status "a placeholder without a value fails" 1
expect_output "naming it" "keeps @KEY@ after rendering"
expect_true "and writes nothing" '[[ ! -e "$SCRATCH/u" ]]'

cargo_toml() {
    printf '[package]\nversion = "0.1.0"\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n' "$1"
}
expect "a release's bundle version is the workspace version" "1.28.0" "$(cargo_toml 1.28.0 | rd_bundle_version)"
expect "a beta's drops the pre-release" "1.8.0" "$(cargo_toml 1.8.0-beta.2 | rd_bundle_version)"
expect "this checkout's has three numbers" "yes" \
    "$([[ "$(rd_bundle_version < "$ROOT/Cargo.toml")" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && echo yes || echo no)"

run_status pub status
expect_status "pub before a prepared clone refuses" 1
expect_output "saying what comes first" "rd_public_prepare_clone first"
git init --quiet --bare "$SCRATCH/remote.git"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
# Not through run_status: its subshell would keep the clone it names to itself.
rd_public_prepare_clone "$SCRATCH/clone" "$SCRATCH/remote.git" main > /dev/null 2>&1
expect "pub runs git in that clone" "refs/heads/main" "$(pub symbolic-ref HEAD)"

finish_tests "shared-libs"
