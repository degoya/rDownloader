#!/usr/bin/env bash
#
# Every action a workflow uses is pinned to a commit, not to a tag or a branch (1.8): a tag can be
# moved to other code by whoever controls the action's repository, and the next run executes it
# with the workflow's token. The release the commit belongs to stays readable as a trailing
# comment (`uses: owner/repo@<40 hex> # v2.3.4`), which Dependabot updates together with the pin.
#
# Local actions (`./…`) are part of this tree and pass; a `docker://` image passes only with a
# `@sha256:` digest. Comment lines are not read. Reads files only; check.sh runs it every time.
#
# Usage:
#   scripts/check-actions-pinned.sh                 # .github/workflows/ and .github/actions/
#   scripts/check-actions-pinned.sh <file.yml>...   # these files
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

files=("$@")
if [[ ${#files[@]} -eq 0 ]]; then
    cd "$ROOT"
    mapfile -t files < <(git ls-files '.github/workflows/*.yml' '.github/workflows/*.yaml' \
        '.github/actions/*/action.yml' '.github/actions/*/action.yaml')
fi
[[ ${#files[@]} -gt 0 ]] || { echo "no workflow files to check" >&2; exit 2; }

unpinned=0
uses=0
for file in "${files[@]}"; do
    line_no=0
    while IFS= read -r line || [[ -n "$line" ]]; do
        line_no=$((line_no + 1))
        [[ "$line" =~ ^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*[\"\']?([^\"\'[:space:]]+) ]] || continue
        ref="${BASH_REMATCH[2]}"
        uses=$((uses + 1))
        case "$ref" in
            ./*) continue ;;
            docker://*@sha256:*) [[ "$ref" =~ @sha256:[0-9a-f]{64}$ ]] && continue ;;
            *@*) [[ "${ref##*@}" =~ ^[0-9a-f]{40}$ ]] && continue ;;
        esac
        echo "$file:$line_no: $ref is not pinned to a commit"
        unpinned=$((unpinned + 1))
    done < "$file"
done

if [[ "$unpinned" -gt 0 ]]; then
    echo "!! $unpinned of $uses action reference(s) not pinned: uses: owner/repo@<40-hex commit> # vX.Y.Z" >&2
    exit 1
fi
echo "    $uses action reference(s) in ${#files[@]} file(s), every one pinned or local"
