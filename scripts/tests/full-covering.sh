#!/usr/bin/env bash
#
# rd_full_covering against many recorded greens (RD-1270-06, rd_full_candidates_load in
# lib/verified.sh): one list of candidates per shell, read again only when a marker changed;
# newest first; at most RD_FULL_CANDIDATES_MAX of them diffed, one git process each and no process
# per path — counted in calls, not in time — with the verdicts of before. And the one rule in its two forms:
# rd_path_read_by against rd_paths_read_by over every tracked path of this checkout, per half.
#
# Pure git and bash, no cargo.
#
#   scripts/tests/full-covering.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=../lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# --- one rule, two forms --------------------------------------------------------------------------
# Every tracked path of this checkout, the generated files and the documentation the preflight
# reads among them, and a few the tree may lack: per half, the paths the predicate reads are the
# ones the pipeline prints.
paths="$({ git -C "$ROOT" ls-files
    printf '%s\n' crates/rd-core/recovery-matrix.md crates/rd-api/mcp-coverage.md docs/roadmap/jobs/x.md \
        plugins/p/CHANGES.md private/site-rules/x.toml .github/readme/x.png dist/plugins/x.rdplug \
        "${RD_GENERATED_FILES[@]}"; } | sort -u)"
rd_read_patterns_load
for half in clippy windows rust web preflight other; do
    piped="$(rd_paths_read_by "$half" <<< "$paths" | sort -u)"
    weighed="$(while IFS= read -r path; do
        if rd_path_read_by "$half" "$path"; then printf '%s\n' "$path"; fi
    done <<< "$paths" | sort -u)"
    if [[ "$piped" == "$weighed" ]]; then
        ok "half $half: rd_path_read_by reads what rd_paths_read_by prints ($(grep -c . <<< "$piped") of $(grep -c . <<< "$paths") paths)"
    else
        fail "half $half: the two forms disagree" "$(diff <(printf '%s\n' "$piped") <(printf '%s\n' "$weighed") || true)"
    fi
done

# --- forty greens -----------------------------------------------------------------------------------
# Forty checkouts, each with a green of its own tree, both halves; each tree differs from the next in
# one Rust source. The markers carry the order they were written in.
repo="$SCRATCH/repo"
git init -q -b development "$repo"
cd "$repo"
mkdir -p crates/rd-x/src docs
echo "start" > docs/readme.md
declare -a trees=()
for i in $(seq 1 40); do
    echo "pub fn v$i() {}" > crates/rd-x/src/lib.rs
    git add -A
    git commit -qm "green $i"
    trees[i]="$(git rev-parse 'HEAD^{tree}')"
    rd_record_full "$SCRATCH/checkout-$i" rust "${trees[i]}"
    rd_record_full "$SCRATCH/checkout-$i" web "${trees[i]}"
    touch -d "@$((1700000000 + i))" "$(rd_full_marker "$SCRATCH/checkout-$i")"
done
# A forty-first checkout with a tree another one recorded: listed once.
rd_record_full "$SCRATCH/checkout-same" rust "${trees[3]}"
touch -d "@1600000000" "$(rd_full_marker "$SCRATCH/checkout-same")"

calls="$SCRATCH/calls"
for command in git ls grep sed cat sort awk tr dirname head; do
    eval "$command() { echo $command >> \"\$calls\"; command $command \"\$@\"; }"
done
count() { command grep -cx "$1" "$calls" || true; }
others() { command grep -cvx git "$calls" || true; }
measure() { : > "$calls"; result="$("$@")"; }

recorded="$(rd_full_recorded "$repo" rust)"
expect "the record: forty trees, each once" "40" "$(grep -c . <<< "$recorded")"
expect "newest first" "${trees[40]} ${trees[39]} ${trees[1]}" \
    "$(sed -n '1p;2p;$p' <<< "$recorded" | paste -sd' ' -)"

# Loaded once in this shell; the questions below share it from their command substitutions.
: > "$calls"
rd_full_candidates_load "$repo"
expect "loading the list: one ls, nothing else" "ls" "$(< "$calls")"

echo "pub fn now() {}" > crates/rd-x/src/lib.rs
now="$(rd_worktree_tree "$repo")"
measure rd_full_covering "$repo" rust "$now"
expect "a Rust change no green covers: not covered, as before" "" "$result"
expect "the cap: $RD_FULL_CANDIDATES_MAX trees diffed of forty" "$RD_FULL_CANDIDATES_MAX" "$(count git)"
expect "one git process per tree, nothing per path, the list not read again" "0" "$(others)"

measure rd_full_covering "$repo" web "$now"
expect "the web half: covered by the newest green" "${trees[40]}" "$result"
expect "after one diff" "1 0" "$(count git) $(others)"

measure rd_full_covering "$repo" rust "${trees[1]}"
expect "the very tree, the oldest: found among all, without a diff" "${trees[1]} 0" "$result $(count git)"

git checkout -q -f "HEAD~2"
echo "a note" > docs/note.md
note="$(rd_worktree_tree "$repo")"
measure rd_full_covering "$repo" rust "$note"
expect "the third newest plus documentation: covered by it" "${trees[38]}" "$result"
expect "after three diffs, newest first" "3 0" "$(count git) $(others)"

git checkout -q -f "$(git rev-list --max-parents=0 HEAD)"
echo "a note" > docs/note.md
oldest="$(rd_worktree_tree "$repo")"
measure rd_full_covering "$repo" rust "$oldest"
expect "the oldest plus documentation, beyond the cap: not covered" "" "$result"
RD_FULL_CANDIDATES_MAX=40 measure rd_full_covering "$repo" rust "$oldest"
expect "within a cap of forty: covered by it" "${trees[1]}" "$result"
rm docs/note.md

# A green recorded since — by another process, so this shell's list is stale: the next question
# reads it again, once.
( rd_record_full "$SCRATCH/checkout-late" rust "$now" )
measure rd_full_covering "$repo" rust "$now"
expect "a green recorded since is found" "$now" "$result"
expect "the changed record read again: one ls" "1" "$(count ls)"
: > "$calls"
rd_full_candidates_load "$repo"
rd_full_covering "$repo" rust "$now" > /dev/null
rd_full_covering "$repo" web "$now" > /dev/null
expect "reloaded here, then shared again: one ls, one diff" "1 1" "$(count ls) $(count git)"

# A record removed: nothing covers.
rm -rf "$CARGO_TARGET_DIR"
measure rd_full_covering "$repo" rust "$now"
expect "no record: not covered" "" "$result"

# Two markers of the same time, written in the order b, a: by name, whatever the clock's resolution.
rd_record_full "$SCRATCH/tie-b" rust "${trees[2]}"
rd_record_full "$SCRATCH/tie-a" rust "${trees[1]}"
touch -d "@1700000000" "$(rd_full_marker "$SCRATCH/tie-b")" "$(rd_full_marker "$SCRATCH/tie-a")"
expect "markers of the same time: by name" "${trees[1]} ${trees[2]}" \
    "$(rd_full_recorded "$repo" rust | paste -sd' ' -)"

finish_tests full-covering
