#!/usr/bin/env bash
#
# The portable Unix launchers twice, once per system (RD-191-09): scripts/linux/*.sh and
# scripts/macos/*.command are the same scripts but for the lines listed below, so a fix to one is
# a fix to the other or this test fails. Each listed difference is applied to the macOS file as a
# substitution; what is left must equal the Linux file byte for byte. A new difference is a new
# substitution here, written down instead of drifting in.
#
# Pure bash. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/launcher-parity.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# macOS text -> Linux text, one sed expression per known difference.
DIFFERENCES=(
    # The launchers name each other by their own extension.
    -e 's/\.command\b/.sh/g'
    # The browser: `open` on macOS, xdg-open when installed on Linux.
    -e 's|^\( *\)open "\${base}" >/dev/null 2>&1 \|\| true$|\1command -v xdg-open >/dev/null 2>\&1 \&\& xdg-open "${base}" >/dev/null 2>\&1 \&|'
    # Who starts the capture-only launcher, and how.
    -e 's/^# Starts only the capture agent, for a Mac whose/# Starts only the capture agent, for a machine whose/'
    -e 's/this file exists to be double-clicked\./this file exists to be started on its own./'
)

for linux in "$ROOT"/scripts/linux/*.sh; do
    name="$(basename "$linux" .sh)"
    macos="$ROOT/scripts/macos/$name.command"
    if [[ ! -f "$macos" ]]; then
        expect "$name has a macOS twin" "present" "missing"
        continue
    fi
    difference="$(diff <(sed "${DIFFERENCES[@]}" "$macos") "$linux" || true)"
    expect "$name: macOS and Linux differ only in the listed lines" "" "$difference"
done
for macos in "$ROOT"/scripts/macos/*.command; do
    name="$(basename "$macos" .command)"
    [[ -f "$ROOT/scripts/linux/$name.sh" ]] || expect "$name has a Linux twin" "present" "missing"
done

finish_tests "launcher-parity"
