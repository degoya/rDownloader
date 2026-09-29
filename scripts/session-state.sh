#!/usr/bin/env bash
#
# The factual half of a session handoff (RD-140-26): what the repository, its worktrees, the
# build lock, the detached runs and the remotes say right now. Everything here is read, never
# remembered — a handoff that writes these facts down by hand is stale by the next merge. The
# decisions and next steps are the other half; the `handoff` skill writes them to
# /var/tmp/rdownloader-session/HANDOFF.md and points here for the rest.
#
#   branches   development and main against origin (as of the last fetch; no network)
#   worktrees  each against development: ahead/behind, uncommitted paths, its last green
#   greens     the main checkout's branch green (target/.rd-verified/) and --full halves
#              (target/.rd-verified-full/), each against HEAD
#   running    every /tmp/claude-<uid>/*/pid: alive with its run time, ended with its exit
#              file, or gone without one; and who holds /tmp/rd-build.lock
#   tags       the newest local tag; with the network, local tags origin lacks and vice versa
#   runs       open GitHub Actions runs of degoya/rDownloader (`gh`), and the newest finished one
#
# Reads only; takes no lock, so it answers while a chain holds one. Without the network or `gh`
# the remote parts say so and are skipped — never a failure.
#
# Usage:
#   scripts/session-state.sh               # everything
#   scripts/session-state.sh --no-network  # no ls-remote, no gh
#   scripts/session-state.sh --brief       # a few lines, no network — a session's first look
#
# Environment: RD_REPO (this checkout), RD_BASE (development), RD_RUN_ROOT (/tmp/claude-<uid>),
# RD_LOCK_FILE (/tmp/rd-build.lock), RD_GH_REPO (degoya/rDownloader), CARGO_TARGET_DIR
# (<main>/target).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
network=1
brief=0
for arg in "$@"; do
    case "$arg" in
        --no-network) network=0 ;;
        --brief) brief=1; network=0 ;;
        -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

# RD_REPO points it at another repository; its test does that with a scratch one.
REPO="${RD_REPO:-$ROOT}"
MAIN="$(git -C "$REPO" worktree list --porcelain | sed -n '1s/^worktree //p')"
MAIN="${MAIN:-$REPO}"
BASE="${RD_BASE:-development}"
RUN_ROOT="${RD_RUN_ROOT:-/tmp/claude-$(id -u)}"
LOCK_FILE="${RD_LOCK_FILE:-/tmp/rd-build.lock}"
GH_REPO="${RD_GH_REPO:-degoya/rDownloader}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$MAIN/target}"
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"

g() { git -C "$MAIN" "$@" 2> /dev/null; }
short() { printf '%s' "${1:0:8}"; }
stamp() { date -r "$1" '+%Y-%m-%d %H:%M' 2> /dev/null || echo '?'; }

# "3 ahead, 1 behind" of $2 against $1, or nothing when either is missing.
ahead_behind() {
    local counts
    counts="$(g rev-list --left-right --count "$1...$2")" || return 0
    read -r behind ahead <<< "$counts"
    printf '%s ahead, %s behind' "$ahead" "$behind"
}

# The branch green of checkout $1: "at HEAD", "N commit(s) before HEAD" or "none".
branch_green() {
    local root="$1" rev head
    rev="$(rd_verified_revision "$root")"
    [[ -n "$rev" ]] || { echo "none"; return; }
    head="$(git -C "$root" rev-parse HEAD 2> /dev/null)"
    if [[ "$rev" == "$head" ]]; then
        echo "at HEAD ($(stamp "$(rd_verified_marker "$root")"))"
    elif git -C "$root" rev-parse -q --verify "$rev^{commit}" > /dev/null 2>&1; then
        echo "$(git -C "$root" rev-list --count "$rev..HEAD") commit(s) before HEAD ($(stamp "$(rd_verified_marker "$root")"))"
    else
        echo "$(short "$rev"), not in this history"
    fi
}

# The --full halves of checkout $1 against its HEAD tree.
full_green() {
    local root="$1" marker tree half recorded out=()
    marker="$(rd_full_marker "$root")"
    [[ -f "$marker" ]] || { echo "none"; return; }
    tree="$(git -C "$root" rev-parse 'HEAD^{tree}' 2> /dev/null)"
    for half in rust web; do
        recorded="$(sed -n "s/^$half //p" "$marker")"
        if [[ -z "$recorded" ]]; then
            out+=("$half none")
        elif [[ "$recorded" == "$tree" ]]; then
            out+=("$half at HEAD")
        elif rd_tree_docs_only "$root" "$recorded" "$tree"; then
            out+=("$half at HEAD but docs")
        else
            out+=("$half older ($(short "$recorded"))")
        fi
    done
    printf '%s, %s (%s)\n' "${out[0]}" "${out[1]}" "$(stamp "$marker")"
}

# Every worktree as "path<TAB>branch-or-detached".
worktrees() {
    g worktree list --porcelain | awk '
        /^worktree / { if (path != "") print path "\t" ref; path = substr($0, 10); ref = "(detached)" }
        /^branch /   { ref = substr($0, 19) }
        END          { if (path != "") print path "\t" ref }'
}

# Every detached run: "name<TAB>state".
runs() {
    local pid_file dir pid state
    for pid_file in "$RUN_ROOT"/*/pid; do
        [[ -f "$pid_file" ]] || continue
        dir="$(dirname "$pid_file")"
        pid="$(tr -dc '0-9' < "$pid_file")"
        if [[ -n "$pid" ]] && kill -0 "$pid" 2> /dev/null; then
            state="running, pid $pid, $(ps -o etime= -p "$pid" 2> /dev/null | tr -d ' ')"
        elif [[ -f "$dir/exit" ]]; then
            state="ended, exit $(tr -d '[:space:]' < "$dir/exit") ($(stamp "$dir/exit"))"
        else
            state="gone without an exit file, pid ${pid:-?} — it crashed or was killed"
        fi
        printf '%s\t%s\n' "$(basename "$dir")" "$state"
    done
}

lock_state() {
    [[ -e "$LOCK_FILE" ]] || { echo "free (no lock file)"; return; }
    if ! command -v fuser > /dev/null; then
        echo "unknown (no fuser)"
        return
    fi
    local holders
    holders="$(fuser "$LOCK_FILE" 2> /dev/null | tr -s ' ' | sed 's/^ //')"
    if [[ -n "$holders" ]]; then echo "held by pid $holders"; else echo "free"; fi
}

# By version, not by reachability: a release tag sits on main's merge commit, not on development.
latest_tag() { g tag -l 'v*' --sort=-v:refname | sed -n 1p; }

# ---------------------------------------------------------------------------------------------

if [[ "$brief" -eq 1 ]]; then
    wt_total=0 wt_dirty=0 wt_ahead=0
    while IFS=$'\t' read -r path ref; do
        [[ "$path" == "$MAIN" ]] && continue
        wt_total=$((wt_total + 1))
        [[ -n "$(git -C "$path" status --porcelain 2> /dev/null | sed -n 1p)" ]] && wt_dirty=$((wt_dirty + 1))
        [[ "$ref" != "(detached)" && "$(g rev-list --count "$BASE..$ref")" != "0" ]] && wt_ahead=$((wt_ahead + 1))
    done < <(worktrees)
    echo "rDownloader: $(g rev-parse --abbrev-ref HEAD) at $(g rev-parse --short HEAD); $BASE vs origin: $(ahead_behind "origin/$BASE" "$BASE"); latest tag $(latest_tag)"
    echo "worktrees: $wt_total besides the main checkout, $wt_ahead ahead of $BASE, $wt_dirty with uncommitted changes"
    echo "green: branch $(branch_green "$MAIN"); full $(full_green "$MAIN")"
    running="$(runs | awk -F'\t' '$2 ~ /^running/ { print $1 }' | paste -sd' ')"
    echo "build lock: $(lock_state); running: ${running:-none}"
    echo "facts in full: scripts/session-state.sh; decisions: /var/tmp/rdownloader-session/HANDOFF.md"
    exit 0
fi

echo "== rDownloader session state, $(date '+%Y-%m-%d %H:%M') — main checkout $MAIN"

echo
echo "-- branches (against origin as of the last fetch)"
for branch in "$BASE" main; do
    g rev-parse -q --verify "refs/heads/$branch" > /dev/null || continue
    printf '%-12s %s  origin: %s\n' "$branch" "$(g rev-parse --short "$branch")" \
        "$(ahead_behind "origin/$branch" "$branch" || true)"
done
echo "checked out: $(g rev-parse --abbrev-ref HEAD)"

echo
echo "-- worktrees (against $BASE)"
while IFS=$'\t' read -r path ref; do
    if [[ ! -d "$path" ]]; then
        printf '%s  %s  MISSING (git worktree prune)\n' "$path" "$ref"
        continue
    fi
    dirty="$(git -C "$path" status --porcelain 2> /dev/null | wc -l | tr -d ' ')"
    against="-"
    [[ "$ref" != "(detached)" ]] && against="$(ahead_behind "$BASE" "$ref")"
    printf '%s  [%s]\n    %s; %s uncommitted; green: %s\n' "$path" "$ref" "${against:--}" "$dirty" \
        "$(branch_green "$path")"
done < <(worktrees)

echo
echo "-- greens of the main checkout (target: $CARGO_TARGET_DIR)"
echo "branch: $(branch_green "$MAIN")"
echo "full:   $(full_green "$MAIN")"

echo
echo "-- running (runs under $RUN_ROOT)"
listed="$(runs)"
if [[ -n "$listed" ]]; then
    while IFS=$'\t' read -r name state; do printf '%-32s %s\n' "$name" "$state"; done <<< "$listed"
else
    echo "no pid files"
fi
echo "build lock $LOCK_FILE: $(lock_state)"

echo
echo "-- tags"
echo "latest local: $(latest_tag)"
if [[ "$network" -eq 0 ]]; then
    echo "origin: skipped (--no-network)"
elif remote="$(timeout 10 git -C "$MAIN" ls-remote --tags --refs origin 'v*' 2> /dev/null)"; then
    remote="$(sed 's|.*refs/tags/||' <<< "$remote" | sort)"
    local_tags="$(g tag -l 'v*' | sort)"
    only_local="$(comm -23 <(echo "$local_tags") <(echo "$remote") | sed '/^$/d' | paste -sd' ')"
    only_remote="$(comm -13 <(echo "$local_tags") <(echo "$remote") | sed '/^$/d' | paste -sd' ')"
    echo "not on origin: ${only_local:-none}"
    echo "only on origin: ${only_remote:-none}"
else
    echo "origin: unreachable, skipped"
fi

echo
echo "-- GitHub runs ($GH_REPO)"
if [[ "$network" -eq 0 ]]; then
    echo "skipped (--no-network)"
elif ! command -v gh > /dev/null; then
    echo "skipped: gh is not installed"
elif ! json="$(timeout 15 gh run list -R "$GH_REPO" --limit 30 \
        --json databaseId,status,conclusion,workflowName,headBranch,createdAt 2> /dev/null)"; then
    echo "skipped: gh could not reach GitHub (not signed in, or no network)"
else
    python3 - "$json" <<'PY'
import json, sys
runs = json.loads(sys.argv[1] or "[]")
def fmt(r):
    return f"{r['databaseId']}  {r['workflowName']} on {r['headBranch']}, {r['createdAt']}"
open_runs = [r for r in runs if r["status"] != "completed"]
for r in open_runs:
    print(f"{r['status']:12} {fmt(r)}")
if not open_runs:
    print("none open")
done = next((r for r in runs if r["status"] == "completed"), None)
if done:
    print(f"newest finished: {done['conclusion']}  {fmt(done)}")
PY
fi
