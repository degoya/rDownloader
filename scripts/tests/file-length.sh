#!/usr/bin/env bash
#
# At most 500 lines per file (owner, 2026-10-08, DOC-13; RD-1190-07), for every script of the
# repository: shell, Python and PowerShell, tracked or new, wherever it lives. A script over the
# limit is split into a library under scripts/lib/ that it sources or imports.
#
# BASELINE holds the scripts over the limit on 2026-10-08 with their counts then — none. A ratchet
# all the same: a listed script may shrink, never grow past its count, and leaves the list once it
# is back at the limit. The workflows have their limit in workflow-shape.sh, the Rust sources in
# crates/rdownloader/tests/repo_lints/file_length.rs, the web interface and the extension in
# web/src/fileLength.test.ts. Lines are counted as `wc -l` counts them.
#
# Pure bash over `git ls-files`: it runs in a second. check.sh runs it when scripts/ change, and
# under --full.
#
#   scripts/tests/file-length.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
LIMIT=500

declare -A BASELINE=()

# One line per finding for the files given as "<lines> <path>" on stdin; nothing when all hold.
findings() {
    local lines path
    local -A seen=()
    while read -r lines path; do
        seen["$path"]=1
        if [[ -z "${BASELINE[$path]:-}" ]]; then
            echo "$path: $lines lines; split it into a library under scripts/lib/"
        elif ((lines > BASELINE["$path"])); then
            echo "$path: $lines lines, grown past its ${BASELINE[$path]}"
        fi
    done
    for path in "${!BASELINE[@]}"; do
        [[ -n "${seen[$path]:-}" ]] || echo "$path: at or under $LIMIT lines, or gone; remove it from BASELINE"
    done
}

# "<lines> <path>" for every script over the limit.
over_limit() {
    local path lines
    while IFS= read -r -d '' path; do
        [[ -f "$ROOT/$path" && ! -L "$ROOT/$path" ]] || continue
        lines="$(wc -l < "$ROOT/$path")"
        ((lines > LIMIT)) && echo "$lines $path"
    done < <(git -C "$ROOT" ls-files -z -co --exclude-standard -- \
        '*.sh' '*.bash' '*.py' '*.ps1' '*.psm1' '*.command')
    return 0
}

scripts="$(git -C "$ROOT" ls-files -co --exclude-standard -- '*.sh' '*.py' | wc -l)"
expect "the scripts are found at all" "yes" "$( ((scripts > 100)) && echo yes)"
expect "every script holds at most $LIMIT lines, the baseline only shrinking" "" "$(over_limit | findings)"

# The guard itself, against a baseline of its own.
BASELINE=([scripts/a.sh]=600 [scripts/gone.sh]=700)
verdict="$(printf '%s\n' '501 scripts/new.sh' '610 scripts/a.sh' | findings | sort)"
expect "a new script over the limit is named" "yes" "$(grep -qF 'scripts/new.sh: 501 lines' <<< "$verdict" && echo yes)"
expect "a listed script that grew is named" "yes" "$(grep -qF 'scripts/a.sh: 610 lines, grown past its 600' <<< "$verdict" && echo yes)"
expect "a listed script back at the limit leaves the list" "yes" "$(grep -qF 'scripts/gone.sh: at or under' <<< "$verdict" && echo yes)"
expect "a listed script that shrank passes" "" "$(printf '%s\n' '590 scripts/a.sh' '650 scripts/gone.sh' | findings)"

finish_tests "file length"
