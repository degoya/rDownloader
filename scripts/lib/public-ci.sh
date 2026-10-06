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
#   RD_PUBLIC_CI_TIMEOUT  seconds to wait for a run to appear at all (900)
#   RD_PUBLIC_CI_CEILING  seconds after which a run that is still going is given up (21600)
#   RD_PUBLIC_CI_POLL     seconds between two looks (60); each look asks for the runs and their jobs
#
# No fixed deadline for a run that is going (RD-150-10): the 1.4.2 chain stopped after 5400 s
# with every job green and `docker` still running. Every job in ci.yml carries its own
# `timeout-minutes`, so GitHub ends a run by itself; the ceiling is the six hours a hosted job
# may run at most, and only catches a run whose jobs never leave the queue.
#
# Only the runner images no recorded green covers are dispatched (RD-160-06): a green run is
# recorded per image and tree (scripts/lib/verified.sh, rd_record_ci), and a tree that differs from
# a green one only in documentation and version lines — the release candidate against its wave's
# integration branch — counts as green. A red run records nothing. With Linux and Windows green on
# record ci.yml's once-per-run jobs are left out too (`jobs=[]`, RD-1120-07): those runs passed
# them on the same content.
#
# The release candidate also dispatches the workflows of RD_PUBLIC_CI_RELEASE_WORKFLOWS beside
# ci.yml (owner, 2026-10-06): the export's push carries `[skip ci]`, so until 1.12 they first ran
# on the push to `main` after the tag. They are waited for with ci.yml's run, a red one holds the
# tag, and a green is recorded under the workflow's file name, like an image.

# shellcheck source=verified.sh
source "$(dirname "${BASH_SOURCE[0]}")/verified.sh"

PUBLIC_DIR="${RD_PUBLIC_DIR:-$HOME/projects/rDownloader-public}"
PUBLIC_REPO="${RD_PUBLIC_REPO:-degoya/rDownloader}"
PUBLIC_CI_TIMEOUT="${RD_PUBLIC_CI_TIMEOUT:-900}"
PUBLIC_CI_CEILING="${RD_PUBLIC_CI_CEILING:-21600}"
PUBLIC_CI_POLL="${RD_PUBLIC_CI_POLL:-60}"
# Every platform ci.yml checks; read by the scripts that source this file.
# shellcheck disable=SC2034
RD_PUBLIC_CI_ALL="linux,windows,macos"
# The workflows a release candidate runs before its tag besides ci.yml (owner, 2026-10-06), each
# with its own defaults: the installers and the self-update as a user meets them, the kill and
# restart of axis B, the end-to-end runs on the three systems.
# shellcheck disable=SC2034
RD_PUBLIC_CI_RELEASE_WORKFLOWS=(e2e.yml recovery.yml self-update.yml installers.yml)

rd_public_ci_gh_ready() {
    command -v gh > /dev/null || { echo "gh is required to watch the public CI" >&2; return 1; }
    gh auth status --hostname github.com > /dev/null 2>&1 \
        || { echo "gh is not signed in to github.com (gh auth login)" >&2; return 1; }
}

# The runner images, one per line, of a comma-separated list of short names (linux, windows,
# macos) or runner images. The images are the ones ci.yml names; a new image there is a new line
# here.
rd_public_ci_images() {
    local item image
    local -a items
    IFS=',' read -r -a items <<< "$1"
    [[ ${#items[@]} -gt 0 ]] || { echo "no platform named" >&2; return 2; }
    for item in "${items[@]}"; do
        case "$item" in
            linux) image="ubuntu-24.04" ;;
            windows) image="windows-2025" ;;
            macos) image="macos-15" ;;
            ubuntu-*|windows-*|macos-*) image="$item" ;;
            *) echo "unknown platform: $item (linux, windows, macos or a runner image)" >&2; return 2 ;;
        esac
        printf '%s\n' "$image"
    done
}

# The JSON list ci.yml's `platforms` input takes, from the same comma-separated list.
rd_public_ci_platforms() {
    local images
    images="$(rd_public_ci_images "$1")" || return 2
    printf '[%s]\n' "$(sed 's/.*/"&"/' <<< "$images" | paste -sd, -)"
}

# Which of runner images $3... tree $2 of checkout $1 still needs a run on. Says for each image
# whether a recorded green covers it (rd_ci_covering) and leaves the others, in order, in the
# array RD_PUBLIC_CI_MISSING. Workflow file names are planned the same way.
rd_public_ci_plan() {
    local root="$1" tree="$2" image covering
    shift 2
    RD_PUBLIC_CI_MISSING=()
    for image in "$@"; do
        covering="$(rd_ci_covering "$root" "$tree" "$image")"
        if [[ -z "$covering" ]]; then
            RD_PUBLIC_CI_MISSING+=("$image")
            echo "  $image: to run"
        elif [[ "$covering" == "$tree" ]]; then
            echo "  $image: green for this tree already"
        else
            echo "  $image: green already for tree ${covering:0:12}, which differs only in documentation and version lines"
        fi
    done
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

# ci.yml's `jobs` input for tree $2 of checkout $1 (RD-1120-07): `[]` when recorded Linux and
# Windows greens cover the tree — a run is recorded only when all of it was green, once-per-run
# jobs included — and nothing otherwise, which leaves ci.yml's default, every one of them.
rd_public_ci_once_jobs() {
    local root="$1" tree="$2" image
    for image in ubuntu-24.04 windows-2025; do
        [[ -n "$(rd_ci_covering "$root" "$tree" "$image")" ]] || return 0
    done
    echo "[]"
}

# Starts ci.yml on branch $1 for the platforms in JSON list $2, and with JSON list $3, when given,
# as its once-per-run `jobs`.
rd_public_ci_dispatch() {
    local -a jobs=()
    [[ -z "${3:-}" ]] || jobs=(-f jobs="$3")
    echo "starting ci.yml on $1 for $2${3:+, once-per-run jobs $3}"
    gh workflow run ci.yml --repo "$PUBLIC_REPO" --ref "$1" -f platforms="$2" "${jobs[@]}"
}

# Starts workflow $2 (a file name of RD_PUBLIC_CI_RELEASE_WORKFLOWS) on branch $1 with its defaults.
rd_public_ci_dispatch_workflow() {
    echo "starting $2 on $1"
    gh workflow run "$2" --repo "$PUBLIC_REPO" --ref "$1"
}

# The jobs of run $1 that changed since the last look, one line each, and every job that failed
# the moment it is seen failed (RD-1100-13): a Windows job that is red 50 minutes before the run
# ends is worth those 50 minutes. Keeps what it saw in the caller's associative array
# RD_PUBLIC_CI_JOBS ("<run>/<job>" → "<status> <conclusion>"). A job still queued says nothing.
rd_public_ci_jobs() {
    local run="$1" jobs status conclusion url name key
    jobs="$(gh run view "$run" --repo "$PUBLIC_REPO" --json jobs \
        --jq '.jobs[] | "\(.status) \(if (.conclusion // "") == "" then "-" else .conclusion end) \(.url) \(.name)"' \
        2> /dev/null < /dev/null)" || return 0
    while read -r status conclusion url name; do
        [[ -n "$status" ]] || continue
        key="$run/$name"
        [[ "${RD_PUBLIC_CI_JOBS[$key]:-}" != "$status $conclusion" ]] || continue
        RD_PUBLIC_CI_JOBS[$key]="$status $conclusion"
        case "$status $conclusion" in
            "completed success"|"completed skipped"|"completed neutral")
                echo "  $(date +%H:%M:%S) $name: $conclusion" ;;
            completed\ *)
                echo "  $(date +%H:%M:%S) $name: $conclusion — failed, while the run goes on"
                echo "!! $name: $conclusion — scripts/ci-log.sh $url" >&2 ;;
            in_progress\ *) echo "  $(date +%H:%M:%S) $name: started" ;;
        esac
    done <<< "$jobs"
}

# Runs as gh lists them ("<id> <status> <conclusion|-> <url> <name>") as the lines this file
# prints and the release pipeline reads: "<status> <conclusion> <name> <url>".
rd_public_ci_runs_text() {
    awk '{ status = $2; conclusion = ($3 == "-" ? "" : $3); url = $4
           $1 = $2 = $3 = $4 = ""; sub(/^ +/, "")
           print status " " conclusion " " $0 " " url }'
}

# One run as the line above, from gh's JSON — for `run list` per element, for `run view` whole.
RD_PUBLIC_CI_RUN_JQ='"\(.databaseId) \(.status) \(if (.conclusion // "") == "" then "-" else .conclusion end) \(.url) \(.name)"'

# Notes run line $1 in the caller's RD_PUBLIC_CI_RUNS (id → line) and prints it when it changed.
rd_public_ci_note() {
    local id status conclusion url name
    read -r id status conclusion url name <<< "$1"
    [[ -n "$id" && "${RD_PUBLIC_CI_RUNS[$id]:-}" != "$1" ]] || return 0
    RD_PUBLIC_CI_RUNS[$id]="$1"
    echo "  $(date +%H:%M:%S) run $name ($url): $status$([[ "$conclusion" == - ]] || echo " $conclusion")"
}

# Waits for every run on branch $1 at commit $2 — only those of event $3 when given — to
# finish, and prints them. Returns 0 when all of them succeeded (or were skipped), 1 when one
# did not, when no run appeared within RD_PUBLIC_CI_TIMEOUT, or when one was still going at
# RD_PUBLIC_CI_CEILING. While it waits it prints only what changed: a job that started, finished
# or failed (rd_public_ci_jobs), a run that appeared or finished.
#
# How it looks (RD-1120-06, audit A3). `gh run list` finds the runs; the start deadline holds only
# until it has found one. From then on each run is asked for by its id (`gh run view`), and a
# failed or empty answer keeps what was seen last — until 1.12 an empty list after the first look
# counted as "no CI run appeared" once 900 s had passed, while the run was going (1.11 wave 2,
# run 37343506422). The ceiling stays the one limit. "Every run completed" is read twice: the
# second look lists again, so a run of another workflow that appeared meanwhile is waited for too,
# and asks every known run again by its id.
rd_public_ci_wait() {
    local branch="$1" sha="$2" event="${3:-}" listed line failed deadline ceiling id rest runs
    local waiting_said=0 confirming=0 seen=0 found unread
    local -a filter=()
    # Read by rd_public_ci_note and rd_public_ci_jobs, which bash's dynamic scope lets see them.
    # shellcheck disable=SC2034
    local -A RD_PUBLIC_CI_RUNS=() RD_PUBLIC_CI_JOBS=()
    [[ -z "$event" ]] || filter=(--event "$event")
    echo "waiting for the CI of $PUBLIC_REPO on $branch at ${sha:0:12} (for as long as it runs)"
    deadline=$(( SECONDS + PUBLIC_CI_TIMEOUT ))
    ceiling=$(( SECONDS + PUBLIC_CI_CEILING ))
    while :; do
        local -A fresh=()
        found=0
        unread=0
        if [[ ${#RD_PUBLIC_CI_RUNS[@]} -eq 0 || "$confirming" -eq 1 ]]; then
            listed="$(gh run list --repo "$PUBLIC_REPO" --branch "$branch" --commit "$sha" "${filter[@]}" \
                --json databaseId,status,conclusion,name,url --jq ".[] | $RD_PUBLIC_CI_RUN_JQ" \
                2> /dev/null < /dev/null)" || listed=""
            while read -r id rest; do
                [[ -n "$id" && -z "${RD_PUBLIC_CI_RUNS[$id]:-}" ]] || continue
                rd_public_ci_note "$id $rest"
                fresh[$id]=1
                found=1
                seen=1
            done <<< "$listed"
        fi
        for id in "${!RD_PUBLIC_CI_RUNS[@]}"; do
            [[ -z "${fresh[$id]:-}" ]] || continue
            line="$(gh run view "$id" --repo "$PUBLIC_REPO" --json databaseId,status,conclusion,name,url \
                --jq "$RD_PUBLIC_CI_RUN_JQ" 2> /dev/null < /dev/null)" || line=""
            if [[ "$line" == "$id "* ]]; then rd_public_ci_note "$line"; else unread=1; fi
        done
        for id in "${!RD_PUBLIC_CI_RUNS[@]}"; do rd_public_ci_jobs "$id"; done
        runs="$(printf '%s\n' "${RD_PUBLIC_CI_RUNS[@]}" | sed '/^$/d' | sort -n)"
        if [[ -n "$runs" ]] && ! awk '$2 != "completed" { found = 1 } END { exit !found }' <<< "$runs"; then
            [[ "$confirming" -eq 0 || "$found" -eq 1 || "$unread" -eq 1 ]] || break
            # Every run completed for the first time: read once more, at once.
            if [[ "$confirming" -eq 0 ]]; then confirming=1; continue; fi
        else
            confirming=0
        fi
        if [[ "$seen" -eq 0 ]] && (( SECONDS >= deadline )); then
            echo "no CI run appeared within ${PUBLIC_CI_TIMEOUT}s; $branch is kept" >&2
            return 1
        fi
        if (( SECONDS >= ceiling )); then
            echo "the public CI did not finish within ${PUBLIC_CI_CEILING}s; $branch is kept" >&2
            [[ -z "$runs" ]] || rd_public_ci_runs_text <<< "$runs" >&2
            return 1
        fi
        if [[ "$seen" -eq 0 && "$waiting_said" -eq 0 ]]; then
            echo "  $(date +%H:%M:%S) no run for ${sha:0:12} yet"
            waiting_said=1
        fi
        sleep "$PUBLIC_CI_POLL"
    done

    runs="$(rd_public_ci_runs_text <<< "$runs")"
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
