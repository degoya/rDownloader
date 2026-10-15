#!/usr/bin/env bash
#
# Starts the whole release chain detached, so it survives the terminal, the IDE and the agent that
# started it: web build, then scripts/check.sh --full, then scripts/release-pipeline.sh (RD-140-06).
#
# It replaces a wrapper typed anew for every release. 1.2.3 started without the check.sh step and
# had to begin again; waiting on it with `pgrep -f` matched the waiting shell instead of the chain.
# This writes the chain's own PID to a file, so waiting is `kill -0` on a number, never a pattern.
#
# Why check.sh --full before the pipeline: the pipeline's preflight refuses a HEAD no green run
# has seen, and a --full green of the tree before the version bump is what lets the pipeline skip
# its own full Rust run (scripts/lib/verified.sh, rd_prebump_full_green). check.sh decides itself
# whether that run is due (RD-160-06): a --full green of this content, or of content that differs
# in documentation only, recorded by any checkout on the target, is recorded for this HEAD and
# tree instead of run again. Until then this script asked for a green of exactly this HEAD, and the
# chain of 2026-09-28 ran --full again for commits that changed only the changelog and job files.
#
# Nothing here takes the build lock itself: check.sh and release-pipeline.sh each take it, and a
# wrapper around them in `flock` would deadlock (AGENTS.md).
#
# Before anything detaches it refuses, compiling nothing, a tree the chain would stop on later:
# baselines behind the version, the release documents (scripts/release-docs.sh --check), a dirty
# tree. Every finding is named with what fixes it (RD-1300-04).
#
# Usage:
#   scripts/release-start.sh 1.4.0            # start; prints where the log and the PID are
#   scripts/release-start.sh 1.4.0 --push     # ... and hand --push to the pipeline
#   scripts/release-start.sh 1.8.0-beta.1     # a pre-release: the pipeline skips what a beta
#                                             # does not do (scripts/lib/release-tag.sh)
#
# Files, under /tmp/claude-<uid>/release-<version>/ (RD_RELEASE_RUN_DIR overrides it):
#   chain.log   everything the chain prints; the pipeline's evidence log is its own, in artifacts/
#   pid         the PID of the detached chain, written by the chain itself
#   exit        the chain's exit code, written when it ends — absent while it runs
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/release-tag.sh
source "$ROOT/scripts/lib/release-tag.sh"

chain=0
version=""
push=()
for argument in "$@"; do
    case "$argument" in
        --chain) chain=1 ;;
        --push) push=(--push) ;;
        -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $argument" >&2; exit 2 ;;
        *) version="$argument" ;;
    esac
done
if ! rd_release_version "$version"; then
    echo "usage: scripts/release-start.sh <X.Y.Z | X.Y.Z-beta.N> [--push]" >&2
    exit 2
fi
[[ "$chain" -eq 1 ]] || "$ROOT/scripts/release-branches.sh" status "$version" --quiet >&2 || true  # RD-1270-16: wiki/site work left on release-X.Y
# What the chain needs committed, refused here in seconds and compiling nothing, before the web
# build and --full (RD-1300-04); every finding is named, each with what fixes it:
# - the migration-forward suite's newest baseline is the release (RD-1280-23): 1.28.0 shipped with
#   its guard red on main, because the test step reused the green before the bump;
# - the release documents, by release-docs.sh --check, the docs-gate's own source: on 1.29.0 the
#   gate stopped the chain after about 40 minutes because docs/roadmap.md did not name the version;
# - a clean tree, which the pipeline's preflight wants as well.
if [[ "$chain" -eq 0 ]]; then
    refused=0
    if ! "$ROOT/scripts/migration-baselines.sh" --check "$version" >&2; then
        echo "run scripts/migration-baselines.sh $version and commit it" >&2
        refused=1
    fi
    "$ROOT/scripts/release-docs.sh" --check "$version" >&2 || refused=1
    if [[ -n "$(git -C "$ROOT" status --porcelain)" ]]; then
        echo "the working tree is not clean; commit what belongs to the release (git status):" >&2
        git -C "$ROOT" status --short >&2
        refused=1
    fi
    if [[ "$refused" -ne 0 ]]; then
        echo "release $version not started; fix the above, then run scripts/release-start.sh $version again" >&2
        exit 1
    fi
fi

run_dir="${RD_RELEASE_RUN_DIR:-/tmp/claude-$(id -u)/release-$version}"
log="$run_dir/chain.log"
pid_file="$run_dir/pid"
exit_file="$run_dir/exit"

# --- the chain itself, detached ----------------------------------------------------------------
if [[ "$chain" -eq 1 ]]; then
    printf '%s\n' "$$" > "$pid_file"
    status=0
    run() {
        echo
        echo "==> [release-start] $* ($(date -Is))"
        "$@"
    }
    cd "$ROOT"
    {
        # rust-embed compiles web/dist into the binary, so the frontend is built before any cargo.
        run pnpm --dir web run build \
            && run env RD_RELEASE_VERSION="$version" scripts/check.sh --full \
            && run scripts/release-pipeline.sh "$version" "${push[@]}"
    } || status=$?
    echo
    echo "==> [release-start] chain ended with exit $status ($(date -Is))"
    printf '%s\n' "$status" > "$exit_file"
    exit "$status"
fi

# --- the launcher --------------------------------------------------------------------------------
mkdir -p "$run_dir"
if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2> /dev/null && [[ ! -f "$exit_file" ]]; then
    echo "a release chain for $version is already running as PID $(cat "$pid_file")" >&2
    echo "  log: $log" >&2
    exit 1
fi
rm -f "$pid_file" "$exit_file"

setsid nohup "$0" --chain "$version" "${push[@]}" > "$log" 2>&1 < /dev/null &
disown || true

# The chain writes its own PID; setsid may fork, so `$!` is not guaranteed to be it.
for _ in $(seq 1 50); do
    [[ -s "$pid_file" ]] && break
    sleep 0.1
done
if [[ ! -s "$pid_file" ]]; then
    echo "the chain did not start; see $log" >&2
    exit 1
fi

pid="$(cat "$pid_file")"
cat <<INFO
==> release chain for $version started, detached, as PID $pid$([[ ${#push[@]} -gt 0 ]] && echo ' (--push)')
    log:   $log
    pid:   $pid_file
    exit:  $exit_file (written when the chain ends)

    follow: tail -f $log
    wait:   while kill -0 $pid 2> /dev/null; do sleep 30; done; cat $exit_file
INFO
