# shellcheck shell=bash
#
# The public CI run, shared by scripts/public-ci.sh and the release pipeline's `public-ci` step
# (RD-130-23, RD-140-22): export a tree to the public repository as a branch, let GitHub's
# runners check it on the platforms this machine cannot, wait, and delete the branch on green.
# A red run keeps its branch, so the failure can be read next to its tree.
#
# Sourced, never run. Every function returns non-zero on a failure instead of exiting: the
# release pipeline calls its steps without errexit and reads the status itself.
#
# Environment:
#   RD_PUBLIC_DIR         the local clone of the public repository (~/projects/rDownloader-public)
#   RD_PUBLIC_REPO        the GitHub repository (degoya/rDownloader)
#   RD_PUBLIC_CI_TIMEOUT  seconds to wait for the runs (5400)
#   RD_PUBLIC_CI_POLL     seconds between two looks (60)

PUBLIC_DIR="${RD_PUBLIC_DIR:-$HOME/projects/rDownloader-public}"
PUBLIC_REPO="${RD_PUBLIC_REPO:-degoya/rDownloader}"
PUBLIC_CI_TIMEOUT="${RD_PUBLIC_CI_TIMEOUT:-5400}"
PUBLIC_CI_POLL="${RD_PUBLIC_CI_POLL:-60}"

rd_public_ci_gh_ready() {
    command -v gh > /dev/null || { echo "gh is required to watch the public CI" >&2; return 1; }
    gh auth status --hostname github.com > /dev/null 2>&1 \
        || { echo "gh is not signed in to github.com (gh auth login)" >&2; return 1; }
}

# The JSON list ci.yml's `platforms` input takes, from a comma-separated list of short names
# (linux, windows, macos) or runner images. The images are the ones ci.yml names; a new image
# there is a new line here.
rd_public_ci_platforms() {
    local item json="" image
    local -a items
    IFS=',' read -r -a items <<< "$1"
    for item in "${items[@]}"; do
        case "$item" in
            linux) image="ubuntu-24.04" ;;
            windows) image="windows-2025" ;;
            macos) image="macos-15" ;;
            ubuntu-*|windows-*|macos-*) image="$item" ;;
            *) echo "unknown platform: $item (linux, windows, macos or a runner image)" >&2; return 2 ;;
        esac
        json+="${json:+,}\"$image\""
    done
    [[ -n "$json" ]] || { echo "no platform named" >&2; return 2; }
    printf '[%s]\n' "$json"
}

# Deletes every ci/* branch of the public repository but $1: what an earlier red run left.
rd_public_ci_prune_stale() {
    local keep="$1" stale
    while read -r stale; do
        [[ -n "$stale" && "$stale" != "$keep" ]] || continue
        echo "deleting $stale, left from an earlier run"
        git -C "$PUBLIC_DIR" push origin --delete "$stale" || echo "could not delete $stale" >&2
    done < <(git -C "$PUBLIC_DIR" ls-remote --heads origin 'refs/heads/ci/*' \
        | awk '{ sub("^refs/heads/", "", $2); print $2 }')
}

# Starts ci.yml on branch $1 for the platforms in JSON list $2.
rd_public_ci_dispatch() {
    echo "starting ci.yml on $1 for $2"
    gh workflow run ci.yml --repo "$PUBLIC_REPO" --ref "$1" -f platforms="$2"
}

# Waits for every run on branch $1 at commit $2 — only those of event $3 when given — to
# finish, and prints them. Returns 0 when all of them succeeded (or were skipped), 1 when one
# did not or the deadline passed.
rd_public_ci_wait() {
    local branch="$1" sha="$2" event="${3:-}" runs failed deadline
    local -a filter=()
    [[ -z "$event" ]] || filter=(--event "$event")
    echo "waiting for the CI of $PUBLIC_REPO on $branch at ${sha:0:12} (at most ${PUBLIC_CI_TIMEOUT}s)"
    deadline=$(( SECONDS + PUBLIC_CI_TIMEOUT ))
    while :; do
        # A failed query is a network hiccup until the deadline says otherwise.
        runs="$(gh run list --repo "$PUBLIC_REPO" --branch "$branch" --commit "$sha" "${filter[@]}" \
            --json status,conclusion,name,url \
            --jq '.[] | "\(.status) \(.conclusion) \(.name) \(.url)"' 2> /dev/null)" || runs=""
        if [[ -n "$runs" ]] && ! grep -qv '^completed ' <<< "$runs"; then
            break
        fi
        if (( SECONDS >= deadline )); then
            echo "the public CI did not finish within ${PUBLIC_CI_TIMEOUT}s; $branch is kept" >&2
            [[ -z "$runs" ]] || echo "$runs" >&2
            return 1
        fi
        if [[ -z "$runs" ]]; then
            echo "  $(date +%H:%M:%S) no run for ${sha:0:12} yet"
        else
            echo "  $(date +%H:%M:%S) $(wc -l <<< "$runs") run(s), $(grep -vc '^completed ' <<< "$runs") not finished"
        fi
        sleep "$PUBLIC_CI_POLL"
    done

    echo "$runs"
    failed="$(grep -vE '^completed (success|skipped|neutral) ' <<< "$runs" || true)"
    if [[ -n "$failed" ]]; then
        echo "the public CI is red; $branch is kept for inspection:" >&2
        echo "$failed" >&2
        echo "read the failures with: scripts/ci-log.sh <run url>" >&2
        return 1
    fi
    echo "the public CI is green on ${sha:0:12}"
}

# Deletes branch $1 from the public repository and from the local clone.
rd_public_ci_delete() {
    git -C "$PUBLIC_DIR" push origin --delete "$1" || return 1
    # Before the first release the clone has no main to return to and still stands on the
    # branch; its local ref then simply stays until the next export.
    git -C "$PUBLIC_DIR" branch -D "$1" > /dev/null 2>&1 || true
    echo "$1 deleted"
}
