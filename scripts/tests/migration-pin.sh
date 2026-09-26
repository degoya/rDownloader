#!/usr/bin/env bash
#
# scripts/migration-pin.sh against a scratch tree (RD-140-22): it pins what has no pin, keeps
# the file sorted, never moves an existing pin, and refuses a migration whose bytes changed.
#
# Pure bash and coreutils, no cargo: check.sh runs it whenever something under scripts/ changes.
#
#   scripts/tests/migration-pin.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
MIGRATIONS="$TREE/crates/rd-db/migrations"
PINS="$TREE/crates/rd-db/migrations.sha384"
mkdir -p "$TREE/scripts" "$MIGRATIONS"
cp "$ROOT/scripts/migration-pin.sh" "$TREE/scripts/"
pin() { run_status "$TREE/scripts/migration-pin.sh" "$@"; }
sum() { sha384sum "$MIGRATIONS/$1" | cut -d' ' -f1; }

echo 'CREATE TABLE b (id INTEGER);' > "$MIGRATIONS/0002_b.sql"
echo 'CREATE TABLE a (id INTEGER);' > "$MIGRATIONS/0001_a.sql"

pin
expect_status "pins every migration without a pin" 0
expect_output "says how many" "2 new pin(s)"
expect "the pin file is plain sha384sum output, sorted by name" \
    "$(sum 0001_a.sql)  0001_a.sql"$'\n'"$(sum 0002_b.sql)  0002_b.sql" "$(cat "$PINS")"
expect "coreutils verify it" "0" "$(cd "$MIGRATIONS" && sha384sum --quiet -c ../migrations.sha384 > /dev/null 2>&1; echo $?)"

pin
expect_status "a second run is a no-op" 0
expect_output "and adds nothing" "0 new pin(s)"

echo 'CREATE TABLE c (id INTEGER);' > "$MIGRATIONS/0003_c.sql"
echo 'CREATE TABLE d (id INTEGER);' > "$MIGRATIONS/0004_d.sql"
pin crates/rd-db/migrations/0004_d.sql
expect_status "a named path pins exactly that one" 0
expect "0004 is pinned, 0003 is not" "0004_d.sql|" \
    "$(awk '{print $2}' "$PINS" | grep -E '000[34]' | tr '\n' '|')"
pin 0003_c.sql
expect "a bare name works too, and the order stays by number" "0001_a.sql 0002_b.sql 0003_c.sql 0004_d.sql" \
    "$(awk '{print $2}' "$PINS" | tr '\n' ' ' | sed 's/ $//')"

before="$(cat "$PINS")"
echo '-- a comment is a change too' >> "$MIGRATIONS/0002_b.sql"
pin
expect_status "a migration that differs from its pin is refused" 1
expect_output "by name" "0002_b.sql differs from its pin"
expect "and the pin is not moved" "$before" "$(cat "$PINS")"

pin 0099_missing.sql
expect_status "a migration that does not exist is an error" 1
expect_output "naming it" "no migration 0099_missing.sql"

finish_tests migration-pin
