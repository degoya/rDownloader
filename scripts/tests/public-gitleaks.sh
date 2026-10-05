#!/usr/bin/env bash
#
# rd_public_find_gitleaks (scripts/lib/public.sh), the scanner the public and wiki exports trust
# (audit K7): `$GITLEAKS` wins, else `gitleaks` on PATH, else the export stops; a GITLEAKS that
# is no executable stops it too, and no other place is looked in — the function names no fixed
# path, as it once did with a copy under /tmp.
#
# Pure bash with stub binaries: it runs in well under a second.
#
#   scripts/tests/public-gitleaks.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="$ROOT/scripts/lib/public.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

mkdir -p "$SCRATCH/empty" "$SCRATCH/on-path" "$SCRATCH/named"
printf '#!/bin/sh\nexit 0\n' > "$SCRATCH/on-path/gitleaks"
printf '#!/bin/sh\nexit 0\n' > "$SCRATCH/named/gitleaks"
chmod +x "$SCRATCH/on-path/gitleaks" "$SCRATCH/named/gitleaks"

# Runs the lookup in a clean environment with PATH $1 and, when given, GITLEAKS $2.
find_gitleaks() {
    local -a environment=(PATH="$1")
    [[ $# -gt 1 ]] && environment+=(GITLEAKS="$2")
    run_status env -i "${environment[@]}" "$BASH" -c \
        'source "$1"; rd_public_find_gitleaks; echo "found $GITLEAKS_BIN"' _ "$LIB"
}

find_gitleaks "$SCRATCH/empty"
expect_status "neither GITLEAKS nor PATH: the export stops" 1
expect_output "and says how to provide it" "gitleaks not found: put it on PATH or name it with GITLEAKS=<path>."

find_gitleaks "$SCRATCH/on-path"
expect_status "gitleaks on PATH: found" 0
expect_output "the one on PATH" "found $SCRATCH/on-path/gitleaks"

find_gitleaks "$SCRATCH/on-path" "$SCRATCH/named/gitleaks"
expect_status "GITLEAKS named: found" 0
expect_output "GITLEAKS before PATH" "found $SCRATCH/named/gitleaks"

find_gitleaks "$SCRATCH/on-path" "$SCRATCH/named/missing"
expect_status "GITLEAKS naming nothing: the export stops, PATH is not asked" 1
expect_output "and the name is given" "GITLEAKS=$SCRATCH/named/missing is not an executable file."

# shellcheck source=../lib/public.sh
source "$LIB"
expect_true "no fixed path is looked in" '! declare -f rd_public_find_gitleaks | grep -qE "/tmp|/home|/opt"'

finish_tests public-gitleaks
