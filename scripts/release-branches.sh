#!/usr/bin/env bash
#
# The release branches of the user wiki and the website (RD-1270-16).
#
# The wiki pass and the website pass of a release are written on a branch `release-X.Y` in a
# worktree beside each repository's main checkout (`~/projects/rdownloader.wiki-X.Y`,
# `~/projects/rdownloader-website-X.Y`), so the handbook of the coming release never shows on
# `main` before the release does. Before the chain runs, `main` moves to that branch:
# `update-website.sh` commits the release on the site's `main` only, `export-wiki.sh` exports the
# wiki's checkout as it stands. Until 1.27 both steps were typed by hand at every release.
#
#   new X.Y      create the worktree `<repository>-X.Y` on `release-X.Y`, branched from the local
#                `main`, in both repositories; an existing branch gets the worktree it lacks, a
#                worktree already on that branch is left as it is
#   ff X.Y       move `main` to `release-X.Y` in both, a fast-forward only: refused with its reason
#                — `main` has commits the release branch lacks, the release worktree or the
#                checkout `main` is on has uncommitted changes, the branch is missing — before
#                either repository moves; nothing is fetched or pushed
#   status X.Y   one line per repository: how far `release-X.Y` is ahead of and behind `main`;
#                exit 1 when it is ahead, so a release would leave its work behind. `--quiet`
#                prints only that warning — release-start.sh runs it before the chain
#
# X.Y may be given as the release version itself (1.27.0, 1.27.0-beta.1): the branch is the
# minor's. Nothing here fetches, pushes or deletes; a branch and its worktree outlive the release
# (`git worktree remove` by hand).
#
# Usage:
#   scripts/release-branches.sh new 1.27
#   scripts/release-branches.sh ff 1.27
#   scripts/release-branches.sh status 1.27.0 [--quiet]
#
# Environment:
#   RD_WIKI_SRC      the private user wiki's main checkout (default: ~/projects/rdownloader.wiki)
#   RD_WEBSITE_DIR   the website's main checkout (default: ~/projects/rdownloader-website)
#
set -euo pipefail

WIKI="${RD_WIKI_SRC:-$HOME/projects/rdownloader.wiki}"
SITE="${RD_WEBSITE_DIR:-$HOME/projects/rdownloader-website}"

usage() { awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; }

command="${1:-}"
version="${2:-}"
quiet=0
[[ "${3:-}" == --quiet ]] && quiet=1
case "$command" in
    -h|--help) usage; exit 0 ;;
    new|ff|status) ;;
    *) echo "usage: scripts/release-branches.sh new|ff|status X.Y [--quiet]" >&2; exit 2 ;;
esac
if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)(\.[0-9]+(-beta\.[1-9][0-9]*)?)?$ ]]; then
    echo "not a release line: '$version' (X.Y, X.Y.Z or X.Y.Z-beta.N)" >&2
    exit 2
fi
LINE="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
BRANCH="release-$LINE"

# The checkout that has `main` checked out, empty when none has.
main_checkout() {
    git -C "$1" worktree list --porcelain | awk '
        /^worktree / { path = substr($0, 10) }
        /^branch refs\/heads\/main$/ { print path; exit }'
}

# The worktree that has the release branch checked out, empty when none has.
branch_checkout() {
    git -C "$1" worktree list --porcelain | awk -v ref="branch refs/heads/$BRANCH" '
        /^worktree / { path = substr($0, 10) }
        $0 == ref { print path; exit }'
}

has_branch() { git -C "$1" show-ref --verify --quiet "refs/heads/$2"; }
dirty() { [[ -n "$(git -C "$1" status --porcelain)" ]]; }

repo_ok() {
    local repo="$1"
    if ! git -C "$repo" rev-parse --git-dir > /dev/null 2>&1; then
        echo "$repo is not a git checkout" >&2
        return 1
    fi
    if ! has_branch "$repo" main; then
        echo "$repo has no branch main" >&2
        return 1
    fi
}

cmd_new() {
    local repo="$1" target="$1-$LINE" there
    repo_ok "$repo" || return 1
    there="$(branch_checkout "$repo")"
    if [[ -n "$there" ]]; then
        echo "$repo: $BRANCH is already checked out in $there"
        return 0
    fi
    if [[ -e "$target" ]]; then
        echo "$repo: $target exists and is not the worktree of $BRANCH; move it aside first" >&2
        return 1
    fi
    if has_branch "$repo" "$BRANCH"; then
        git -C "$repo" worktree add --quiet "$target" "$BRANCH"
        echo "$repo: worktree $target on the existing $BRANCH"
    else
        git -C "$repo" worktree add --quiet -b "$BRANCH" "$target" main
        echo "$repo: worktree $target on $BRANCH, new from main ($(git -C "$repo" rev-parse --short main))"
    fi
}

# Refuses (exit 1, the reason on stderr) what would keep `ff` from being a clean fast-forward.
ff_ready() {
    local repo="$1" there checkout lacking
    repo_ok "$repo" || return 1
    if ! has_branch "$repo" "$BRANCH"; then
        echo "$repo: no branch $BRANCH (scripts/release-branches.sh new $LINE)" >&2
        return 1
    fi
    if ! git -C "$repo" merge-base --is-ancestor main "$BRANCH"; then
        lacking="$(git -C "$repo" rev-list --count "$BRANCH..main")"
        echo "$repo: main has $lacking commit(s) $BRANCH lacks, so it cannot fast-forward;" \
            "merge main into $BRANCH in its worktree first" >&2
        return 1
    fi
    there="$(branch_checkout "$repo")"
    if [[ -n "$there" ]] && dirty "$there"; then
        echo "$repo: $there has uncommitted changes; commit them on $BRANCH first" >&2
        return 1
    fi
    checkout="$(main_checkout "$repo")"
    if [[ -n "$checkout" ]] && dirty "$checkout"; then
        echo "$repo: $checkout (on main) has uncommitted changes; commit or remove them first" >&2
        return 1
    fi
}

cmd_ff() {
    local repo="$1" checkout from to
    from="$(git -C "$repo" rev-parse main)"
    to="$(git -C "$repo" rev-parse "$BRANCH")"
    if [[ "$from" == "$to" ]]; then
        echo "$repo: main is already at $BRANCH ($(git -C "$repo" rev-parse --short main))"
        return 0
    fi
    checkout="$(main_checkout "$repo")"
    if [[ -n "$checkout" ]]; then
        git -C "$checkout" merge --ff-only --quiet "$BRANCH"
    else
        git -C "$repo" update-ref refs/heads/main "$to" "$from"
    fi
    echo "$repo: main fast-forwarded to $BRANCH," \
        "$(git -C "$repo" rev-list --count "$from..$to") commit(s), now $(git -C "$repo" rev-parse --short main)"
}

# 0 when the release branch is not ahead of main (or does not exist), 1 when it is.
cmd_status() {
    local repo="$1" ahead behind
    if ! git -C "$repo" rev-parse --git-dir > /dev/null 2>&1 || ! has_branch "$repo" main; then
        [[ "$quiet" -eq 1 ]] || echo "$repo: no checkout with a branch main, skipped"
        return 0
    fi
    if ! has_branch "$repo" "$BRANCH"; then
        [[ "$quiet" -eq 1 ]] || echo "$repo: no $BRANCH"
        return 0
    fi
    ahead="$(git -C "$repo" rev-list --count "main..$BRANCH")"
    behind="$(git -C "$repo" rev-list --count "$BRANCH..main")"
    if [[ "$ahead" -gt 0 ]]; then
        echo "warning: $repo: $BRANCH is $ahead commit(s) ahead of main — the release takes main;" \
            "scripts/release-branches.sh ff $LINE"
        return 1
    fi
    [[ "$quiet" -eq 1 ]] || echo "$repo: $BRANCH is not ahead of main ($behind behind)"
}

status=0
case "$command" in
    new)
        for repo in "$WIKI" "$SITE"; do cmd_new "$repo" || status=1; done
        ;;
    ff)
        # Both repositories are checked before either moves, so a refusal leaves neither half done.
        for repo in "$WIKI" "$SITE"; do ff_ready "$repo" || status=1; done
        if [[ "$status" -eq 0 ]]; then
            for repo in "$WIKI" "$SITE"; do cmd_ff "$repo"; done
        else
            echo "nothing moved" >&2
        fi
        ;;
    status)
        for repo in "$WIKI" "$SITE"; do cmd_status "$repo" || status=1; done
        ;;
esac
exit "$status"
