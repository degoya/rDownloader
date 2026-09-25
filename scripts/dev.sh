#!/usr/bin/env bash
#
# Runs the service locally against a scratch database, the way AGENTS.md describes.
#
# Usage:
#   scripts/dev.sh                 # serve on 127.0.0.1:8710
#   scripts/dev.sh --fresh         # start from an empty database
#   scripts/dev.sh --unsigned      # allow unsigned plugins (local development only)
#   PORT=8711 scripts/dev.sh
#   HOST=0.0.0.0 scripts/dev.sh    # reachable from outside this machine (dev only)
#
set -euo pipefail

PORT="${PORT:-8710}"
# Loopback by default. Override only when the host's browser cannot reach WSL's localhost;
# 0.0.0.0 opens the service to the network, so it is a per-session choice, not a habit.
HOST="${HOST:-127.0.0.1}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
cd "$ROOT"

database="data/rdownloader.sqlite3"
extra=()
for argument in "$@"; do
    case "$argument" in
        --fresh) rm -f "$database"* ;;
        --unsigned) extra+=(--plugin-development-mode) ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

# Never in a feature worktree: web/node_modules and web/dist are symlinks into the main
# checkout there, and a build through them rewrites the tracked web/components.d.ts and
# web/auto-imports.d.ts with paths from the wrong tree.
build_web() {
    if [[ -L web/node_modules || -L web/dist ]]; then
        echo "web/dist is stale, but this is a feature worktree (web/dist is a symlink)." >&2
        echo "Build the frontend in the main checkout instead." >&2
        exit 1
    fi
    npm run typecheck:full --prefix web
    npm run build --prefix web
}

# rust-embed pulls web/dist in at compile time, so the frontend has to be there AND current —
# existence alone served a three-day-old bundle out of a fresh binary.
scripts/web-dist-stale.sh || build_web

mkdir -p data downloads

# Providers come from installed plugin manifests only (RD-101-13), and the bundled sync looks
# next to the binary — where a cargo build puts nothing. Without this a fresh development
# instance has no hosters at all and the accounts list is empty, which looks like a bug and is
# not one. Packages come from scripts/build-plugins.sh; if they are missing, say so once.
if [[ -d dist/plugins ]]; then
    extra+=(--bundled-plugins dist/plugins)
else
    echo "note: dist/plugins is empty — run scripts/build-plugins.sh or there will be no providers" >&2
fi

CARGO_BUILD_JOBS="$JOBS" cargo run -j "$JOBS" -p rdownloader -- serve \
    --database "$database" --downloads downloads --listen "$HOST:$PORT" "${extra[@]}"
