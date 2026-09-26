#!/usr/bin/env bash
#
# scripts/i18n-key.sh against scratch catalogues (RD-140-22): a key reaches all four languages
# in the tree's format, an escaped dot stays inside one key, and the refusals — an existing
# leaf, a group inside a flat group, a string in the way — are loud.
#
#   scripts/tests/i18n-key.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export RD_LOCALES_DIR="$SCRATCH/locales"
for language in de en es fr; do
    mkdir -p "$RD_LOCALES_DIR/$language"
    cat > "$RD_LOCALES_DIR/$language/server.json" <<EOF
{
  "title": "$language",
  "codes": {
    "auth.invalid": "x"
  }
}
EOF
done
key() { run_status "$ROOT/scripts/i18n-key.sh" "$@"; }
value() { python3 -c 'import json,sys
node = json.load(open(sys.argv[1]))
for part in sys.argv[2:]:
    node = node[part]
print(node)' "$RD_LOCALES_DIR/$1/server.json" "${@:2}"; }

key server actions.enable Aktivieren Enable Activar Activer
expect_status "a new key in a new group" 0
expect "de" "Aktivieren" "$(value de actions enable)"
expect "en" "Enable" "$(value en actions enable)"
expect "es" "Activar" "$(value es actions enable)"
expect "fr" "Activer" "$(value fr actions enable)"
expect "the tree's format: two spaces, order kept, a trailing newline" \
    '{|  "title": "en",|  "codes": {|    "auth.invalid": "x"|  },|  "actions": {|    "enable": "Enable"|  }|}|' \
    "$(tr '\n' '|' < "$RD_LOCALES_DIR/en/server.json")"

key server 'codes.collector\.check_no_resolver' Kein None Ninguno Aucun
expect_status "an escaped dot writes one literal key" 0
expect "inside the flat group" "None" "$(value en codes collector.check_no_resolver)"

key server codes.collector.other a b c d
expect_status "a group inside a flat group is refused" 1
expect_output "with the escaped form to use instead" "codes.collector\\.other"

key server actions.enable A B C D
expect_status "an existing leaf is refused" 1
expect_output "naming it" "actions.enable already exists"
expect "and the value stays" "Enable" "$(value en actions enable)"

key server title.sub a b c d
expect_status "a string where a group would go is refused" 1

key server actions..x a b c d
expect_status "an empty segment is refused" 1

key server only.two a b
expect_status "four translations or nothing" 2

key nosuch a.b a b c d
expect_status "a catalogue that does not exist is refused" 1

finish_tests i18n-key
