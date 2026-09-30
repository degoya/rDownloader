#!/usr/bin/env bash
#
# scripts/update-schema-change.sh on a scratch repository (RD-180-02): a release is compared with
# the one before it on its channel — plain tags with plain tags, betas with betas — and says
# `true` only when crates/rd-db/migrations/ differs; what it cannot tell is `true`.
#
#   scripts/tests/update-schema-change.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRIPT="$ROOT/scripts/update-schema-change.sh"
REPO="$SCRATCH/repo"

git init --quiet "$REPO"
repo() { git -C "$REPO" -c user.name=test -c user.email=test@example.test "$@"; }
commit_tag() {
    repo add --all
    repo commit --quiet --allow-empty -m "$1"
    repo tag "$1"
}
mkdir -p "$REPO/crates/rd-db/migrations"
echo "create table a" > "$REPO/crates/rd-db/migrations/0001_a.sql"
commit_tag v1.0.0
echo "readme" > "$REPO/README.md"
commit_tag v1.0.1
echo "create table b" > "$REPO/crates/rd-db/migrations/0002_b.sql"
commit_tag v1.1.0-beta.1
echo "more readme" >> "$REPO/README.md"
commit_tag v1.1.0-beta.2
commit_tag v1.1.0
echo "create table c" > "$REPO/crates/rd-db/migrations/0003_c.sql"
commit_tag v1.2.0-beta.1

quiet() { "$SCRIPT" "$@" 2> /dev/null; }
expect "the first release has nothing to compare with" true "$(quiet v1.0.0 "$REPO")"
expect "a release without new migrations" false "$(quiet v1.0.1 "$REPO")"
expect "a release compared with the plain one before it, not its betas" true "$(quiet v1.1.0 "$REPO")"
expect "the first beta has no beta before it" true "$(quiet v1.1.0-beta.1 "$REPO")"
expect "a beta compared with the beta before it" false "$(quiet v1.1.0-beta.2 "$REPO")"
expect "a beta with a new migration since the last beta" true "$(quiet v1.2.0-beta.1 "$REPO")"
expect "a tag that is not there" true "$(quiet v9.9.9 "$REPO")"
run_status "$SCRIPT"
expect_status "no tag is a usage error" 2

finish_tests update-schema-change
