#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # `changed` and `full` are check.sh's, which sources this file
#
# The checks of scripts/ itself (RD-140-22), kept here so check.sh stays readable: `bash -n` and
# `shellcheck` over every tracked shell script, then every test under scripts/tests/. None of it
# compiles, and all of it together takes seconds.
#
# When: under --full, and whenever anything under scripts/ but its documentation, another tracked
# shell script, or anything under .github/ changed. Until RD-140-22 the tests ran only for scripts/lib/ and
# scripts/tests/, so a change to a script itself — the thing the tests are about — tested nothing. `actionlint` over the workflows runs
# under --full and whenever .github/ or sdk/ci/ changed (RD-191-09, audit K2).
#
# The lint runs at severity `warning`: errors and warnings fail the run, the info and style
# notes (quoting a literal `$` on purpose, `ls` in a pipe) do not. Project-wide settings are in
# scripts/.shellcheckrc. A machine without shellcheck or actionlint skips that one with a notice
# (docs/development.md says how to install both); bash -n and the tests still run. The `scripts`
# job of .github/workflows/ci.yml runs rd_script_lint and rd_workflow_lint with both installed, at
# the versions pinned there, so a lint finding cannot reach development unseen (RD-191-09 T03:
# until then shellcheck ran nowhere, because no machine had it).
#
# Expects from the caller: `step`, `skip`, `full` and `changed`, as check.sh defines them, and
# the working directory at the checkout root. rd_script_lint and rd_workflow_lint need only
# `step` and `skip`.

# bash -n and shellcheck over every tracked shell script: scripts/ and what ships or runs beside
# it (packaging/, docker/, resources/, .claude/hooks/). Not the test fixtures, stand-ins for
# yt-dlp and the like that a test executes (RD-191-09 RA-TOOL-02).
rd_script_lint() {
    local file failed=0
    local -a files
    mapfile -t files < <(git ls-files '*.sh' '*.command' ':(exclude)*/tests/fixtures/*')

    step "bash -n over ${#files[@]} shell scripts"
    for file in "${files[@]}"; do
        bash -n "$file" || failed=1
    done
    [[ "$failed" -eq 0 ]] || { echo "!! a script does not parse (above)" >&2; exit 1; }
    echo "    every one parses"

    if command -v shellcheck > /dev/null; then
        step "shellcheck $(shellcheck --version | sed -n 's/^version: //p') over ${#files[@]} shell scripts (severity warning)"
        shellcheck --severity=warning "${files[@]}"
        echo "    clean"
    else
        echo
        echo "==> shellcheck is not installed; skipped (uv tool install shellcheck-py, or the"
        echo "    distribution's shellcheck package; docs/development.md)"
        skip "shellcheck" "not installed"
    fi
}

# actionlint over .github/workflows/ and the SDK's workflows (sdk/ci/, audit K2); it lints their
# `run:` blocks with shellcheck when that is installed.
rd_workflow_lint() {
    if command -v actionlint > /dev/null; then
        step "actionlint $(actionlint --version | head -n 1) over .github/workflows/ and sdk/ci/"
        actionlint
        actionlint sdk/ci/*.yml
        echo "    clean"
    else
        echo
        echo "==> actionlint is not installed; skipped (docs/development.md)"
        skip "actionlint" "not installed"
    fi
}

rd_script_checks() {
    local test touched workflows
    touched="$(grep -E '^scripts/|\.(sh|command)$' <<< "$changed" | grep -vE '\.md$' || true)"
    workflows="$(grep -E '^(\.github|sdk/ci)/' <<< "$changed" || true)"
    if [[ "$full" -eq 1 || -n "$workflows" ]]; then
        rd_workflow_lint
    else
        skip "actionlint" "nothing under .github/ or sdk/ci/ changed"
    fi
    # The workflows too: scripts/tests/release-assets.sh and workflow-shape.sh read them.
    if [[ "$full" -ne 1 && -z "$touched" && -z "$workflows" ]]; then
        skip "bash -n, shellcheck and the script tests" "no shell script, nothing under scripts/ but documentation and nothing under .github/ changed"
        return 0
    fi
    rd_script_lint

    for test in scripts/tests/*.sh; do
        step "script test: $test"
        "$test"
    done
}
