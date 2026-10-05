#!/usr/bin/env bash
#
# Integrates a wave (RD-140-22): one integration branch, every wave branch merged into it, and
# the one check of the merged tree — the sequence the `wave` skill describes, as a script
# instead of from memory.
#
#   1. The integration branch and its worktree (scripts/worktree.sh new), from --base.
#   2. Each branch merged with --no-ff. After every merge: duplicate migration numbers and
#      plugin ids, which two branches can each get right and still get wrong together. A
#      conflict only in generated files takes our side; any other conflict stops the run. The
#      merge drivers of .gitattributes are registered first (RD-1100-13): CHANGELOG.md merges
#      as the union of both sides, the migration pins as their sorted union, the locale
#      catalogues key by key (scripts/lib/merge-drivers/).
#   3. The preflight (RD-1110-15): scripts/check.sh --preflight, every check that compiles
#      nothing — script tests, lints, formatting, the test maps, the secret scan — into
#      preflight.log, minutes, each finding collected. Red stops the run with all of them in
#      failures, before the gate's compile. Then the gate (RD-1100-13): scripts/check.sh --gate,
#      clippy over the whole workspace for Linux and for Windows, both with --keep-going, into
#      gate.log. Red stops the run before anything is generated, with every error of both
#      platforms in failures — the generators compile too, and the first compile error used to
#      end the run in one of them.
#   4. The generators once, after the last merge (api-contract, mcp-coverage, web-declarations,
#      licenses when a lock file changed, and archive-jobs, which archives finished jobs and recounts the job
#      index), committed as one chore(generated) commit.
#   5. Components: a plugin changed under its old version stops the run (it goes back to its
#      branch); stale or missing ones are built with --components-only.
#   6. scripts/check.sh --windows, then scripts/check.sh --full — the minute first, the hour only
#      after it is green — detached, so an editor crash does not kill them, with their logs, a
#      PID file, a status file and the failure list under /tmp/claude-<uid>/<integration-branch>/.
#      Each ends at once when its green already covers the merged content up to documentation
#      (RD-160-06), so a run again after a documentation fix costs nothing; --full lists every
#      failing test of the run, not the first (scripts/lib/stages.sh).
#   7. After both are green, scripts/prune-target.sh --if-free: the old crate variants go while
#      the wave's own are the newest, and only when no build holds the target's lock (RD-160-06).
#
# Usage:
#   scripts/integrate.sh integration/1.4-w4 feat/a fix/b tooling/c
#   scripts/integrate.sh integration/1.4-w4 feat/a --base integration/1.4-w3
#   scripts/integrate.sh integration/1.4-w4 feat/a --merge-only    # steps 1 and 2
#   scripts/integrate.sh integration/1.4-w4 feat/a --no-check      # steps 1 to 5
#   scripts/integrate.sh integration/1.4-w4 feat/a --no-gate       # step 3 without the gate
#   scripts/integrate.sh integration/1.4-w4 feat/a --no-windows    # no Windows clippy in 6
#
# RD_INTEGRATE_LOGS names another log directory (the tests use it).
#
# Run it again after resolving a conflict or after a fix on a branch: branches already merged are
# skipped, and so is everything that has nothing to do. An integration branch gets no
# branch-level check round of its own — after a fix, this is the round. What it does not do: push, merge into development, or run
# the public CI — that is scripts/public-ci.sh <integration-branch> --platforms linux,windows,
# once the check is green, and its green is what lets the wave into development.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/integrate.sh
source "$ROOT/scripts/lib/integrate.sh"
# The main checkout, also when this runs from a worktree: the integration worktree goes beside it.
common="$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)"
MAIN="$(dirname "$common")"

usage() {
    echo "usage: scripts/integrate.sh <integration-branch> <branch>... [--base <ref>]" >&2
    echo "       [--merge-only | --no-check] [--no-gate] [--no-windows]" >&2
    exit 2
}

base="development"
stop_after="check"
gate=1
windows=1
integration=""
branches=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base) base="${2:?--base needs a ref}"; shift 2 ;;
        --merge-only) stop_after="merge"; shift ;;
        --no-check) stop_after="components"; shift ;;
        --no-gate) gate=0; shift ;;
        --no-windows) windows=0; shift ;;
        -h|--help) sed -n '2,50p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; usage ;;
        *) if [[ -z "$integration" ]]; then integration="$1"; else branches+=("$1"); fi; shift ;;
    esac
done
[[ -n "$integration" && ${#branches[@]} -gt 0 ]] || usage
for branch in "$base" "${branches[@]}"; do
    git -C "$MAIN" rev-parse --verify --quiet "$branch^{commit}" > /dev/null \
        || { echo "no such branch: $branch" >&2; exit 2; }
done

tree="$MAIN-${integration//\//-}"
logs="${RD_INTEGRATE_LOGS:-/tmp/claude-$(id -u)/${integration//\//-}}"

# --- 1. the integration branch ---------------------------------------------------------------
echo "==> $integration in $tree"
if [[ -d "$tree" ]]; then
    [[ "$(git -C "$tree" branch --show-current)" == "$integration" ]] \
        || { echo "$tree is not on $integration" >&2; exit 1; }
    if git -C "$tree" rev-parse --verify --quiet MERGE_HEAD > /dev/null; then
        echo "a merge is in progress in $tree: resolve it and 'git commit --no-edit' first" >&2
        exit 1
    fi
    if [[ -n "$(git -C "$tree" status --porcelain --untracked-files=no)" ]]; then
        echo "$tree has uncommitted changes" >&2
        git -C "$tree" status --short >&2
        exit 1
    fi
    echo "    exists; continuing"
elif git -C "$MAIN" rev-parse --verify --quiet "refs/heads/$integration" > /dev/null; then
    echo "$integration exists but has no worktree at $tree" >&2
    exit 1
else
    BASE="$base" "$MAIN/scripts/worktree.sh" new "$integration" > /dev/null
    echo "    created from $base"
fi

# --- 2. the merges ---------------------------------------------------------------------------
rd_integrate_merge_drivers "$tree" "$ROOT"
echo "==> merging ${#branches[@]} branch(es)"
for branch in "${branches[@]}"; do
    rd_integrate_merge "$tree" "$branch" || exit 1
    if ! problems="$(rd_integrate_duplicates "$tree")"; then
        echo "!! after merging $branch:" >&2
        printf '     %s\n' "$problems" >&2
        echo "   The merge is committed. Renumber on the branch that took the number second, then" >&2
        echo "   'git -C $tree reset --hard HEAD^' and run this again." >&2
        exit 1
    fi
done
echo "    no duplicate migration number, no duplicate plugin id"
[[ "$stop_after" != merge ]] || { echo "==> --merge-only: stopped after the merges"; exit 0; }

cd "$tree"
mkdir -p "$logs"

# --- 3. the preflight and the gate ------------------------------------------------------------
# Both in the foreground: the preflight takes minutes and compiles nothing, the gate compiles what
# the generators after it would compile anyway. check.sh takes the lock itself for the gate and
# skips a half its green already covers. The preflight leaves the job layout to archive-jobs.sh
# below, which rewrites what --check would refuse.
echo "==> the preflight: what compiles nothing, every finding at once ($logs/preflight.log)"
rd_integrate_check "$logs" preflight env RD_SKIP_JOB_LAYOUT=1 scripts/check.sh --preflight || exit 1
if [[ "$gate" -eq 1 ]]; then
    echo "==> the gate: clippy over the workspace for Linux and Windows, --keep-going ($logs/gate.log)"
    rd_integrate_check "$logs" gate scripts/check.sh --gate || exit 1
else
    echo "==> --no-gate: no lint before the generators"
fi

# --- 4. the generators, once -----------------------------------------------------------------
# Everything they write is committed in one commit: the generated files, and the job files
# archive-jobs.sh moved, the links it rewrote and the index it recounted (RD-1100-13; the job
# index conflicted nine times in the 1.9.1 integration). The tree was clean after the merges, so
# what is modified now is theirs.
echo "==> the generators"
scripts/api-contract.sh
scripts/mcp-coverage.sh
scripts/web-declarations.sh
if ! git diff --quiet "$base" HEAD -- Cargo.lock web/pnpm-lock.yaml; then
    scripts/licenses.sh
else
    echo "    licences: neither lock file differs from $base"
fi
scripts/archive-jobs.sh
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
    git add -u
    git diff --cached --stat | sed 's/^/    /'
    git commit --quiet -m "chore(generated): regenerate after merging $(printf '%s ' "${branches[@]}" | sed 's/ $//')"
    echo "    committed $(git rev-parse --short HEAD)"
else
    echo "    nothing to commit"
fi

# --- 5. components ---------------------------------------------------------------------------
echo "==> plugin components"
# Direct cargo calls in this worktree look for components under its own target/ unless
# CARGO_TARGET_DIR says otherwise; the scripts point there by themselves.
if [[ "$tree" != "$MAIN" && ! -e target/wasm32-unknown-unknown && -d "$MAIN/target/wasm32-unknown-unknown" ]]; then
    mkdir -p target
    ln -s "$MAIN/target/wasm32-unknown-unknown" target/wasm32-unknown-unknown
    echo "    linked target/wasm32-unknown-unknown to the main checkout's"
fi
unbumped="$(scripts/build-plugins.sh --list-unbumped)"
if [[ -n "$unbumped" ]]; then
    echo "!! plugins that changed under a version already signed — raise it on their branch:" >&2
    echo "     $(tr '\n' ' ' <<< "$unbumped")" >&2
    exit 1
fi
build="$( (scripts/build-plugins.sh --list-missing; scripts/build-plugins.sh --list-stale) | sort -u)"
if [[ -n "$build" ]]; then
    mapfile -t names <<< "$build"
    echo "    building ${#names[@]}: ${names[*]}"
    scripts/build-plugins.sh --components-only "${names[@]}"
else
    echo "    every component is current"
fi
[[ "$stop_after" != components ]] || { echo "==> --no-check: stopped before the check"; exit 0; }

# --- 6. the check, detached ------------------------------------------------------------------
# The Windows lint first (RD-1100-13): a minute warm, and red there holds back the hour of --full.
rm -f "$logs/status"
{
    echo '#!/usr/bin/env bash'
    echo "cd '$tree'"
    echo "export RD_CHECK_LOGS='$logs'"
    if [[ "$windows" -eq 1 ]]; then
        echo "scripts/check.sh --windows > '$logs/windows.log' 2>&1; windows=\$?"
        echo "echo \"REAL EXIT: \$windows\" >> '$logs/windows.log'"
    else
        echo "windows=skipped"
    fi
    echo "full=skipped"
    echo "if [[ \$windows == 0 || \$windows == skipped ]]; then"
    echo "    scripts/check.sh --full > '$logs/check.log' 2>&1; full=\$?"
    echo "    echo \"REAL EXIT: \$full\" >> '$logs/check.log'"
    echo "fi"
    echo "prune=skipped"
    echo "if [[ \$full == 0 && ( \$windows == 0 || \$windows == skipped ) ]]; then"
    echo "    scripts/prune-target.sh --if-free > '$logs/prune.log' 2>&1; prune=\$?"
    echo "fi"
    echo "echo \"full=\$full windows=\$windows prune=\$prune\" > '$logs/status'"
} > "$logs/run.sh"
chmod +x "$logs/run.sh"
setsid nohup "$logs/run.sh" > /dev/null 2>&1 < /dev/null &
echo "$!" > "$logs/check.pid"
cat <<INFO
==> started, detached: $([[ "$windows" -eq 1 ]] && echo "scripts/check.sh --windows, then --full (only after a green)" || echo "scripts/check.sh --full")
    PID      $(cat "$logs/check.pid") (in $logs/check.pid)
    logs     $([[ "$windows" -eq 1 ]] && echo "$logs/windows.log and check.log" || echo "$logs/check.log")
    failures $logs/failures — every failed stage with its failing tests, written as they fail
    status   $logs/status — written last, as "full=<exit|skipped> windows=<exit|skipped> prune=<exit|skipped>"
             (the prune, into prune.log, only after both are green and when no build holds the lock)
    follow   scripts/watch-run.sh $(cat "$logs/check.pid") $logs/check.log — stage starts, failures, the end

    Judge each log by its closing line "==> all requested checks passed", not by an exit code
    alone. Then: scripts/public-ci.sh $integration --platforms linux,windows
INFO
