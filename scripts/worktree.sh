#!/usr/bin/env bash
#
# Feature worktrees, set up and taken down the way this repository needs them.
#
# A fresh worktree has no `web/node_modules` and no `web/dist`, and `rust-embed` refuses to
# compile without the latter.
#
#  * `web/node_modules` is the worktree's own, a `pnpm install --frozen-lockfile` from the shared
#    store: hard links, a second or two (RD-150-14). Until 1.5 it was a symlink to the main
#    checkout's, and the unplugin generators resolved *through* it, so a build here rewrote the
#    tracked `web/components.d.ts` and `web/auto-imports.d.ts` to point at the other checkout.
#  * `web/dist` is still a symlink to the main checkout's, the fast way to something `rust-embed`
#    can compile. A symlink is not matched by the `dist/` ignore rule, so it shows up as untracked
#    and can be staged by a careless `git add`; and a build here would write through it into the
#    main checkout. Before building in a worktree, remove the link — the link, not what it points
#    at: `rm web/dist`.
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
# shellcheck source=lib/web-dist.sh
source "$MAIN/scripts/lib/web-dist.sh"
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
    pnpm install --dir "$path/web" --frozen-lockfile
    ln -s "$MAIN/web/dist" "$path/web/dist"
    if [[ "$own_target" -eq 1 ]]; then
        : > "$(rd_own_target_flag "$path")"
        mkdir -p "$path/target" "$MAIN/target/wasm32-unknown-unknown"
        ln -s "$MAIN/target/wasm32-unknown-unknown" "$path/target/wasm32-unknown-unknown"
        echo "==> own target: $path/target (a lane of its own; components linked from $MAIN/target)"
    fi
    cat <<INFO

==> $path is ready on $branch (from $BASE)

    web/node_modules is this worktree's own. web/dist is a symlink into the main checkout:
    never stage it, and 'rm web/dist' (the link only) before 'pnpm --dir web run build'.
INFO
    ;;

check)
    [[ -d "$path" ]] || { echo "no worktree at $path" >&2; exit 1; }
    # A warning here, a refusal in check.sh, the packaging scripts and e2e.sh --build (RD-1120-06).
    rd_web_dist_guard "$path" "a check, package or e2e build of $branch" \
        || echo "   (a warning here; those scripts refuse to run in it)" >&2
    if git -C "$path" diff --quiet -- "${GENERATED[@]}"; then
        echo "==> generated declarations are unchanged"
    else
        echo "!! the generated declarations were rewritten" >&2
        git -C "$path" diff --stat -- "${GENERATED[@]}" >&2
        echo "   discard them with: git -C $path checkout -- ${GENERATED[*]}" >&2
        exit 1
    fi
    ;;

finish)
    [[ -d "$path" ]] || { echo "no worktree at $path" >&2; exit 1; }
    # A rewrite of the generated declarations is discarded rather than merged: the build in the
    # merged checkout writes them again, from its own inventory.
    restore_generated "$path"
    # A branch already contained in the base — a wave branch, merged and checked through its
    # integration branch — brings nothing a merge could add. Only the worktree and the branch go;
    # uncommitted work still stops it, because that is not in the base.
    if git -C "$MAIN" merge-base --is-ancestor "$branch" "$BASE"; then
        if [[ -n "$(git -C "$path" status --porcelain --untracked-files=no)" ]]; then
            echo "!! $branch has uncommitted changes" >&2
            git -C "$path" status --short >&2
            exit 1
        fi
        echo "==> $branch is already in $BASE; removing its worktree and branch"
        [[ -L "$path/web/dist" ]] && rm -f "$path/web/dist"
        [[ -L "$path/target/wasm32-unknown-unknown" ]] && rm -f "$path/target/wasm32-unknown-unknown"
        git -C "$MAIN" worktree remove "$path"
        git -C "$MAIN" branch -d "$branch"
        exit 0
    fi
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
    # The link only; a web/dist the worktree built itself is ignored and goes with the worktree.
    [[ -L "$path/web/dist" ]] && rm -f "$path/web/dist"
    # The link, not what it points at: those are the main checkout's components.
    [[ -L "$path/target/wasm32-unknown-unknown" ]] && rm -f "$path/target/wasm32-unknown-unknown"

    cd "$MAIN"
    git checkout "$BASE"
    git merge --no-ff "$branch"
    # Only now: the branch and the worktree are what the merge can be recovered from.
    git worktree remove "$path"
    git branch -d "$branch"
    echo "==> $branch merged into $BASE; worktree and branch removed"
    echo "    the frontend in this checkout is stale — run 'pnpm --dir web run build'"
    ;;

*) usage ;;
esac
