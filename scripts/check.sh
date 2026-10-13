#!/usr/bin/env bash
#
# CI-parity checks, scoped to what the change touches, with the parallelism this machine
# survives.
#
# Two levels (RD-120-58): without --full a run is BRANCH level, scoped to what the change touches
# (the reverse-dependency step is ONE level, not a transitive hull; the rd-api suites come from
# scripts/lib/rd-api-tests.map); --full runs everything, once per wave on `development` and in the
# release chain, and a tag and a Windows package refuse a tree without one. The modes:
# scripts/README.md ("check.sh's modes"). Why the scope, the rd-api batches of four and no default
# `clippy --workspace --all-targets` — each an incident that took WSL down — with the measurements:
# docs/architecture.md#build ("Never build every rd-api test binary at once", "Two levels").
#
# Usage:
#   scripts/check.sh                       # branch level: what the change touches
#   scripts/check.sh --full                # everything — wave end on development, release, tag
#   scripts/check.sh --full --again        # ... even when a --full green covers this content,
#                                          # whole or per crate (RD-1150-06)
#   scripts/check.sh --defer               # postpone a triviality; does NOT record a green
#   scripts/check.sh --preflight           # what needs no build, minutes, no lock, no green;
#                                          # seconds when a preflight green covers the tree
#   scripts/check.sh --preflight --again   # ... all of it even then
#   scripts/check.sh --rust                # skip the web half
#   scripts/check.sh --web                 # skip the Rust half
#   scripts/check.sh --clippy rd-scheduler rd-files   # lint what you touched
#   scripts/check.sh --clippy-all          # the whole workspace, at JOBS=2, deliberately
#   scripts/check.sh --windows             # only the Windows lint: cargo xwin clippy, every crate
#   scripts/check.sh --windows --again     # ... even when its green covers this content
#   scripts/check.sh --gate                # integrate.sh's gate: clippy for Linux and Windows,
#                                          # whole workspace, --keep-going, every error in one run
#   JOBS=2 scripts/check.sh                # lower parallelism further
#   TEST_THREADS=16 scripts/check.sh       # run tests wider than the build (scripts/lib/jobs.sh)
#   RD_VITEST_WORKERS=4 scripts/check.sh   # Vitest's workers beside the Rust half (likewise)
#   RD_BASE=main scripts/check.sh          # compare against another base branch
#   RD_CHECK_LOGS=/tmp/x scripts/check.sh  # where the stage logs and `failures` go
#                                          # (default /tmp/claude-<uid>/check-<checkout>)
#
# A failing stage does not end the run (RD-1100-13): tests run with --no-fail-fast, the next
# stage starts, and the run ends non-zero after the last one, with every failure — stage, log,
# failing tests or compiler errors — in $RD_CHECK_LOGS/failures (scripts/lib/stages.sh).
#
# The run serialises itself against every other heavy script through scripts/lib/lock.sh, so
# "check whether something else is running" is no longer anybody's job. RD_NO_LOCK=1 opts out —
# and then the target/ stamp below is yours to take care of.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
# --defer and --preflight build nothing (rustfmt writes nothing to target/), so they do not queue
# behind somebody else's build. Read from "$@" rather than from the parsed flags because the lock
# has to be taken before anything else. Every other run compiles, and a worktree's own web/dist
# would not be what rust-embed serves it (scripts/lib/web-dist.sh, RD-1120-06).
case " $* " in
    *" --defer "*|*" --preflight "*) RD_NO_LOCK=1 ;;
    *)
        # shellcheck source=lib/web-dist.sh
        source "$ROOT/scripts/lib/web-dist.sh"
        rd_web_dist_guard "$ROOT" "scripts/check.sh" || exit 2
        ;;
esac

# Not twice (RD-160-06, RD-1120-06): a --full, --windows or --gate run of content recorded greens
# already cover — the same tree, or one that differs only in what each half does not read,
# recorded by any checkout on this target — records them for this tree and ends, before the lock,
# so it never queues behind somebody else's build; --again runs everything anyway. The rule is
# the one the tag and the Windows package apply (rd_full_gate). Partly covered, a --full runs
# only the rest (scripts/lib/check-reuse.sh), and of a half a green does not cover only what the
# change reaches, per crate and per kind (scripts/lib/check-reuse-crates.sh, RD-1150-06).
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
# shellcheck source=lib/check-reuse.sh
source "$ROOT/scripts/lib/check-reuse.sh"
rd_check_reuse_end() {
    rd_check_reuse_plan "$ROOT" "$@" || return 0
    echo
    echo "==> all requested checks passed"
    exit 0
}
# Asked before the lock, so a covered run never queues, and once more by the run that holds it —
# a green may have been recorded while it waited — whose tree (reuse_tree) is then the run's own.
# Not by the lane stage between the two (lib/lock.sh), which only waits: the plan and its tree
# were asked three times per run, and each tree hashes every file the stamp touched (RD-1270-13).
if [[ "${RD_LOCK_HELD:-0}" != 1 && "${RD_LOCK_STAGE:-}" != lane ]]; then rd_check_reuse_end "$@"; fi
rd_take_lock "$@"
if [[ "${RD_LOCK_HELD:-0}" == 1 ]]; then rd_check_reuse_end "$@"; fi
cd "$ROOT"

# shellcheck source=lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"

# Where cargo actually writes: the main checkout's target/ for a feature worktree, or its own for
# one made with `worktree.sh new --own-target` (scripts/lib/lanes.sh; the lock exported it). Every
# question about built artefacts — the checkout marker, the plugin components — is asked there.
TARGET_DIR="$(rd_target_dir "$ROOT")"
BASE="${RD_BASE:-development}"

# The flags and the run kind (lib/check-args.sh); a usage error ends the run with 2.
# shellcheck source=lib/check-args.sh
source "$ROOT/scripts/lib/check-args.sh"
rd_check_args "$@" || exit 2
CHECK_LOGS="${RD_CHECK_LOGS:-/tmp/claude-$(id -u)/check-$(basename "$ROOT")}"
# shellcheck source=lib/stages.sh
source "$ROOT/scripts/lib/stages.sh"
rd_stages_init
# shellcheck source=lib/lint.sh
source "$ROOT/scripts/lib/lint.sh"
# The Rust tests and the web half, as functions (RD-1100-12 T16).
# shellcheck source=lib/check-tests.sh
source "$ROOT/scripts/lib/check-tests.sh"
# shellcheck source=lib/check-web.sh
source "$ROOT/scripts/lib/check-web.sh"
# The checks that compile nothing, and the preflight that runs them alone (RD-1110-15).
# shellcheck source=lib/script-checks.sh
source "$ROOT/scripts/lib/script-checks.sh"
# shellcheck source=lib/public.sh
source "$ROOT/scripts/lib/public.sh"
# shellcheck source=lib/preflight.sh
source "$ROOT/scripts/lib/preflight.sh"
# shellcheck source=lib/check-record.sh
source "$ROOT/scripts/lib/check-record.sh"

# The Windows lint alone and the integration gate, each a run of its own (scripts/lib/lint.sh).
if [[ "$windows" -eq 1 ]]; then rd_check_windows; fi
if [[ "$gate" -eq 1 ]]; then rd_check_gate; fi

# The preflight (RD-1110-15): every check that compiles nothing, each finding collected, in
# minutes; a wave agent's before its report, integrate.sh's before the gate. It verifies no
# revision; its green is recorded by tree as the half `preflight` (audit C2), and a --full of the
# same content up to the generators' output skips the script lints and tests it ran.
# Not twice (RD-1270-13): with a preflight green of this very tree, or of HEAD's before a version
# bump (lib/preflight-reuse.sh), only the two file checks that read what a tree does not hold run:
# the job layout's environment, and the version copies a bump rewrote (RD-1280-13). --again runs
# everything.
if [[ "$preflight" -eq 1 ]]; then
    preflight_tree="$(rd_worktree_tree "$ROOT")"
    preflight_green=""
    [[ "$again" -eq 1 ]] || preflight_green="$(rd_preflight_green "$ROOT" "$preflight_tree")"
    if [[ -n "$preflight_green" ]]; then
        preflight_reason="a recorded preflight green covers this tree ($([[ "$preflight_green" == "$preflight_tree" ]] \
            && echo 'this very content' || echo "tree ${preflight_green:0:12} before a version bump")); --again runs them"
        rd_file_checks "$(rd_scope_boundary "$BASE" "$ROOT")" "$preflight_reason"
        skip "cargo fmt, the test maps, gitleaks, bash -n, shellcheck, actionlint and the script tests" "$preflight_reason"
    else
        rd_preflight "$(rd_scope_boundary "$BASE" "$ROOT")"
    fi
    rd_stages_report
    rd_stages_exit_if_failed
    [[ -z "$preflight_tree" ]] || rd_record_full "$ROOT" preflight "$preflight_tree"
    echo "==> preflight: recorded for tree ${preflight_tree:0:12}; it verifies no revision, scripts/check.sh is still due"
    echo "==> all requested checks passed"
    exit 0
fi

# The halves of a --full a recorded green covers are not run again (lib/check-reuse.sh); the
# Rust and the web half name the green as their skip reason.
rd_check_reuse_apply
# What a --full shares with the preflight — the file checks, cargo fmt, the test maps, gitleaks
# and the script checks — skips itself when a preflight green covers the tree (RD-1280-13): the
# reason, or nothing (lib/preflight.sh).
preflight_cover="$(rd_preflight_cover_reason)"

# Runs one test selection through nextest, or through cargo test when nextest is absent. Both
# with --no-fail-fast: a run lists every failing test, not the first (RD-1100-13).
# `-j` is nextest's own alias for `--test-threads`, so passing both is an error there. cargo test
# knows no `-E` filterset (the crash matrix's, RD-1120-08): without nextest it runs the whole
# selection instead, which is more, never less.
run_tests() {
    if command -v cargo-nextest > /dev/null; then
        CARGO_BUILD_JOBS="$JOBS" cargo nextest run "$@" --no-fail-fast --test-threads "$TEST_THREADS"
    else
        local args=()
        while [[ $# -gt 0 ]]; do
            if [[ "$1" == -E ]]; then shift; [[ $# -eq 0 ]] || shift; continue; fi
            args+=("$1")
            shift
        done
        CARGO_BUILD_JOBS="$JOBS" cargo test ${args[@]+"${args[@]}"} --no-fail-fast -j "$JOBS"
    fi
}
command -v cargo-nextest > /dev/null || skip "nextest" "cargo-nextest is not installed; cargo test ran instead"

# ---------------------------------------------------------------------------------------------
# What changed
# ---------------------------------------------------------------------------------------------

boundary="$(rd_scope_boundary "$BASE" "$ROOT")"
changed="$(rd_scope_changed "$boundary")"
full_tree=""
if [[ "$full" -eq 1 ]]; then full_tree="${reuse_tree:-$(rd_worktree_tree "$ROOT")}"; fi
verified_before="$(rd_verified_revision "$ROOT")"

touches() { [[ -n "$changed" ]] && grep -qE "$1" <<< "$changed"; }

echo "==> scope"
echo "    level:    $([[ "$full" -eq 1 ]] && echo 'full (--full)' || echo 'branch (--full runs everything)')"
echo "    base:     $BASE"
echo "    boundary: ${boundary:0:12}$([[ "$boundary" == "$verified_before" ]] && echo ' (last green run)' || echo ' (branch point)')"
if [[ -n "$verified_before" ]]; then
    echo "    unverified commits since the last green run: $(rd_unverified_count "$ROOT" "$BASE")"
else
    echo "    no green run has ever been recorded in this checkout"
fi
changed_count=0
[[ -n "$changed" ]] && changed_count="$(wc -l <<< "$changed")"
echo "    changed paths: $changed_count"
# Named rather than counted while the list is short: "the summary says what is in scope" is only
# true if the summary says *which*.
if [[ "$changed_count" -gt 0 && "$changed_count" -le 12 ]]; then
    while read -r path; do echo "      $path"; done <<< "$changed"
fi

# ---------------------------------------------------------------------------------------------
# --defer: postpone a triviality, without letting it slip through
# ---------------------------------------------------------------------------------------------

if [[ "$defer" -eq 1 ]]; then
    # shellcheck source=lib/check-defer.sh
    source "$ROOT/scripts/lib/check-defer.sh"
    rd_check_defer
fi

# ---------------------------------------------------------------------------------------------
# What the change demands
# ---------------------------------------------------------------------------------------------

# shellcheck source=lib/check-scope.sh
source "$ROOT/scripts/lib/check-scope.sh"
rd_check_scope

# A --full that a green covers only in part builds on it per crate and per kind (RD-1150-06): the
# changed members with their whole reverse hull, Vitest alone for translation catalogues. It says
# here what it builds on and what it checks, and lists what it leaves out under "skipped, and why".
# shellcheck source=lib/crate-graph.sh
source "$ROOT/scripts/lib/crate-graph.sh"
# shellcheck source=lib/check-reuse-crates.sh
source "$ROOT/scripts/lib/check-reuse-crates.sh"
rd_check_reuse_crates

# git diff --check, the job layout, the version copies, the action pins, the release notes and the
# container's Python tools (scripts/lib/preflight.sh): files only, well under a second, whatever
# the change touched.
rd_file_checks "$boundary" "$preflight_cover"

# The scripts themselves: bash -n, shellcheck and every test under scripts/tests/ (RD-140-22),
# whenever something under scripts/ changed — a script included, not only its libraries — and
# actionlint whenever .github/ did (RD-191-09). Python and bash, seconds, so no reason to wait
# for the Rust half.
rd_script_checks

# gitleaks over what the public export would publish (RD-1110-15): a second, so --full runs it
# and a secret is found before the export refuses it.
if [[ "$full" -eq 1 ]]; then
    rd_secret_scan "$preflight_cover"
else
    skip "gitleaks" "branch level; --full and --preflight scan the tree"
fi

# The web half beside the Rust half when it builds nothing rust-embed reads (lib/check-web.sh).
rd_check_web_start

# ---------------------------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------------------------

if [[ "$run_rust" -eq 1 ]]; then
    # Seconds, so not worth scoping: a stray .rs anywhere fails the run either way. cargo fmt's red
    # is both halves' (lib/check-record.sh); a problem of the rd-api test map or of the Rust test
    # inputs map — a row naming a suite that is gone, a suite no row names, a path a Rust source
    # reads without its row (RD-191-09) — stops the run here. The preflight runs all three.
    rd_rust_tree_checks "$preflight_cover"
    # Every stage from here on is the Rust half's, whichever path the call above took: a red one
    # holds the rust half's record (lib/check-record.sh).
    stage_half=rust

    # Feature worktrees share this checkout's target/ through CARGO_TARGET_DIR, and cargo
    # fingerprints a workspace crate per source path. The effect is a build that links *another*
    # checkout's rlib for a crate this one also builds, and it surfaces as a compile error
    # against code that is demonstrably present: "no `MirrorHint` in the root" while
    # rd-core/src/collector.rs defines and exports it. It cost three false reds on 2026-09-21
    # alone, in two subagent runs and one of this script's.
    #
    # Touching the sources forces the fingerprint to miss, so every workspace crate is rebuilt
    # from this checkout. Deliberately `crates/` only: `plugins/` sources are what the component
    # staleness check below compares its artefacts against, and stamping those would make every
    # one of the fifty look stale for no reason.
    #
    # Paid on a checkout change rather than on every run, which is what the marker buys. That is
    # only correct together with the lock taken at the top of this script: one checkout builds
    # at a time, so the marker's answer cannot go stale mid-build. Switch the lock off with
    # RD_NO_LOCK=1 and the stamping is yours — `find crates -name '*.rs' -exec touch {} +`.
    step "own sources newer than any foreign artefact"
    marker="$TARGET_DIR/.rd-checkout"
    previous="$(cat "$marker" 2> /dev/null || true)"
    if [[ "$previous" != "$ROOT" ]]; then
        echo "    stamping: $TARGET_DIR last served ${previous:-no checkout}"
        find crates -name '*.rs' -exec touch {} +
        mkdir -p "$TARGET_DIR" && printf '%s\n' "$ROOT" > "$marker"
    else
        echo "    not stamped: $TARGET_DIR already belongs to this checkout"
    fi

    # Resolve only, no compile, so it costs what `cargo metadata` costs. The manifest gate in
    # crates/rd-capture/src/platform_gate/ sees only what that crate declares; this sees what a
    # dependency, a feature or a [patch] pulls in behind an innocent name. CI runs the same
    # script — this is here so the answer arrives before the push rather than after it.
    step "the headless Linux tree of the capture agent"
    attempt scripts/check-capture-linux-tree.sh

    # Only for a run that tests Rust: a scripts-only change has no use for a component, and since
    # the gates build the stale ones themselves (RD-1100-13) asking would cost minutes.
    if [[ "$docs_only" -eq 0 && "$rust_touched" -eq 1 ]]; then
        # shellcheck source=lib/components.sh
        source "$ROOT/scripts/lib/components.sh"
        rd_component_gates
    else
        skip "the plugin component gates" "no Rust test runs"
    fi

    # Branch level lints what it tests: the touched crates, all targets — except rd-api, whose
    # integration binaries are linted only as far as they are selected, never all at once.
    clippy_auto=()
    if [[ "$full" -eq 0 && "$docs_only" -eq 0 && "$rust_touched" -eq 1 ]]; then
        clippy_auto=("${packages[@]+"${packages[@]}"}")
    fi
    if [[ "$clippy_all" -eq 1 ]]; then
        # The Linux half of the gate, so its green is recorded as `clippy` too (audit C8).
        step "clippy over the whole workspace (JOBS=2 — this is the heavy one)"
        rd_lint_recorded clippy "$(rd_worktree_tree "$ROOT")" rd_lint_linux
    elif [[ ${#clippy_crates[@]} -gt 0 ]]; then
        step "clippy on ${clippy_crates[*]}"
        args=()
        for crate in "${clippy_crates[@]}"; do args+=(-p "$crate"); done
        attempt env CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" --all-targets -j "$JOBS" -- -D warnings
    elif [[ ${#clippy_auto[@]} -gt 0 ]]; then
        mapfile -t clippy_auto < <(printf '%s\n' "${clippy_auto[@]}" | sort -u)
        args=()
        names=()
        for crate in "${clippy_auto[@]}"; do
            [[ "$crate" == rd-api ]] || { args+=(-p "$crate"); names+=("$crate"); }
        done
        if [[ ${#args[@]} -gt 0 ]]; then
            step "clippy on the touched crates (${names[*]})"
            attempt env CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" --all-targets -j "$JOBS" -- -D warnings
        fi
        if printf '%s\n' "${clippy_auto[@]}" | grep -qx rd-api; then
            args=(-p rd-api --lib)
            if [[ ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]]; then
                for binary in "${rd_api_binaries[@]+"${rd_api_binaries[@]}"}"; do args+=(--test "$binary"); done
            else
                skip "clippy on rd-api's integration binaries" "every one is selected, and linting all at once is what AGENTS.md forbids"
            fi
            step "clippy on rd-api (library$([[ ${#args[@]} -gt 3 ]] && echo ' and the selected binaries'))"
            attempt env CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" -j "$JOBS" -- -D warnings
        fi
    elif [[ "$full" -eq 1 ]]; then
        # Named as a skip rather than passed over (RD-1100-13): a --full green says nothing about
        # the lint, and reading it as if it did is how a lint finding reaches GitHub.
        skip "clippy" "--full lints nothing; scripts/check.sh --gate (integrate.sh) and CI's rust job lint the workspace, --clippy-all here"
    else
        skip "clippy" "no crate in the change set; --clippy <crates> or --clippy-all lints anyway"
    fi

    if [[ "$docs_only" -eq 1 || "$rust_touched" -eq 0 ]]; then
        reason="the change is documentation only"
        [[ "$docs_only" -eq 1 ]] || reason="nothing the Rust build reads changed"
        skip "rust tests, rd-api, the crash matrix and sqlx" "$reason"
    else
        rd_check_rust_tests
    fi
else
    skip "the whole Rust half" "${rust_skip_reason:---web was given}"
fi

# ---------------------------------------------------------------------------------------------
# Web and extension
# ---------------------------------------------------------------------------------------------

rd_check_web_join

# ---------------------------------------------------------------------------------------------
# What this run did not do, and the green record
# ---------------------------------------------------------------------------------------------

rd_stages_report
if [[ "$full" -eq 0 ]]; then
    echo
    echo "==> branch level: scripts/check.sh --full is still due."
    echo "    It runs everything left out above, and belongs at the end of the wave on"
    echo "    development and in the release chain; a tag and a Windows package refuse a tree"
    echo "    without it."
fi
# The full green is recorded per half and by tree, for the tag, the Windows package and the next
# --full; the tree was taken before the run, so a run that rewrote a tracked file records the
# content it tested. A half whose own stages were green is recorded although another is red
# (RD-1270-22, lib/check-record.sh); a --full also stands for the preflight of its tree (RD-1270-13).
rd_check_record_halves
rd_stages_exit_if_failed

# The green record only moves for a run that covered both halves. Half a run says nothing about
# the other half, and a docs-only conclusion is only meaningful relative to a previous record.
if [[ "$((run_rust | covered_rust))" -eq 1 && "$((run_web | covered_web))" -eq 1 ]]; then
    if [[ "$docs_only" -eq 1 && -z "$verified_before" ]]; then
        echo
        echo "==> green record not moved: this run was documentation only and no earlier run is"
        echo "    on record, so there is nothing this green could be relative to."
    else
        rd_record_verified "$ROOT" "$(git rev-parse HEAD)"
        echo
        echo "==> recorded green at $(git rev-parse --short HEAD) in $(rd_verified_marker "$ROOT")"
    fi
else
    echo
    echo "==> green record not moved: a half run (--rust or --web) does not verify a revision."
fi

echo
echo "==> all requested checks passed"
