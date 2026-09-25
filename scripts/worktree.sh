#!/usr/bin/env bash
#
# Feature worktrees, set up and taken down the way this repository needs them.
#
# Two traps this encodes, both of which cost real time:
#
#  * A fresh worktree has no `web/node_modules` and no `web/dist`, and `rust-embed` refuses to
#    compile without the latter. Symlinking both from the main checkout is the fast way — but a
#    symlink is not matched by the `node_modules/` ignore rule, so it shows up as untracked and
#    can be staged by a careless `git add`.
#  * The unplugin generators resolve *through* that symlink, so `npm run build` inside a worktree
#    rewrites `web/components.d.ts` and `web/auto-imports.d.ts` to point at the other checkout.
#    Those files are tracked, so it lands in a commit and is broken everywhere else.
#
# Usage:
#   scripts/worktree.sh new fix/0.9.3-something
#   scripts/worktree.sh check fix/0.9.3-something     # generated files clean?
#   scripts/worktree.sh finish fix/0.9.3-something    # merge, then remove worktree and branch
#
set -euo pipefail

MAIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Worktrees share the main checkout's target directory -- that is the documented setup, and the
# green marker a worktree's own `check.sh` recorded lives there. `rd_target_dir` falls back to
# `<path>/target` when CARGO_TARGET_DIR is unset, so without this line the gate below looks inside
# the worktree, finds nothing and reports "never verified" for a branch that is demonstrably
# green. It cost two refused merges on 2026-09-22 before the cause was found, and a gate that
# says "unverified" when it merely cannot see the record is a gate people route around.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MAIN/target}"
# shellcheck source=lib/verified.sh
source "$MAIN/scripts/lib/verified.sh"
BASE="${BASE:-development}"
GENERATED=(web/auto-imports.d.ts web/components.d.ts)

usage() { echo "usage: scripts/worktree.sh {new|check|finish} <branch>" >&2; exit 2; }
# `check` is a pure query and stays lock-free; `finish` merges but does not build.
[[ $# -eq 2 ]] || usage
command="$1"
branch="$2"
# fix/0.9.3-foo -> rDownloader-0.9.3-foo, beside the main checkout.
path="$MAIN-$(echo "$branch" | tr '/' '-')"

restore_generated() {
    local tree="$1"
    git -C "$tree" checkout -- "${GENERATED[@]}" 2>/dev/null || true
}

case "$command" in
new)
    cd "$MAIN"
    git worktree add -b "$branch" "$path" "$BASE"
    ln -s "$MAIN/web/node_modules" "$path/web/node_modules"
    ln -s "$MAIN/web/dist" "$path/web/dist"
    cat <<INFO

==> $path is ready on $branch (from $BASE)

    web/node_modules and web/dist are symlinks into the main checkout. Never stage them, and
    do not run 'npm run build' in there — it rewrites the generated .d.ts files through the
    link. Build the frontend in the main checkout after merging.
INFO
    ;;

check)
    [[ -d "$path" ]] || { echo "no worktree at $path" >&2; exit 1; }
    if git -C "$path" diff --quiet -- "${GENERATED[@]}"; then
        echo "==> generated declarations are unchanged"
    else
        echo "!! the generated declarations were rewritten, most likely through the symlink" >&2
        git -C "$path" diff --stat -- "${GENERATED[@]}" >&2
        echo "   discard them with: git -C $path checkout -- ${GENERATED[*]}" >&2
        exit 1
    fi
    ;;

finish)
    [[ -d "$path" ]] || { echo "no worktree at $path" >&2; exit 1; }
    # Symlink damage is discarded rather than merged: it is never a real change.
    restore_generated "$path"
    # This is where a deferral is used up. scripts/check.sh --defer lets a translated string or
    # a colour be committed without a forty minute run, and the next ordinary run picks those
    # commits up by itself — but only if one happens before the merge. A branch whose HEAD no
    # green run has ever seen is not "tests pass", so it does not get merged.
    rd_verified_gate "$path" "$branch" || exit 1
    if [[ -n "$(git -C "$path" status --porcelain --untracked-files=no)" ]]; then
        echo "!! $branch has uncommitted changes" >&2
        git -C "$path" status --short >&2
        exit 1
    fi
    rm -f "$path/web/node_modules" "$path/web/dist"

    cd "$MAIN"
    git checkout "$BASE"
    git merge --no-ff "$branch"
    # Only now: the branch and the worktree are what the merge can be recovered from.
    git worktree remove "$path"
    git branch -d "$branch"
    echo "==> $branch merged into $BASE; worktree and branch removed"
    echo "    the frontend in this checkout is stale — run 'npm run build --prefix web'"
    ;;

*) usage ;;
esac
