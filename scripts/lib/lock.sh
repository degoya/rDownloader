#!/usr/bin/env bash
# shellcheck shell=bash
#
# One heavy job at a time, as code rather than as a sentence somebody has to remember.
#
# Until RD-120-25 the serialisation existed only as prose in AGENTS.md and in the job files:
# every agent and every person had to check by hand whether another build was running, and
# `grep -rn flock scripts/` found nothing at all. Two full runs in two checkouts against the
# same target/ is how this machine goes into swap.
#
# Usage, at the top of a script that builds, tests, lints or packages, after ROOT is known and
# BEFORE the script changes directory or parses its arguments:
#
#     source "$ROOT/scripts/lib/lock.sh"
#     rd_take_lock "$@"
#
# The function does not hold a lock in this shell; it re-runs the script under `flock`, so the
# lock lives exactly as long as that process does and is released even on a kill -9.
#
# Environment:
#   RD_LOCK_FILE   the lock (default /tmp/rd-build.lock — the file the job files already name)
#   RD_LOCK_WAIT   seconds to wait before giving up (default 7200), for the lock and then again
#                  for memory
#   RD_MIN_FREE_MB MiB of MemAvailable a run waits for once it holds the lock (default 6144;
#                  0 switches the gate off). A run that gives up exits 198.
#   RD_NO_LOCK=1   run without the lock. Then the target/ stamp of scripts/check.sh is yours to
#                  take care of: that marker (see §5 of RD-120-25) only stamps on a checkout
#                  change, and it is the lock that keeps two checkouts from interleaving.
#   RD_LOCK_HELD   set by this function before the re-exec, so a chain (release.sh → check.sh →
#                  build-plugins.sh) holds ONE lock and nested calls do not deadlock on it.
#
# Deliberately NOT locked: the pure queries that only stat files — `build-plugins.sh
# --list-stale`, `--list-missing`, `--list-packageable`, `set-version.sh`, `worktree.sh check`,
# and `check.sh --defer`, which runs no cargo at all.

# Resolved while this file is sourced, which is before the caller does its `cd "$ROOT"`. After
# that `cd`, a relative "$0" such as `scripts/check.sh` would no longer name the script from a
# caller that started somewhere else.
# Assigned rather than defaulted: a value inherited from somewhere else would name the wrong
# script, and this must be the file that is sourcing the library.
RD_LOCK_SELF="$0"
case "$RD_LOCK_SELF" in
    /*) ;;
    *) RD_LOCK_SELF="$PWD/$RD_LOCK_SELF" ;;
esac

# Forces every workspace crate to be rebuilt from *this* checkout, once per checkout change.
#
# The worktrees share one CARGO_TARGET_DIR and cargo 1.98 fingerprints a workspace crate per
# source path, so a build can link another checkout's rlib for a crate this one also builds. It
# surfaces as a compile error against code that is demonstrably present -- "no method named
# `revoke_all_sessions`" while `facade_ext.rs` defines it -- which reads as a broken branch
# rather than as shared state. That is what makes it expensive: three scripts were fixed one at
# a time on 2026-09-23 (`api-contract.sh`, then `mcp-coverage.sh`, each after it had cost a full
# run) before it was obvious the stamp belongs here, where no script can be written without it.
#
# Deliberately `crates/` only: `plugins/` sources are what build-plugins.sh compares its
# components against, and stamping those would make every one of them look stale for nothing.
# `crates/rd-plugin-api/wit/` is untouched too, since it holds .wit files rather than .rs.
#
# Paid on a checkout change rather than on every run, which is what the marker buys -- and that
# is only sound while the lock is held, since one checkout building at a time is what keeps the
# marker's answer from going stale mid-build.
rd_stamp_sources() {
    local root target marker previous
    root="$(cd "$(dirname "$RD_LOCK_SELF")/.." && pwd)"
    [[ -d "$root/crates" ]] || return 0
    target="${CARGO_TARGET_DIR:-$root/target}"
    marker="$target/.rd-checkout"
    previous="$(cat "$marker" 2> /dev/null || true)"
    [[ "$previous" = "$root" ]] && return 0
    echo "==> stamping crates/: $target last served ${previous:-no checkout}"
    (cd "$root" && find crates -name '*.rs' -exec touch {} +)
    mkdir -p "$target"
    printf '%s\n' "$root" > "$marker"
}

# Waits while less than RD_MIN_FREE_MB of memory is available, so a heavy run does not start
# into a machine that is already short (RD-130-17, "Stabilität vor Tempo").
#
# The lock only keeps these scripts apart. WSL went down twice under cargo, and the load that
# fills memory need not come from here at all — an IDE, a browser, a test instance, a build in
# another project. MemAvailable is the kernel's own estimate of what can be had without
# swapping, which is the question; MemFree leaves out the page cache and would wait for nothing.
# Polled every 5 s, reported every 30 s, given up after RD_LOCK_WAIT seconds like the lock
# itself. A system without /proc/meminfo (macOS) or without the MemAvailable line is not gated.
rd_wait_for_memory() {
    local minimum="${RD_MIN_FREE_MB:-6144}" limit="${RD_LOCK_WAIT:-7200}" available waited=0
    if [[ ! "$minimum" =~ ^(0|[1-9][0-9]*)$ ]]; then
        echo "RD_MIN_FREE_MB must be a whole number of MiB, not '$minimum'" >&2
        exit 2
    fi
    [[ "$minimum" -eq 0 || ! -r /proc/meminfo ]] && return 0
    while :; do
        available="$(awk '/^MemAvailable:/ { print int($2 / 1024) }' /proc/meminfo)"
        [[ -z "$available" || "$available" -ge "$minimum" ]] && return 0
        if [[ "$waited" -ge "$limit" ]]; then
            echo "!! gave up after ${waited}s waiting for memory: ${available} MiB available," >&2
            echo "   RD_MIN_FREE_MB=${minimum} wanted; nothing was run." >&2
            echo "   Free memory first, or lower RD_MIN_FREE_MB (0 switches the gate off)." >&2
            exit 198
        fi
        if (( waited % 30 == 0 )); then
            echo "==> waiting for memory: ${available} MiB available, ${minimum} MiB wanted (RD_MIN_FREE_MB)"
        fi
        sleep 5
        waited=$((waited + 5))
    done
}

rd_take_lock() {
    # The re-executed child arrives here with the lock held; that is where the stamp belongs,
    # because it must happen inside the lock and before the first cargo invocation. The memory
    # gate too: waiting outside the lock would let a run that got in first take what was seen.
    if [[ "${RD_LOCK_HELD:-0}" = 1 ]]; then
        rd_wait_for_memory
        rd_stamp_sources
        return 0
    fi
    # RD_NO_LOCK=1 keeps the stamping the caller's own, as documented above. No memory gate
    # either: `check.sh --defer` sets it precisely because it runs no cargo.
    [[ "${RD_NO_LOCK:-0}" = 1 ]] && return 0

    if ! command -v flock > /dev/null 2>&1; then
        echo "note: flock is not installed, so this run is not serialised" >&2
        rd_wait_for_memory
        return 0
    fi

    local lockfile="${RD_LOCK_FILE:-/tmp/rd-build.lock}"
    # Only to tell the waiting case from the immediate one, so a run that sits still for twenty
    # minutes says why. `flock` creates the file itself.
    flock -n "$lockfile" true 2> /dev/null \
        || echo "==> waiting for $lockfile (another heavy job)"

    export RD_LOCK_HELD=1

    # Not `exec flock …`, although that is the shorter form: on a timeout `flock` exits 1 and
    # prints nothing, so an exec'd run that never started would look exactly like a script that
    # ran and failed. `-E 199` gives the give-up case an exit code of its own, which costs one
    # idle shell per chain and buys a message instead of a silent nothing.
    local status=0
    flock -E 199 -w "${RD_LOCK_WAIT:-7200}" "$lockfile" "$RD_LOCK_SELF" "$@" || status=$?
    if [[ "$status" -eq 199 ]]; then
        echo "!! gave up after ${RD_LOCK_WAIT:-7200}s waiting for $lockfile" >&2
        echo "   another heavy job still holds it; nothing was run." >&2
        echo "   RD_LOCK_WAIT=<seconds> waits longer, RD_NO_LOCK=1 runs without the lock." >&2
    fi
    exit "$status"
}
