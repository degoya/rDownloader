#!/usr/bin/env bash
#
# .claude/hooks/guard-bash.sh (RD-1270-17): the Bash guard refuses the command shapes AGENTS.md
# forbids only where the forbidden word stands in command position, and lets the same words pass
# as an argument, inside a quoted string, in a comment or in a heredoc's text. Each case feeds the
# hook the JSON Claude Code sends and reads whether it answered with a deny decision.
#
# Pure bash and python3, a second. The public export has no .claude/, and the test says so and
# passes there.
#
#   scripts/tests/guard-bash.sh
# The cases are command lines, single-quoted so that nothing in them expands.
# shellcheck disable=SC2016
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOOK="$ROOT/.claude/hooks/guard-bash.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

if [[ ! -f "$HOOK" ]]; then
    echo "skip: no .claude/hooks/ in this tree (the public export)"
    exit 0
fi

# deny|allow for one command line.
decide() {
    local answer
    answer="$(python3 -c 'import json, sys; print(json.dumps({"tool_input": {"command": sys.argv[1]}}))' "$1" \
        | bash "$HOOK")"
    if [[ "$answer" == *'"permissionDecision": "deny"'* ]]; then echo deny; else echo allow; fi
}
denied() { expect "refused: $1" deny "$(decide "$1")"; }
allowed() { expect "passes:  $1" allow "$(decide "$1")"; }

# pkill -f / pgrep -f in command position.
denied 'pkill -f rdownloader'
denied 'pgrep -af "rdownloader serve" && echo running'
denied 'sudo pkill -f rdownloader'
denied 'ps -ef; pgrep --full serve'
denied 'pid=$(pgrep -f watch-run)'
denied 'bash -c "pkill -f rdownloader"'
allowed 'pgrep -x rdownloader | while read -r pid; do kill "$pid"; done'
allowed 'grep -n "pkill -f" AGENTS.md'
allowed 'git log --oneline --grep="pgrep -f"'
allowed 'git commit -m "docs: never pkill -f, it matches its own shell"'
allowed 'echo ok # pkill -f would match this shell'

# flock around a self-locking script.
denied 'flock /tmp/rd-build.lock scripts/check.sh'
denied 'flock -w 10 /tmp/rd-build.lock bash -c "scripts/build-plugins.sh --components-only"'
denied 'cd /x && flock /tmp/rd-build.lock ./scripts/api-contract.sh'
allowed 'flock /tmp/rd-build.lock cargo nextest run -p rd-db'
allowed 'grep -n flock scripts/check.sh scripts/lib/lock.sh'
allowed 'echo "never flock scripts/check.sh"'

# Test output through a filter.
denied 'scripts/check.sh 2>&1 | tail -5'
denied './scripts/check.sh --full | grep FAIL'
denied 'bash scripts/check.sh --preflight | head'
denied 'cargo nextest run -p rd-http | grep -c PASS'
denied 'CARGO_BUILD_JOBS=2 cargo test -p rd-db 2>&1 | tail -20'
denied 'pnpm --dir web test src/a.test.ts | grep fail'
denied 'pnpm run test | rg FAIL'
denied 'npm test | head -3'
denied 'out=$(cargo test 2>&1 | tail -1)'
denied 'timeout 600 scripts/check.sh | tail -3'
allowed 'grep -rn "archive-jobs" scripts/check.sh scripts/lib/*.sh | head -20'
allowed 'git diff scripts/check.sh | head -40'
allowed 'cargo nextest run -p rd-http > /tmp/claude-1000/x/log 2>&1; echo "REAL EXIT: $?"; tail -5 /tmp/claude-1000/x/log'
allowed 'echo "scripts/check.sh | tail is forbidden"'
allowed 'pnpm --dir web install | tail -3'
allowed 'cargo metadata --format-version 1 | head -c 100'

# A heredoc is text, unless it is a script.
allowed "$(printf '%s\n' "cat > /tmp/notes.md <<'EOF'" 'pkill -f rdownloader' 'scripts/check.sh | tail' 'EOF')"
allowed "$(printf '%s\n' 'git commit -F - <<EOF' 'docs: pgrep -f matches itself' 'EOF')"
denied "$(printf '%s\n' "cat > /tmp/claude-1000/x/run.sh <<'EOF'" 'pkill -f rdownloader' 'EOF')"
denied "$(printf '%s\n' "bash <<'EOF'" 'scripts/check.sh | tail' 'EOF')"

# The commit trailer and the signing keys.
denied 'git commit -m "x" -m "Co-Authored-By: someone <a@b>"'
denied 'git -C /x commit -m "co-authored-by: a"'
allowed 'grep -rn "Co-Authored-By" CLAUDE.md'
denied 'cat ~/.config/rdownloader/plugin.key'
denied 'base64 /home/degoya/projects/rdownloader-website/.env'
allowed 'ls ~/.config/rdownloader/'

finish_tests guard-bash
