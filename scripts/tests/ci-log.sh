#!/usr/bin/env bash
#
# scripts/ci-log.sh against a stub `gh` (RD-140-22): the failed jobs are named, each log is
# stored without its byte-order mark, colour codes and timestamps, and only the lines that say
# what broke are printed — with the context before `##[error]` when nothing else matched.
#
#   scripts/tests/ci-log.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export RD_CI_LOG_DIR="$SCRATCH/ci"
unset RD_PUBLIC_REPO
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
cat > "$SCRATCH/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
args="$*"
case "$args" in
    "run view 42 --repo "*"--json status"*) echo "completed failure https://example.invalid/runs/42 CI" ;;
    "run view 42 --repo "*"--json jobs"*) printf '7\tfailure\ttest (windows-2025)\n8\ttimed_out\tdocker\n' ;;
    "run view 43 --repo "*"--json status"*) echo "completed success https://example.invalid/runs/43 CI" ;;
    "run view 43 --repo "*"--json jobs"*) ;;
    *"actions/jobs/7/logs") cat "$FAKE/log7" ;;
    *"actions/jobs/8/logs") cat "$FAKE/log8" ;;
    *"actions/jobs/7 --jq"*) printf '42\tfailure\ttest (windows-2025)\n' ;;
    *) echo "fake gh: unexpected $args" >&2; exit 1 ;;
esac
EOF
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH"

{
    printf '\xEF\xBB\xBF2026-09-26T10:00:00.1234567Z ##[group]Run cargo nextest\n'
    printf '2026-09-26T10:00:01.0000000Z \x1b[32m    PASS\x1b[0m [   0.1s] rd-core tests::fine\n'
    printf '2026-09-26T10:00:02.0000000Z \x1b[31m    FAIL\x1b[0m [   0.2s] rd-files tests::paths\n'
    printf "2026-09-26T10:00:03.0000000Z thread 'paths' panicked at crates/rd-files/src/lib.rs:9:5\n"
    printf '2026-09-26T10:00:04.0000000Z error[E0425]: cannot find value `x`\n'
    printf '2026-09-26T10:00:05.0000000Z error: unused import: `std::fs`\n'
    printf '2026-09-26T10:00:06.0000000Z ##[error]Process completed with exit code 100.\n'
} > "$FAKE/log7"
{
    for n in $(seq 1 30); do printf '2026-09-26T10:00:00.0000000Z step line %s\n' "$n"; done
    printf '2026-09-26T10:01:00.0000000Z curl: (56) Recv failure\n'
    printf '2026-09-26T10:01:01.0000000Z ##[error]Process completed with exit code 56.\n'
} > "$FAKE/log8"
ci_log() { rm -f "$FAKE/gh.calls"; run_status "$ROOT/scripts/ci-log.sh" "$@"; }

ci_log 42
expect_status "a run with failed jobs" 0
expect_output "names them" "failed jobs: 2"
expect_output "each with its id" "7  failure  test (windows-2025)"
expect_true "asks the public repository by default" 'grep -q -- "--repo degoya/rDownloader" "$FAKE/gh.calls"'
LOG7="$RD_CI_LOG_DIR/42-7.log"
expect_true "stores the log as <run>-<job>.log" '[[ -f "$LOG7" && -f "$RD_CI_LOG_DIR/42-8.log" ]]'
expect_true "without colour codes" '! grep -q $'"'"'\x1b'"'"' "$LOG7"'
expect "without timestamps and byte-order mark" "##[group]Run cargo nextest" "$(head -1 "$LOG7")"
expect_output "prints the FAIL line with its line number" "3:    FAIL [   0.2s] rd-files tests::paths"
expect_output "the panic" "4:thread 'paths' panicked"
expect_output "the compiler error" "5:error[E0425]"
expect_output "the plain error clippy writes" "6:error: unused import"
expect_output "GitHub's error line" "7:##[error]Process completed with exit code 100."
expect_true "but not a passing test" '! grep -q "rd-core tests::fine" <<< "$output"'
expect_true "nor the context meant for a job without such lines" '! grep -q "the 20 lines before" <<< "$(sed -n "/job 7/,/job 8/p" <<< "$output")"'
expect_output "a job without them gets the lines before ##[error]" "(no test or compiler failure; the 20 lines before the first ##[error])"
expect_output "which carry the cause" "31:curl: (56) Recv failure"
expect_true "and no more than those" '! grep -q "^11:step line 11" <<< "$output" && grep -q "^12:step line 12" <<< "$output"'

ci_log https://github.com/other/repo/actions/runs/42
expect_status "a run URL" 0
expect_true "takes the repository from the URL" 'grep -q -- "--repo other/repo" "$FAKE/gh.calls"'

ci_log https://github.com/other/repo/actions/runs/42/job/7
expect_status "a job URL" 0
expect_output "reads that job only" "job 7 of run 42 in other/repo: failure"
expect_true "and fetches no other" '! grep -q "jobs/8/logs" "$FAKE/gh.calls"'

ci_log --job 7
expect_status "--job" 0
expect_output "names the job" "test (windows-2025) (job 7)"

ci_log 43
expect_status "a green run" 0
expect_output "says there is nothing to read" "no failed job"

ci_log
expect_status "no argument is a usage error" 2
ci_log not-a-run
expect_status "neither is a word" 2

finish_tests ci-log
