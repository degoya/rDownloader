#!/usr/bin/env bash
#
# scripts/session-state.sh (RD-140-26) against a scratch repository: a local bare `origin` with
# one pushed tag, a development branch one commit ahead of it and a local tag origin lacks, a
# worktree ahead and dirty, green markers in a scratch target directory, three detached runs
# (alive, ended with an exit file, gone without one) and a stub `gh`. The output has to state
# each of those facts; without the network, and with a `gh` that cannot reach GitHub, the remote
# parts have to say they were skipped and the run still has to succeed.
#
# Pure bash and git: it runs in a second or two. check.sh runs it when scripts/lib/ or
# scripts/tests/ change, and under --full.
#
#   scripts/tests/session-state.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/session-state.sh"
SCRATCH="$(mktemp -d)"
alive=""
trap '[[ -n "$alive" ]] && kill "$alive" 2> /dev/null; rm -rf "$SCRATCH"' EXIT
REPO="$SCRATCH/repo"
RUNS="$SCRATCH/runs"
BIN="$SCRATCH/bin"

failures=0
passed=0
ok() { echo "ok   $1"; passed=$((passed + 1)); }
fail() { echo "FAIL $1"; failures=$((failures + 1)); }
expect() { if eval "$2"; then ok "$1"; else fail "$1"; fi; }
# shellcheck disable=SC2034  # `status` is read inside the eval of expect()
run() {
    RD_REPO="$REPO" RD_RUN_ROOT="$RUNS" RD_LOCK_FILE="$SCRATCH/no.lock" RD_GH_REPO=o/r \
        CARGO_TARGET_DIR="$SCRATCH/target" PATH="$BIN:$PATH" "$SCRIPT" "$@" > "$SCRATCH/out" 2>&1 \
        && status=0 || status=$?
}
has() { grep -qF -- "$2" "$1"; }
gitc() { git -c user.name=t -c user.email=t@t -c init.defaultBranch=development "$@"; }

gitc init -q --bare "$SCRATCH/origin.git"
gitc init -q "$REPO"
gitc -C "$REPO" commit -q --allow-empty -m one
gitc -C "$REPO" remote add origin "$SCRATCH/origin.git"
gitc -C "$REPO" tag v1.0.0
gitc -C "$REPO" push -q origin development v1.0.0
gitc -C "$REPO" fetch -q origin
gitc -C "$REPO" commit -q --allow-empty -m two
gitc -C "$REPO" tag v1.1.0
gitc -C "$REPO" worktree add -q -b feat/x "$SCRATCH/wt" 2> /dev/null
gitc -C "$SCRATCH/wt" commit -q --allow-empty -m three
echo dirty > "$SCRATCH/wt/new.txt"

head="$(git -C "$REPO" rev-parse HEAD)"
escaped="$(printf '%s' "$REPO" | tr '/' '%')"
mkdir -p "$SCRATCH/target/.rd-verified" "$SCRATCH/target/.rd-verified-full"
echo "$head" > "$SCRATCH/target/.rd-verified/$escaped"
printf 'rust %s\nweb 0000000000000000000000000000000000000000\n' \
    "$(git -C "$REPO" rev-parse 'HEAD^{tree}')" > "$SCRATCH/target/.rd-verified-full/$escaped"

mkdir -p "$RUNS/alive" "$RUNS/ended" "$RUNS/crashed" "$BIN"
sleep 60 & alive=$!
echo "$alive" > "$RUNS/alive/pid"
sleep 0 & gone=$!
wait "$gone"
echo "$gone" > "$RUNS/ended/pid"
echo 0 > "$RUNS/ended/exit"
echo "$gone" > "$RUNS/crashed/pid"

cat > "$BIN/gh" <<'EOF'
#!/usr/bin/env bash
[[ -n "${GH_FAIL:-}" ]] && exit 1
echo '[{"databaseId":7,"status":"in_progress","conclusion":"","workflowName":"CI","headBranch":"main","createdAt":"2026-09-26T10:00:00Z"},
      {"databaseId":6,"status":"completed","conclusion":"success","workflowName":"Release","headBranch":"v1.1.0","createdAt":"2026-09-26T09:00:00Z"}]'
EOF
chmod +x "$BIN/gh"

run
expect "a full run succeeds" '[[ $status -eq 0 ]]'
expect "development is one ahead of origin" 'has "$SCRATCH/out" "origin: 1 ahead, 0 behind"'
expect "the worktree is ahead and dirty" 'has "$SCRATCH/out" "[feat/x]" && has "$SCRATCH/out" "1 ahead, 0 behind; 1 uncommitted; green: none"'
expect "the branch green is at HEAD" 'has "$SCRATCH/out" "branch: at HEAD"'
expect "the full halves are told apart" 'has "$SCRATCH/out" "full:   rust at HEAD, web older (00000000)"'
expect "a live run shows its pid" 'has "$SCRATCH/out" "running, pid $alive"'
expect "an ended run shows its exit" 'has "$SCRATCH/out" "ended, exit 0"'
expect "a run without an exit file is called out" 'has "$SCRATCH/out" "gone without an exit file, pid $gone"'
expect "an absent lock file is free" 'has "$SCRATCH/out" "free (no lock file)"'
expect "the newest tag, and the one origin lacks" 'has "$SCRATCH/out" "latest local: v1.1.0" && has "$SCRATCH/out" "not on origin: v1.1.0"'
expect "open GitHub runs and the newest finished one" 'has "$SCRATCH/out" "in_progress  7  CI on main" && has "$SCRATCH/out" "newest finished: success  6  Release on v1.1.0"'

GH_FAIL=1 run
expect "a gh that cannot reach GitHub is a note, not a failure" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "skipped: gh could not reach GitHub"'

run --no-network
expect "--no-network skips both remote parts" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "origin: skipped (--no-network)" && ! has "$SCRATCH/out" "in_progress"'

run --brief
expect "--brief is five lines" '[[ $status -eq 0 ]] && [[ "$(wc -l < "$SCRATCH/out")" -eq 5 ]]'
expect "--brief carries the counts and the running run" 'has "$SCRATCH/out" "latest tag v1.1.0" && has "$SCRATCH/out" "1 besides the main checkout, 1 ahead of development, 1 with uncommitted changes" && has "$SCRATCH/out" "running: alive"'

echo
echo "$passed passed, $failures failed"
[[ "$failures" -eq 0 ]]
