#!/usr/bin/env bash
#
# Keeps target/ small: removes the old hash variants cargo leaves behind in every `deps`
# directory, and the incremental caches (RD-140-06).
#
# cargo never collects garbage. Every change of features, flags or sources adds another
# `<crate>-<16 hex>` variant of a crate beside the ones before it, and nothing ever removes them:
# target/debug/deps held 4.28 million entries on 2026-09-23, past the point where ext4 refuses
# every create in the directory and cargo reports "No space left on device" with 300 GB free
# (AGENTS.md). Run this before it gets there; it was a hand-typed loop until now.
#
# The rule: files named `<stem>-<16 hex><anything>` are grouped by their stem — `lib` prefix
# included, so a library's `.rlib`/`.rmeta` and a test binary of the same crate are separate
# groups — and within a stem by the kind of artifact a hash built: its outputs without the `.d`
# and the split debug info beside them (`.rlib`+`.rmeta` of a build, `.rmeta` of a `check`, the
# extensionless test binary), or, with nothing else, by those alone (a `check`'s `.d`, a library
# build's debug info). Per stem and kind the hash whose newest file is the newest is kept, with
# every file of the other hashes removed (`--keep N` keeps the newest N). Until RD-160-06 the
# kind was not looked at, and the `.d` of a `check` run evicted the test binary of the same stem.
# A variant that is still in use (a second feature set, a library's `check` beside its test's)
# is simply rebuilt the next time it is asked for: cargo notices a missing output and compiles
# it again, so this costs time, never correctness.
#
# One more kind of leftover has no hash to group by: the split debug info (`split-debuginfo =
# "unpacked"`) of a crate built without one, which the plugin cdylibs were on the host until 1.12 —
# `<stem>.<codegen unit>.rcgu.dwo`. Every rebuild writes new units and leaves the old ones, 10 765
# of them for one plugin on 2026-09-26. Such a file goes when it is more than an hour older than
# the newest `lib<stem>.*` or `<stem>.*` artifact beside it; with no artifact beside it, it stays.
#
# Never touched: target/wasm32-unknown-unknown/, where the plugin components live and their
# content stamps say whether they are current, and everything outside `deps` and `incremental`.
#
# Lanes (RD-140-06) are target directories of their own and are pruned as such, each under its
# own lock: the checkout's target and the lanes under it (target/lanes/<name>, the release chain's
# Windows build) by default; with --all also the target/ of every worktree made with
# `worktree.sh new --own-target`.
#
# Removing entries does not shrink a directory that is already past the ext4 limit — ext4
# directories never shrink. The script reports each `deps` directory's own size; past 256 MiB it
# says so, and the remedy is AGENTS.md's rename-and-delete.
#
# Never beside a build: run on the shared target while tests ran, it deleted test binaries in
# use, and the shared target was left alone after that. The lock keeps a scripted build out, so
# --if-free prunes a directory only when its lock is free at that moment and otherwise skips it
# with a note and exit 0 — for the end of a wave (scripts/integrate.sh runs it after a green
# check). A bare `cargo` outside the lock is not protected; AGENTS.md wraps those in `flock`.
#
# Usage:
#   scripts/prune-target.sh              # prune (takes the build lock)
#   scripts/prune-target.sh --if-free    # prune what no build holds right now; never wait
#   scripts/prune-target.sh --dry-run    # say what would go, change nothing (lock-free)
#   scripts/prune-target.sh --keep 2     # keep the newest two variants per stem and kind
#   scripts/prune-target.sh --all        # ... and every worktree's own target
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"

dry_run=0
keep=1
all=0
if_free=0
arguments=("$@")
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) dry_run=1; shift ;;
        --all) all=1; shift ;;
        --if-free) if_free=1; shift ;;
        --keep)
            [[ "${2:-}" =~ ^[1-9][0-9]*$ ]] || { echo "--keep wants a positive whole number" >&2; exit 2; }
            keep="$2"; shift 2 ;;
        -h|--help) sed -n '2,53p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

# One target directory per run of this script: the first run lists them and runs itself once per
# directory with RD_PRUNE_ONE naming it.
if [[ -z "${RD_PRUNE_ONE:-}" ]]; then
    if [[ "$all" -eq 1 ]]; then
        targets="$(rd_all_target_dirs "$ROOT")"
    else
        own="$(rd_target_dir "$ROOT")"
        targets="$(rd_all_target_dirs "$ROOT" | awk -v own="$own" '$0 == own || index($0, own "/lanes/") == 1')"
        [[ -n "$targets" ]] || targets="$own"
    fi
    status=0
    while read -r one; do
        [[ -n "$one" ]] || continue
        # A chain that holds the lock itself (RD_LOCK_HELD) keeps builds out already.
        if [[ "$if_free" -eq 1 && "$dry_run" -eq 0 && -z "${RD_LOCK_HELD:-}" ]] \
            && ! flock -n "$(rd_target_lock_file "$one" "$ROOT")" true 2> /dev/null; then
            echo "$one: a build holds its lock; not pruned (--if-free)"
            continue
        fi
        one_status=0
        RD_PRUNE_ONE="$one" "$0" "${arguments[@]}" || one_status=$?
        # 199 is the lock's give-up code: taken between the look above and the run.
        if [[ "$if_free" -eq 1 && "$one_status" -eq 199 ]]; then
            echo "$one: a build took its lock; not pruned (--if-free)"
        elif [[ "$one_status" -ne 0 ]]; then
            status="$one_status"
        fi
    done <<< "$targets"
    exit "$status"
fi

# Deleting under a running build would remove an rlib a link is about to read, so the run holds
# the lock of the directory it prunes — that directory's only, and no lane, since it compiles
# nothing. The dry run only reads, so it stays lock-free like the other queries.
if [[ "$dry_run" -eq 0 ]]; then
    export RD_LANE_TARGET_DIR="$RD_PRUNE_ONE" RD_LOCK_NO_SLOT=1 RD_MIN_FREE_MB=0
    # `flock -w 0` does not wait at all.
    [[ "$if_free" -eq 0 ]] || export RD_LOCK_WAIT=0
    # shellcheck source=lib/lock.sh
    source "$ROOT/scripts/lib/lock.sh"
    rd_take_lock "${arguments[@]}"
fi

target="$RD_PRUNE_ONE"
[[ -d "$target" ]] || { echo "no target directory at $target — nothing to prune"; exit 0; }

python3 - "$target" "$keep" "$dry_run" <<'PY'
import os
import re
import shutil
import sys

target, keep, dry_run = sys.argv[1], int(sys.argv[2]), sys.argv[3] == "1"
VARIANT = re.compile(r"^(?P<stem>.+)-(?P<hash>[0-9a-f]{16})(?P<rest>.*)$")
BESIDE = (".d", ".dwo", ".dwp", ".o", ".pdb", ".dSYM")
UNHASHED = re.compile(r"^(?:lib)?(?P<stem>[^.]+)\.(?P<rest>.+)$")
DWO_MARGIN = 3600
# The components, and the lanes, which are target directories of their own and pruned as such.
SKIP = ("wasm32-unknown-unknown", "lanes")
DIRECTORY_WARNING = 256 * 1024 * 1024


def candidates(name):
    """`deps` and `incremental` of every profile: target/<profile>/ and target/<triple>/<profile>/."""
    found = []
    for first in sorted(os.listdir(target)):
        if first in SKIP:
            continue
        level = os.path.join(target, first)
        if not os.path.isdir(level):
            continue
        if os.path.isdir(os.path.join(level, name)):
            found.append(os.path.join(level, name))
        for second in sorted(os.listdir(level)):
            path = os.path.join(level, second, name)
            if os.path.isdir(path):
                found.append(path)
    return found


def kind(rests):
    """What a variant built: its outputs without the dep-info and debug files beside them, or,
    with nothing else, those (a `check`'s `.d`; a library build's `.d` and debug info)."""
    outputs = {rest for rest in rests if not rest.endswith(BESIDE)}
    if outputs:
        return tuple(sorted(outputs))
    return tuple(sorted({".d" if rest == ".d" else "debug" for rest in rests}))

def human(size):
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return f"{size:.1f} {unit}" if unit != "B" else f"{size} B"
        size /= 1024


total_files = total_bytes = 0
for deps in candidates("deps"):
    groups = {}
    newest_artifact = {}
    unhashed_dwo = []
    with os.scandir(deps) as entries:
        for entry in entries:
            try:
                stat = entry.stat(follow_symlinks=False)
            except FileNotFoundError:
                continue
            match = VARIANT.match(entry.name)
            if not match:
                loose = UNHASHED.match(entry.name)
                if loose and loose["rest"].endswith(".rcgu.dwo"):
                    unhashed_dwo.append((loose["stem"], stat.st_mtime, entry.path, stat.st_size))
                elif loose:
                    stem = loose["stem"]
                    newest_artifact[stem] = max(newest_artifact.get(stem, 0.0), stat.st_mtime)
                continue
            variants = groups.setdefault(match["stem"], {})
            files = variants.setdefault(match["hash"], [0.0, [], set()])
            files[0] = max(files[0], stat.st_mtime)
            files[1].append((entry.path, stat.st_size, entry.is_dir(follow_symlinks=False)))
            files[2].add(match["rest"])
    doomed = []
    for variants in groups.values():
        kinds = {}
        for variant in variants.values():
            kinds.setdefault(kind(variant[2]), []).append(variant)
        for same_kind in kinds.values():
            ordered = sorted(same_kind, key=lambda variant: variant[0], reverse=True)
            for _, files, _ in ordered[keep:]:
                doomed.extend(files)
    for stem, mtime, path, size in unhashed_dwo:
        if stem in newest_artifact and mtime < newest_artifact[stem] - DWO_MARGIN:
            doomed.append((path, size, False))
    files_here = bytes_here = 0
    for path, size, is_directory in doomed:
        files_here += 1
        bytes_here += size
        if dry_run:
            continue
        if is_directory:
            shutil.rmtree(path, ignore_errors=True)
        else:
            try:
                os.remove(path)
            except FileNotFoundError:
                pass
    directory_size = os.stat(deps).st_size
    verb = "would remove" if dry_run else "removed"
    print(f"{deps}: {verb} {files_here} files, {human(bytes_here)}; directory itself {human(directory_size)}")
    if directory_size > DIRECTORY_WARNING:
        print(f"  !! the directory's own blocks are past {human(DIRECTORY_WARNING)} and ext4 never shrinks them;")
        print(f"     recreate it as AGENTS.md describes (mv deps deps.doomed, then rm -rf in the background)")
    total_files += files_here
    total_bytes += bytes_here

for incremental in candidates("incremental"):
    size = 0
    for base, _, names in os.walk(incremental):
        for name in names:
            try:
                size += os.lstat(os.path.join(base, name)).st_size
            except FileNotFoundError:
                pass
    print(f"{incremental}: {'would remove' if dry_run else 'removed'} {human(size)}")
    if not dry_run:
        shutil.rmtree(incremental, ignore_errors=True)
    total_bytes += size

print(f"==> {'would free' if dry_run else 'freed'} {human(total_bytes)} "
      f"({total_files} files: variants past the newest {keep} per stem and kind, and stale split debug info)")
PY
