#!/usr/bin/env bash
#
# Is the built frontend in web/dist still the one the sources describe?
#
# Everything used to ask `[[ -f web/dist/index.html ]]` — existence, not freshness. So a stale
# bundle from three days ago counted as present and got embedded into the binary, while
# api-contract.sh, which needs web/dist only because rust-embed reads it at compile time, forced
# a full `npm run build` whenever it happened to be absent. Both answers were wrong in
# different directions.
#
# Same `find -newer` shape build-plugins.sh uses for plugin components, and it costs the same:
# a handful of stat calls.
#
# Exit status is the answer, so it reads as a condition:
#
#   scripts/web-dist-stale.sh || npm run build --prefix web
#
#   0  web/dist is up to date
#   1  web/dist is missing or older than a source that goes into it
#
# The verdict is also printed, in those words, so a run's log says which it was. --quiet
# suppresses it.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

quiet=0
for argument in "$@"; do
    case "$argument" in
        --quiet) quiet=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

say() { [[ "$quiet" -eq 1 ]] || echo "$*"; }

if [[ ! -f web/dist/index.html ]]; then
    say "web/dist is stale: web/dist/index.html does not exist"
    exit 1
fi

# Everything vite reads. A path that does not exist is dropped rather than making find fail —
# web/openapi.json is generated and tsconfig is a glob.
sources=()
for path in web/src web/public web/index.html web/package.json web/package-lock.json \
            web/vite.config.ts web/openapi.json web/tsconfig*.json; do
    [[ -e "$path" ]] && sources+=("$path")
done

newer="$(find "${sources[@]}" -newer web/dist/index.html -print -quit)"
if [[ -n "$newer" ]]; then
    say "web/dist is stale: $newer is newer than web/dist/index.html"
    exit 1
fi

say "web/dist is fresh"
exit 0
