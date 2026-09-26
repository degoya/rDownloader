# shellcheck shell=bash
#
# The assertions the script tests share (RD-140-22). Sourced, never run: it lives one directory
# down so that check.sh, which runs every `scripts/tests/*.sh`, does not take it for a test.
#
#   source "$ROOT/scripts/tests/lib/expect.sh"
#   expect "a name" "expected" "$(actual)"
#   run_status some command args     # sets $status and leaves the output in $output
#   expect_status "a name" 1         # compares the last run_status
#   expect_output "a name" "needle"  # the last run_status printed this (fixed string)
#   finish_tests <label>             # the summary line; exit 1 when anything failed

failures=0
passed=0
status=0
output=""

expect() {
    local name="$1" expected="$2" actual="$3"
    if [[ "$actual" == "$expected" ]]; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: expected '${expected}', got '${actual}'"
        failures=$((failures + 1))
    fi
}

# Runs a command with errexit off, keeping its exit status and its combined output.
run_status() {
    set +e
    output="$("$@" 2>&1)"
    status=$?
    set -e
}

expect_status() {
    local name="$1" expected="$2"
    if [[ "$status" -eq "$expected" ]]; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: exit $status, expected $expected; output:"
        sed 's/^/       /' <<< "$output"
        failures=$((failures + 1))
    fi
}

expect_output() {
    local name="$1" needle="$2"
    if grep -qF -- "$needle" <<< "$output"; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: output lacks '$needle'; output:"
        sed 's/^/       /' <<< "$output"
        failures=$((failures + 1))
    fi
}

# Whether condition $2 (a shell expression, evaluated) holds.
expect_true() {
    local name="$1"
    if eval "$2"; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: $2"
        failures=$((failures + 1))
    fi
}

finish_tests() {
    echo
    if [[ "$failures" -gt 0 ]]; then
        echo "$1: $failures failed, $passed passed"
        exit 1
    fi
    echo "$1: $passed passed"
}
