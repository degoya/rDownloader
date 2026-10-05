#!/usr/bin/env bash
#
# scripts/i18n-key.sh against scratch catalogues (RD-140-22): a key reaches all four languages
# in the tree's format, an escaped dot stays inside one key, and the refusals — an existing
# leaf, a group inside a flat group, a string in the way — are loud. Named languages
# (RD-1100-09): any order, every required one, an in-progress one optional, an unknown refused.
#
#   scripts/tests/i18n-key.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export RD_LOCALES_DIR="$SCRATCH/locales"
# The tree's four required languages and one unfinished one, whose directory holds no catalogue yet.
export RD_LANGUAGES="$SCRATCH/languages.json"
cat > "$RD_LANGUAGES" <<'EOF'
{
  "en": { "name": "English", "status": "required" },
  "de": { "name": "Deutsch", "status": "required" },
  "fr": { "name": "Français", "status": "required" },
  "es": { "name": "Español", "status": "required" },
  "it": { "name": "Italiano", "status": "in-progress" }
}
EOF
mkdir -p "$RD_LOCALES_DIR/it"
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

key server named.order fr=Ouvrir en=Open es=Abrir de=Öffnen
expect_status "named languages in any order" 0
expect "de by name" "Öffnen" "$(value de named order)"
expect "fr by name" "Ouvrir" "$(value fr named order)"
expect "an in-progress language left out gets no catalogue" "no" \
    "$([[ -e "$RD_LOCALES_DIR/it/server.json" ]] && echo yes || echo no)"

key server named.close de=Schließen en=Close es=Cerrar fr=Fermer it=Chiudi
expect_status "an in-progress language named starts its catalogue" 0
expect "it" "Chiudi" "$(value it named close)"

key server named.missing de=a en=b es=c
expect_status "a required language left out is refused" 2
expect_output "naming it" "missing: fr"
expect "and nothing is written" "no" \
    "$(python3 -c 'import json,sys; print("yes" if "missing" in json.load(open(sys.argv[1]))["named"] else "no")' "$RD_LOCALES_DIR/en/server.json")"

key server named.unknown de=a en=b es=c fr=d xx=e
expect_status "a language the list does not know is refused" 2
expect_output "naming it" "xx is not in the language list"

key server named.twice de=a en=b es=c fr=d de=e
expect_status "a language named twice is refused" 2

key server named.mixed de=a b c d
expect_status "named and positional mixed is refused" 2

key nosuch a.b a b c d
expect_status "a catalogue that does not exist is refused" 1

finish_tests i18n-key
