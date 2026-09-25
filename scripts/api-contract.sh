#!/usr/bin/env bash
#
# Regenerates the OpenAPI document and the TypeScript types from it.
#
# Two things are easy to get wrong by hand and both are silent: the generator writes no trailing
# newline, so the file differs from every other one in the tree, and forgetting the second step
# leaves the frontend types describing an API that no longer exists.
#
# Run this after changing any REST endpoint or DTO.
#
# Usage:
#   scripts/api-contract.sh
#   scripts/api-contract.sh --check    # fail if it would change anything
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
rd_take_lock "$@"
cd "$ROOT"

check_only=0
for argument in "$@"; do
    case "$argument" in
        --check) check_only=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

# rust-embed pulls web/dist in at compile time, so the binary cannot build without it. But
# nothing this script produces depends on the bundle's *contents*: the `openapi` subcommand
# serves no assets, and `npm run generate:api` is a pure transform over the JSON. So a bundle
# has to exist; it does not have to be current.
#
# That distinction is the whole point of this function. Refusing a stale one used to lock the
# door on exactly the branches that need it most: a feature worktree cannot build the frontend
# (web/node_modules and web/dist are symlinks into the main checkout, and a build through them
# rewrites the tracked web/components.d.ts and web/auto-imports.d.ts with paths from the wrong
# tree), so any branch that touched web/src *and* changed a route hit a hard stop here and got
# "build it in the main checkout" for a build it did not need. Both halves of that guard were
# right on their own and wrong together. Found on 2026-09-23 by the branch that added
# GET /api/v1/remote-jobs/providers.
ensure_web_dist() {
    if scripts/web-dist-stale.sh; then
        return
    fi
    if [[ -L web/node_modules || -L web/dist ]]; then
        if [[ -f web/dist/index.html ]]; then
            echo "==> web/dist is stale; generating against it anyway"
            echo "    (this script needs a bundle to compile against, not a current one)"
            return
        fi
        echo "web/dist/index.html does not exist, and this is a feature worktree." >&2
        echo "Build the frontend in the main checkout instead." >&2
        exit 1
    fi
    # The main checkout can build, and keeping its bundle current is worth the time there.
    #
    # No typecheck here, deliberately. This function exists so `rust-embed` has a bundle to
    # compile against, and it runs *before* the contract is regenerated -- so typechecking here
    # means typechecking the frontend against the schema this run is about to replace. On
    # 2026-09-23 that deadlocked a merge: `web/src/api/schema.d.ts` had come from a branch that
    # predated a new route, the view using that route failed to typecheck, and the regeneration
    # that would have fixed the schema never ran because the typecheck stopped it first. The
    # typecheck belongs after generation, and `check.sh` runs it there.
    npm run build --prefix web
}

ensure_web_dist

echo "==> generating web/openapi.json"
CARGO_BUILD_JOBS="$JOBS" cargo run -j "$JOBS" -p rdownloader --quiet -- openapi --output web/openapi.json
# The CLI omits it; every other file in the tree has one.
printf '\n' >> web/openapi.json

echo "==> generating web/src/api/schema.d.ts"
npm run generate:api --prefix web

if [[ "$check_only" -eq 1 ]]; then
    if git diff --quiet -- web/openapi.json web/src/api/schema.d.ts; then
        echo "==> the committed contract matches the code"
    else
        echo "!! the API contract is out of date; commit the regenerated files" >&2
        git diff --stat -- web/openapi.json web/src/api/schema.d.ts >&2
        exit 1
    fi
else
    git diff --stat -- web/openapi.json web/src/api/schema.d.ts
fi
