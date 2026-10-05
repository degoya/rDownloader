#!/usr/bin/env bash
#
# Regenerates the frontend's unplugin declarations, web/components.d.ts and web/auto-imports.d.ts.
# Only a vite build writes them, and nothing in a wave builds the frontend before the release:
# a worktree's web/dist is a link into the main checkout, so `check.sh --full` there builds no
# frontend, and a component a branch added stayed out of components.d.ts until the package build
# refused the tree on development (QueueColumnHeader in 1.9.1, UsenetQuotaEditor in 1.10).
# integrate.sh runs this among its generators, so the declarations go into the generated commit.
#
# The build goes into a directory of its own, never web/dist, which may be the main checkout's.
#
# Usage: scripts/web-declarations.sh [<out-dir>]    (default: a temporary directory, removed after)
set -euo pipefail

cd "$(dirname "$0")/.."
out="${1:-}"
if [[ -z "$out" ]]; then
    out="$(mktemp -d)"
    trap 'rm -rf "$out"' EXIT
fi
log="$out.log"
if ! pnpm --dir web exec vite build --outDir "$out" --emptyOutDir > "$log" 2>&1; then
    echo "!! the frontend build failed — $log" >&2
    exit 1
fi
rm -f "$log"
if git diff --quiet -- web/components.d.ts web/auto-imports.d.ts; then
    echo "    web declarations: current"
else
    echo "    web declarations: regenerated"
    git diff --stat -- web/components.d.ts web/auto-imports.d.ts | sed 's/^/    /'
fi
