#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # `changed` and `full` are check.sh's, which sources this file
#
# The checks of scripts/ itself (RD-140-22), kept here so check.sh stays readable: `bash -n` and
# `shellcheck` over every tracked shell script, then every test under scripts/tests/. None of it
# compiles, and all of it together takes seconds.
#
# When: under --full, and whenever anything under scripts/ changed but its documentation. Until
# RD-140-22 the tests ran only for scripts/lib/ and scripts/tests/, so a change to a script
# itself — the thing the tests are about — tested nothing.
#
# The lint runs at severity `warning`: errors and warnings fail the run, the info and style
# notes (quoting a literal `$` on purpose, `ls` in a pipe) do not. Project-wide settings are in
# scripts/.shellcheckrc. A machine without shellcheck skips it with a notice; bash -n and the
# tests still run.
#
# Expects from the caller: `step`, `skip`, `full` and `changed`, as check.sh defines them, and
# the working directory at the checkout root.

rd_script_checks() {
    local file test failed=0 touched
    local -a files
    touched="$(grep -E '^scripts/' <<< "$changed" | grep -vE '\.md$' || true)"
    if [[ "$full" -ne 1 && -z "$touched" ]]; then
        skip "bash -n, shellcheck and the script tests" "nothing under scripts/ changed but documentation"
        return 0
    fi
    mapfile -t files < <(git ls-files 'scripts/*.sh' 'scripts/*.command')

    step "bash -n over ${#files[@]} shell scripts"
    for file in "${files[@]}"; do
        bash -n "$file" || failed=1
    done
    [[ "$failed" -eq 0 ]] || { echo "!! a script does not parse (above)" >&2; exit 1; }
    echo "    every one parses"

    if command -v shellcheck > /dev/null; then
        step "shellcheck over ${#files[@]} shell scripts (severity warning)"
        shellcheck --severity=warning "${files[@]}"
        echo "    clean"
    else
        echo
        echo "==> shellcheck is not installed; skipped (uv tool install shellcheck-py, or the"
        echo "    distribution's shellcheck package)"
        skip "shellcheck" "not installed"
    fi

    for test in scripts/tests/*.sh; do
        step "script test: $test"
        "$test"
    done
}
