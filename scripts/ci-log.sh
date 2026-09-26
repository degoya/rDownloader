#!/usr/bin/env bash
#
# Reads the failures of a GitHub Actions run (RD-140-22): names the failed jobs, stores each
# one's log without colour codes and timestamps, and prints only the lines that say what broke.
#
# Before this script the same four steps were typed by hand — find the failed job, fetch its log,
# strip the ANSI codes, filter for the failures — twelve times on 2026-09-26 alone.
#
# Usage:
#   scripts/ci-log.sh <run-id>                  # every failed job of the run
#   scripts/ci-log.sh <run-url>                 # https://github.com/<o>/<r>/actions/runs/<id>
#   scripts/ci-log.sh <job-url>                 # .../actions/runs/<id>/job/<job-id>: that job
#   scripts/ci-log.sh --job <job-id>            # one job by its id, failed or not
#   scripts/ci-log.sh <run> --repo owner/name   # another repository than the public one
#
# Printed are the lines matching FAIL, panicked, failures, error[E…] or a plain `error: ` (what
# clippy under -D warnings says), and GitHub's own `##[error]` line for the failed step, each with
# its line number in the stored log, so the context is one `sed -n '<from>,<to>p' <log>` away. A
# job that failed with none of the first kind — a download, a shell step — also gets the lines
# before its first `##[error]`, because that line alone only says "exit code 56".
#
# Environment:
#   RD_PUBLIC_REPO   the default repository (degoya/rDownloader)
#   RD_CI_LOG_DIR    where the logs go (/tmp/claude-<uid>/ci)
#
set -euo pipefail

REPO="${RD_PUBLIC_REPO:-degoya/rDownloader}"
LOG_DIR="${RD_CI_LOG_DIR:-/tmp/claude-$(id -u)/ci}"
PATTERN='FAIL|error(\[E[0-9]+\])?: |panicked|failures'
CONTEXT=20

usage() {
    echo "usage: scripts/ci-log.sh <run-id|run-url|job-url> [--repo owner/name]" >&2
    echo "       scripts/ci-log.sh --job <job-id> [--repo owner/name]" >&2
    exit 2
}

run_id=""
job_id=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo) REPO="${2:?--repo needs owner/name}"; shift 2 ;;
        --job) job_id="${2:?--job needs a job id}"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        https://github.com/*/actions/runs/*)
            REPO="$(sed -E 's#^https://github.com/([^/]+/[^/]+)/.*#\1#' <<< "$1")"
            run_id="$(sed -E 's#.*/actions/runs/([0-9]+).*#\1#' <<< "$1")"
            if [[ "$1" =~ /job/([0-9]+) ]]; then job_id="${BASH_REMATCH[1]}"; fi
            shift ;;
        -*) echo "unknown argument: $1" >&2; usage ;;
        *)
            [[ "$1" =~ ^[0-9]+$ && -z "$run_id" ]] || { echo "not a run id: $1" >&2; usage; }
            run_id="$1"; shift ;;
    esac
done
[[ -n "$run_id" || -n "$job_id" ]] || usage
command -v gh > /dev/null || { echo "gh is required (https://cli.github.com)" >&2; exit 1; }

mkdir -p "$LOG_DIR"

# One job: fetch, clean, store, filter. $1 id, $2 name, $3 run id.
read_job() {
    local id="$1" name="$2" run="$3" log hits count first
    log="$LOG_DIR/${run}-${id}.log"
    # The log is raw terminal output; recent gh refuses to pass its escape codes on unasked.
    gh api --allow-escape-sequences "repos/$REPO/actions/jobs/$id/logs" > "$log.raw" \
        || { echo "!! could not fetch the log of job $id" >&2; rm -f "$log.raw"; return 1; }
    # A byte-order mark, the colour codes, and the timestamp GitHub puts in front of every line.
    sed -E -e '1s/^\xEF\xBB\xBF//' -e 's/\x1b\[[0-9;?]*[A-Za-z]//g' \
        -e 's/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z //' "$log.raw" > "$log"
    rm -f "$log.raw"
    hits="$(grep -nE "$PATTERN|^##\[error\]" "$log" || true)"
    count=0
    [[ -z "$hits" ]] || count="$(wc -l <<< "$hits")"
    echo
    echo "==> $name (job $id): $count line(s) — $log"
    [[ -z "$hits" ]] || printf '%s\n' "$hits"
    if ! grep -qE "$PATTERN" "$log"; then
        first="$(grep -nm1 '^##\[error\]' "$log" | cut -d: -f1 || true)"
        if [[ -n "$first" ]]; then
            echo "    (no test or compiler failure; the $CONTEXT lines before the first ##[error])"
            awk -v from=$((first - CONTEXT)) -v to="$first" 'NR >= from && NR <= to { print NR ":" $0 }' "$log"
        fi
    fi
}

if [[ -n "$job_id" ]]; then
    meta="$(gh api "repos/$REPO/actions/jobs/$job_id" \
        --jq '"\(.run_id)\t\(.conclusion)\t\(.name)"')" \
        || { echo "no job $job_id in $REPO" >&2; exit 1; }
    IFS=$'\t' read -r run conclusion name <<< "$meta"
    echo "job $job_id of run $run in $REPO: $conclusion"
    read_job "$job_id" "$name" "$run"
    exit 0
fi

summary="$(gh run view "$run_id" --repo "$REPO" --json status,conclusion,displayTitle,url \
    --jq '"\(.status) \(.conclusion) \(.url) \(.displayTitle)"')" \
    || { echo "no run $run_id in $REPO" >&2; exit 1; }
echo "run $run_id: $summary"
failed="$(gh run view "$run_id" --repo "$REPO" --json jobs \
    --jq '.jobs[] | select(.conclusion == "failure" or .conclusion == "timed_out" or .conclusion == "startup_failure") | "\(.databaseId)\t\(.conclusion)\t\(.name)"')"
if [[ -z "$failed" ]]; then
    echo "no failed job"
    exit 0
fi
echo "failed jobs: $(wc -l <<< "$failed")"
while IFS=$'\t' read -r id conclusion name; do
    echo "  $id  $conclusion  $name"
done <<< "$failed"

status=0
while IFS=$'\t' read -r id conclusion name; do
    read_job "$id" "$name" "$run_id" || status=1
done <<< "$failed"
exit "$status"
