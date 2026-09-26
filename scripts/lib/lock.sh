#!/usr/bin/env bash
# shellcheck shell=bash
#
# One heavy job per target directory and at most RD_LANES at once, as code rather than as a
# sentence somebody has to remember.
#
# Until RD-120-25 the serialisation existed only as prose in AGENTS.md and in the job files:
# every agent and every person had to check by hand whether another build was running, and
# `grep -rn flock scripts/` found nothing at all. Two full runs in two checkouts against the
# same target/ is how this machine goes into swap.
#
# Since RD-140-06 a run takes two locks, in this order: the lock of the target directory it
# builds in (scripts/lib/lanes.sh — the shared target/ keeps /tmp/rd-build.lock), then a free
# lane of RD_LANES (`/tmp/rd-build.lock.lane<i>`). Every checkout that shares the main target/
# therefore still runs one at a time, as the checkout stamp needs; a worktree with a target of
# its own, or the release chain's Windows lane, runs beside it while a lane is free.
#
# Usage, at the top of a script that builds, tests, lints or packages, after ROOT is known and
# BEFORE the script changes directory or parses its arguments:
#
#     source "$ROOT/scripts/lib/lock.sh"
#     rd_take_lock "$@"
#
# The function does not hold a lock in this shell; it re-runs the script under `flock`, so the
# lock lives exactly as long as that process does and is released even on a kill -9. Always with
# `flock -o`: without it the script inherits the lock's descriptor, and so does every daemon it
# starts — the sccache server outlives the run and would hold the lock until it exits.
#
# Environment:
#   RD_LOCK_FILE   the lock of the shared target/ (default /tmp/rd-build.lock — the file the
#                  job files already name); the lane and own-target locks are named after it
#   RD_LANES       how many runs may build at once, each in its own target directory (default 2)
#   RD_LOCK_WAIT   seconds to wait before giving up (default 7200), for the target's lock, then
#                  for a lane, then for memory
#   RD_MIN_FREE_MB MiB of MemAvailable a run waits for once it holds the lock (default 6144;
#                  0 switches the gate off). A run that gives up exits 198.
#   RD_NO_LOCK=1   run without the lock. Then the target/ stamp of scripts/check.sh is yours to
#                  take care of: that marker (see §5 of RD-120-25) only stamps on a checkout
#                  change, and it is the lock that keeps two checkouts from interleaving.
#   RD_LOCK_HELD   set by this function before the re-exec, so a chain (release.sh → check.sh →
#                  build-plugins.sh) holds ONE lock and nested calls do not deadlock on it. A
#                  chain that starts a step in another lane unsets it for that step.
#   RD_LOCK_NO_SLOT=1  take the target's lock and nothing else — no lane, no memory gate, no
#                  stamp, no sccache: for a run that compiles nothing (prune-target.sh) and must
#                  only keep builds out of the directory it changes.
#   RD_LANE_TARGET_DIR  build in this directory instead of the checkout's target (lanes.sh).
#   RD_LANE_POLL   seconds between two looks for a free lane (default 5; the tests use 1).
#   RD_NO_SCCACHE=1  do not put sccache in front of rustc even where it is installed (RD-140-06).
#   SCCACHE_CACHE_SIZE  the cache's ceiling when sccache is used (default 40G).
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

# shellcheck source=lanes.sh
source "$(dirname "${BASH_SOURCE[0]}")/lanes.sh"

# The checkout the sourcing script belongs to.
rd_lock_root() {
    (cd "$(dirname "$RD_LOCK_SELF")/.." && pwd)
}

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
#
# Skipped for a target directory that has never built a crate: nothing foreign can be in it. That
# is every lane's first run — an own target or the release chain's Windows lane — and from then on
# its marker names its one checkout, so a lane is never stamped at all (RD-140-06).
rd_stamp_sources() {
    local root target marker previous
    root="$(rd_lock_root)"
    [[ -d "$root/crates" ]] || return 0
    target="$(rd_build_dir "$root")"
    marker="$target/.rd-checkout"
    previous="$(cat "$marker" 2> /dev/null || true)"
    [[ "$previous" = "$root" ]] && return 0
    if [[ -z "$previous" ]] && rd_target_is_fresh "$target"; then
        mkdir -p "$target"
        printf '%s\n' "$root" > "$marker"
        return 0
    fi
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

# Puts sccache in front of rustc when it is installed (RD-140-06), so the rebuild a checkout
# change forces is served from a cache instead of compiled again.
#
# The stamp above keeps its job: it is what makes cargo *ask* rustc again for this checkout's
# crates instead of linking another checkout's rlib, and sccache then answers from the cache by
# content -- a touched file whose bytes did not change is a hit. What sccache cannot cache is
# incremental output, so CARGO_INCREMENTAL=0 comes with it; and it never caches a crate that
# links (binaries, proc-macros, cdylibs), which is why the first build after a switch is faster
# rather than free. A RUSTC_WRAPPER the caller already set is theirs and is left alone.
rd_use_sccache() {
    [[ "${RD_NO_SCCACHE:-0}" = 1 || -n "${RUSTC_WRAPPER:-}" ]] && return 0
    local sccache
    sccache="$(command -v sccache 2> /dev/null)" || return 0
    export RUSTC_WRAPPER="$sccache"
    export CARGO_INCREMENTAL=0
    export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-40G}"
    echo "==> sccache in front of rustc ($sccache, cache ceiling $SCCACHE_CACHE_SIZE)"
}

# Exports the checkout's target directory for cargo when nothing else has, so a linked worktree
# builds where its records are read — the main checkout's target/, or its own — without the
# `export CARGO_TARGET_DIR=…` every worktree shell used to need.
rd_export_target_dir() {
    [[ -n "${CARGO_TARGET_DIR:-}" ]] && return 0
    CARGO_TARGET_DIR="$(rd_target_dir "$(rd_lock_root)")"
    export CARGO_TARGET_DIR
}

# Stage two: the target's lock is held by the parent `flock`; take the first free lane and run the
# script under it. `flock -n` either takes a lane at once or exits 196 without running anything;
# which of the two happened is told by the file the script touches the moment it holds its lane,
# not by the code alone, so a script that itself exits 196 is never run twice.
rd_take_lane() {
    unset RD_LOCK_STAGE
    if [[ "${RD_LOCK_NO_SLOT:-0}" = 1 ]]; then
        export RD_LOCK_HELD=1
        exec "$RD_LOCK_SELF" "$@"
    fi
    local lanes lane lockfile started status waited=0 limit="${RD_LOCK_WAIT:-7200}"
    local poll="${RD_LANE_POLL:-5}"
    lanes="$(rd_lanes)" || exit 2
    started="$(mktemp -u "${TMPDIR:-/tmp}/rd-lane-started.XXXXXX")"
    while :; do
        for ((lane = 1; lane <= lanes; lane++)); do
            lockfile="$(rd_lane_lock_file "$lane")"
            status=0
            RD_LOCK_HELD=1 RD_LOCK_STARTED="$started" RD_LOCK_LANE="$lane" \
                flock -n -o -E 196 "$lockfile" "$RD_LOCK_SELF" "$@" || status=$?
            if [[ -e "$started" ]]; then
                rm -f "$started"
                exit "$status"
            fi
            [[ "$status" -eq 196 ]] && continue
            echo "!! the run ended with exit $status before it held lane $lane ($lockfile)" >&2
            exit "$status"
        done
        if [[ "$waited" -ge "$limit" ]]; then
            echo "!! gave up after ${waited}s waiting for a lane: all $lanes busy (RD_LANES)" >&2
            echo "   nothing was run. RD_LOCK_WAIT=<seconds> waits longer." >&2
            exit 199
        fi
        if [[ "$waited" -eq 0 ]]; then
            echo "==> waiting for a lane: all $lanes busy (RD_LANES=$lanes)"
        fi
        sleep "$poll"
        waited=$((waited + poll))
    done
}

rd_take_lock() {
    rd_export_target_dir
    # The re-executed child arrives here with the lock held; that is where the stamp belongs,
    # because it must happen inside the lock and before the first cargo invocation. The memory
    # gate too: waiting outside the lock would let a run that got in first take what was seen.
    # The gate is per lane: MemAvailable already has the other lane's build taken out of it.
    if [[ "${RD_LOCK_HELD:-0}" = 1 ]]; then
        [[ "${RD_LOCK_NO_SLOT:-0}" = 1 ]] && return 0
        if [[ -n "${RD_LOCK_STARTED:-}" ]]; then
            : > "$RD_LOCK_STARTED"
            unset RD_LOCK_STARTED
            echo "==> lane ${RD_LOCK_LANE:-?} of $(rd_lanes 2> /dev/null || echo '?'), building in $(rd_build_dir "$(rd_lock_root)")"
        fi
        rd_wait_for_memory
        rd_stamp_sources
        rd_use_sccache
        return 0
    fi
    # RD_NO_LOCK=1 keeps the stamping the caller's own, as documented above. No memory gate
    # either: `check.sh --defer` sets it precisely because it runs no cargo.
    [[ "${RD_NO_LOCK:-0}" = 1 ]] && return 0

    if ! command -v flock > /dev/null 2>&1; then
        echo "note: flock is not installed, so this run is not serialised" >&2
        rd_wait_for_memory
        rd_use_sccache
        return 0
    fi

    [[ "${RD_LOCK_STAGE:-}" = lane ]] && rd_take_lane "$@"
    rd_lanes > /dev/null || exit 2

    local root build_dir lockfile
    root="$(rd_lock_root)"
    build_dir="$(rd_build_dir "$root")"
    lockfile="$(rd_target_lock_file "$build_dir" "$root")"
    # Only to tell the waiting case from the immediate one, so a run that sits still for twenty
    # minutes says why. `flock` creates the file itself.
    flock -n "$lockfile" true 2> /dev/null \
        || echo "==> waiting for $lockfile (another heavy job in $build_dir)"

    # Not `exec flock …`, although that is the shorter form: on a timeout `flock` exits 1 and
    # prints nothing, so an exec'd run that never started would look exactly like a script that
    # ran and failed. `-E 199` gives the give-up case an exit code of its own, which costs one
    # idle shell per chain and buys a message instead of a silent nothing.
    local status=0
    RD_LOCK_STAGE=lane flock -o -E 199 -w "${RD_LOCK_WAIT:-7200}" "$lockfile" "$RD_LOCK_SELF" "$@" \
        || status=$?
    if [[ "$status" -eq 199 ]]; then
        echo "!! gave up after ${RD_LOCK_WAIT:-7200}s waiting for $lockfile or a lane" >&2
        echo "   another heavy job still holds it; nothing was run." >&2
        echo "   RD_LOCK_WAIT=<seconds> waits longer, RD_NO_LOCK=1 runs without the lock." >&2
    fi
    exit "$status"
}
