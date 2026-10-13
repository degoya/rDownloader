#!/usr/bin/env bash
#
# At most 500 lines per file (owner, 2026-10-08, DOC-13; RD-1190-07), for every script of the
# repository: shell, Python and PowerShell, tracked or new, wherever it lives. A script over the
# limit is split into a library under scripts/lib/ that it sources or imports.
#
# No script was over the limit on 2026-10-08, so the limit is hard: no baseline, no exceptions
# (RD-1280-22 removed the empty list's logic). The workflows have their limit in
# workflow-shape.sh, the Rust sources in crates/rdownloader/tests/repo_lints/file_length.rs, the
# web interface and the extension in web/src/fileLength.test.ts. Lines are counted as `wc -l`
# counts them.
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

# One line per finding for the files given as "<lines> <path>" on stdin; nothing when all hold.
findings() {
    local lines path
    while read -r lines path; do
        echo "$path: $lines lines; split it into a library under scripts/lib/"
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
expect "every script holds at most $LIMIT lines" "" "$(over_limit | findings)"

# The guard itself: every script over the limit is named, each on its own line.
verdict="$(printf '%s\n' '501 scripts/new.sh' '610 scripts/a.sh' | findings)"
expect "a script over the limit is named" "yes" "$(grep -qF 'scripts/new.sh: 501 lines' <<< "$verdict" && echo yes)"
expect "and every other one with it" "2" "$(wc -l <<< "$verdict" | tr -d ' ')"

finish_tests "file length"
