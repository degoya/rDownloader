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
#   scripts/worktree.sh new --own-target fix/0.9.3-something   # a check lane of its own
#   scripts/worktree.sh check fix/0.9.3-something     # generated files clean?
#   scripts/worktree.sh finish fix/0.9.3-something    # merge, then remove worktree and branch
#
# --own-target (RD-140-06) gives the worktree its own target/ instead of the main checkout's, so
# its checks run in a lane beside the main checkout's (scripts/lib/lock.sh, RD_LANES) instead of
# after them. It costs a debug working set of ~52 GiB once the worktree builds, and a first build
# of every dependency — from sccache where it is installed, from scratch where not — so it is
# for a branch that is verified on its own while something else holds the shared target, not for
# every worktree of a wave. The plugin components stay the shared ones: its
# target/wasm32-unknown-unknown is a link to the main checkout's.
set -euo pipefail

MAIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Where a worktree's green record lives — the main checkout's target/, or the worktree's own —
# is rd_target_dir's to say (scripts/lib/lanes.sh). Until 1.4 this line exported the main
# target instead, because the fallback then was `<path>/target`: without it the gate looked inside
# the worktree, found nothing and reported "never verified" for a branch that was demonstrably
# green, which cost two refused merges on 2026-09-22. A CARGO_TARGET_DIR that is set still wins.
# shellcheck source=lib/verified.sh
source "$MAIN/scripts/lib/verified.sh"
BASE="${BASE:-development}"
GENERATED=(web/auto-imports.d.ts web/components.d.ts)

usage() { echo "usage: scripts/worktree.sh {new [--own-target]|check|finish} <branch>" >&2; exit 2; }
# `check` is a pure query and stays lock-free; `finish` merges but does not build.
own_target=0
if [[ $# -eq 3 && "$1" == new && "$2" == --own-target ]]; then
    own_target=1
    set -- "$1" "$3"
fi
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
    if [[ "$own_target" -eq 1 ]]; then
        : > "$(rd_own_target_flag "$path")"
        mkdir -p "$path/target" "$MAIN/target/wasm32-unknown-unknown"
        ln -s "$MAIN/target/wasm32-unknown-unknown" "$path/target/wasm32-unknown-unknown"
        echo "==> own target: $path/target (a lane of its own; components linked from $MAIN/target)"
    fi
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
    # The link, not what it points at: those are the main checkout's components.
    [[ -L "$path/target/wasm32-unknown-unknown" ]] && rm -f "$path/target/wasm32-unknown-unknown"

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
