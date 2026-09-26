#!/usr/bin/env bash
#
# Integrates a wave (RD-140-22): one integration branch, every wave branch merged into it, and
# the one check of the merged tree — the sequence the `wave` skill describes, as a script
# instead of from memory.
#
#   1. The integration branch and its worktree (scripts/worktree.sh new), from --base.
#   2. Each branch merged with --no-ff. After every merge: duplicate migration numbers and
#      plugin ids, which two branches can each get right and still get wrong together. A
#      conflict only in generated files takes our side; any other conflict stops the run.
#   3. The generators once, after the last merge (api-contract, mcp-coverage, and licenses when
#      a lock file changed), committed as one chore(generated) commit.
#   4. Components: a plugin changed under its old version stops the run (it goes back to its
#      branch); stale or missing ones are built with --components-only.
#   5. scripts/check.sh --full, then scripts/check.sh --windows, detached — an editor crash does
#      not kill them — with their logs, a PID file and a status file under
#      /tmp/claude-<uid>/<integration-branch>/.
#
# Usage:
#   scripts/integrate.sh integration/1.4-w4 feat/a fix/b tooling/c
#   scripts/integrate.sh integration/1.4-w4 feat/a --base integration/1.4-w3
#   scripts/integrate.sh integration/1.4-w4 feat/a --merge-only    # steps 1 and 2
#   scripts/integrate.sh integration/1.4-w4 feat/a --no-check      # steps 1 to 4
#   scripts/integrate.sh integration/1.4-w4 feat/a --no-windows    # no Windows clippy in 5
#
# Run it again after resolving a conflict: branches already merged are skipped, and so is
# everything that has nothing to do. What it does not do: push, merge into development, or run
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
    echo "       [--merge-only | --no-check] [--no-windows]" >&2
    exit 2
}

base="development"
stop_after="check"
windows=1
integration=""
branches=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base) base="${2:?--base needs a ref}"; shift 2 ;;
        --merge-only) stop_after="merge"; shift ;;
        --no-check) stop_after="components"; shift ;;
        --no-windows) windows=0; shift ;;
        -h|--help) sed -n '2,29p' "$0"; exit 0 ;;
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
logs="/tmp/claude-$(id -u)/${integration//\//-}"

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

# --- 3. the generators, once -----------------------------------------------------------------
echo "==> the generators"
scripts/api-contract.sh
scripts/mcp-coverage.sh
if ! git diff --quiet "$base" HEAD -- Cargo.lock web/package-lock.json; then
    scripts/licenses.sh
else
    echo "    licences: neither lock file differs from $base"
fi
if [[ -n "$(git status --porcelain --untracked-files=no -- "${RD_GENERATED_FILES[@]}")" ]]; then
    git add -- "${RD_GENERATED_FILES[@]}"
    git commit --quiet -m "chore(generated): regenerate after merging $(printf '%s ' "${branches[@]}" | sed 's/ $//')"
    echo "    committed $(git rev-parse --short HEAD)"
else
    echo "    nothing to commit"
fi

# --- 4. components ---------------------------------------------------------------------------
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

# --- 5. the check, detached ------------------------------------------------------------------
mkdir -p "$logs"
rm -f "$logs/status"
{
    echo '#!/usr/bin/env bash'
    echo "cd '$tree'"
    echo "scripts/check.sh --full > '$logs/check.log' 2>&1; full=\$?"
    echo "echo \"REAL EXIT: \$full\" >> '$logs/check.log'"
    if [[ "$windows" -eq 1 ]]; then
        echo "scripts/check.sh --windows > '$logs/windows.log' 2>&1; windows=\$?"
        echo "echo \"REAL EXIT: \$windows\" >> '$logs/windows.log'"
    else
        echo "windows=skipped"
    fi
    echo "echo \"full=\$full windows=\$windows\" > '$logs/status'"
} > "$logs/run.sh"
chmod +x "$logs/run.sh"
setsid nohup "$logs/run.sh" > /dev/null 2>&1 < /dev/null &
echo "$!" > "$logs/check.pid"
cat <<INFO
==> started, detached: scripts/check.sh --full$([[ "$windows" -eq 1 ]] && echo ", then --windows")
    PID     $(cat "$logs/check.pid") (in $logs/check.pid)
    logs    $logs/check.log$([[ "$windows" -eq 1 ]] && echo " and windows.log")
    status  $logs/status — written last, as "full=<exit> windows=<exit>"

    Judge each log by its closing line "==> all requested checks passed", not by an exit code
    alone. Then: scripts/public-ci.sh $integration --platforms linux,windows
INFO
