#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # RD_RUST_INPUTS_MAP is lib/scope.sh's, which check.sh sources first
#
# The preflight of scripts/check.sh (RD-1110-15): every check that compiles nothing, in minutes,
# each finding recorded and the run going on, so one round shows all of them. 1.10.1's third
# integration round took six `check.sh --full` runs, each stopped at its first red script test or
# finding one new problem an hour in; every one of those problems was in this list.
#
#   scripts/check.sh --preflight
#
# Runs: git diff --check, the job layout, the version copies, the action pins, the plugin release
# notes, the application's release notes, the container's Python tools, cargo fmt, the
# rd-api test map and the Rust test inputs map, gitleaks over the tree the public export would
# publish, bash -n, shellcheck, actionlint, and every script test. No lock (rustfmt writes nothing
# to target/), no green record. The stages are check.sh's own: a --full run goes through the same
# functions, so the preflight is a subset of it, never a second copy.
#
# Who runs it: a wave agent before its report — it compiles nothing, so the no-compile rule
# allows it — and scripts/integrate.sh right after the merges, before the gate, with
# RD_SKIP_JOB_LAYOUT=1: archive-jobs.sh runs there as a generator after the gate, and --full checks
# the layout it leaves.
#
# Expects from the caller: `step`, `skip` and `attempt` (lib/stages.sh), lib/scope.sh,
# lib/script-checks.sh and lib/public.sh sourced, and the working directory at the checkout root.

# The checks that read files only, well under a second together.
rd_file_checks() {
    local boundary="$1"
    step "git diff --check"
    attempt git diff --check "$boundary"

    # The job layout (RD-140-19): a finished job left in docs/roadmap/jobs/, or an open one in its
    # archive/, fails here, whatever the change touched — a status line is edited in a
    # documentation commit, and that is exactly the change that must not leave the file where it
    # was.
    if [[ "${RD_SKIP_JOB_LAYOUT:-0}" == 1 ]]; then
        skip "the job layout" "RD_SKIP_JOB_LAYOUT=1: archive-jobs.sh runs as a generator after this (integrate.sh)"
    else
        step "the job layout: finished jobs archived, open ones not"
        attempt scripts/archive-jobs.sh --check
    fi

    # One version (2026-09-28): Cargo.toml's workspace version is the source, and every copy
    # (web/package.json, the extension manifest, the generated OpenAPI document) must agree.
    step "the version: every copy agrees with Cargo.toml"
    attempt scripts/set-version.sh --check

    # Every action a workflow uses is pinned to a commit (1.8): a moved tag runs other code with
    # the workflow's token.
    step "the workflows: every action pinned to a commit"
    attempt scripts/check-actions-pinned.sh

    # The plugin release notes (RD-1140-03): every bundled plugin's version has its section in
    # its CHANGES.md, short and for users. A section is written with the version raise, and
    # CHANGES.md is documentation (rd_inert_path), so this runs whatever the change touched.
    step "the plugin release notes: every version has its section, short and for users"
    attempt scripts/plugin-release-notes.sh --check

    # The application's release notes (RD-1150-02): every section of RELEASE-NOTES.md short and
    # for users — the update dialog and the GitHub release show them. The version's own section is
    # the docs gate's to demand; here the one in the making may still be a draft.
    step "the release notes: every section short and for users"
    attempt scripts/release-notes.sh --check

    # The container image's Python tools (PIPE-05): docker/requirements.txt is the hashed compile of
    # docker/requirements.in. A change to the .in alone touches no script, so the script tests,
    # which read the real files until 1.19, never saw it at branch level and no workflow checks it.
    step "the container's Python tools: requirements.txt is the compile of requirements.in"
    attempt scripts/docker-tools.sh
}

# The rd-api test map is only as good as its upkeep: a row naming a suite that is gone, a suite
# that no row names, a suite its binary does not declare, a test file outside the binaries. Prints
# them and fails; check.sh stops on it, the preflight records it.
rd_api_map_check() {
    local map="${RD_API_MAP:-scripts/lib/rd-api-tests.map}" problems
    problems="$(rd_api_test_map_problems "$map")"
    if [[ -n "$problems" ]]; then
        printf '!! %s\n' "$problems" >&2
        echo "   Give each rd-api integration suite its row in $map." >&2
        return 1
    fi
    echo "    $(rd_api_test_suites | wc -l) suites in $(rd_api_test_binaries | wc -l) binaries, every one mapped"
}

# The same for the files outside crates/ that Rust tests read (RD-191-09): a path a Rust source
# names without its row would leave a change to it untested at branch level.
rd_rust_inputs_check() {
    if ! python3 scripts/lib/rust-test-inputs.py . "$RD_RUST_INPUTS_MAP" >&2; then
        echo "!! Give each such path its row in $RD_RUST_INPUTS_MAP." >&2
        return 1
    fi
    echo "    every file outside crates/ and plugins/ a Rust test reads has its row"
}

# gitleaks over what the public export would publish: the tracked and the new files of the working
# tree, minus scripts/public-exclude.txt, scanned from inside so that .gitleaks.toml applies — the
# export's own rules (scripts/lib/public.sh), found before the export refuses. The scanner as the
# export finds it: $GITLEAKS, else `gitleaks` on PATH; without one, skipped with a notice.
rd_secret_scan() {
    local scanner="${GITLEAKS:-$(command -v gitleaks || true)}"
    if [[ -z "$scanner" ]]; then
        echo
        echo "==> gitleaks is not installed; skipped (https://github.com/gitleaks/gitleaks/releases)"
        skip "gitleaks" "not installed"
        return 0
    fi
    step "gitleaks over the tree the public export would publish"
    attempt rd_secret_scan_tree "$scanner"
}

rd_secret_scan_tree() {
    local scanner="$1" stage status=0
    stage="$(mktemp -d)"
    git ls-files -z --cached --others --exclude-standard \
        | tar --null --ignore-failed-read -T - -cf - 2> /dev/null | tar -xf - -C "$stage"
    rd_public_exclude "$stage" scripts/public-exclude.txt
    (cd "$stage" && "$scanner" dir . --no-banner --redact --exit-code 1) || status=$?
    rm -rf "$stage"
    [[ "$status" -eq 0 ]] || echo "!! a real secret is removed at the source; a fixture gets an entry in .gitleaks.toml" >&2
    return "$status"
}

# The preflight itself; $1 is the boundary git diff --check compares against.
rd_preflight() {
    rd_file_checks "$1"
    step "cargo fmt --all --check"
    attempt cargo fmt --all --check
    step "the rd-api test map against the test suites"
    attempt rd_api_map_check
    step "the Rust test inputs map against the sources"
    attempt rd_rust_inputs_check
    rd_secret_scan
    rd_script_lint
    rd_workflow_lint
    rd_script_tests
}
