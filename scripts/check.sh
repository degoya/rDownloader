#!/usr/bin/env bash
#
# CI-parity checks, scoped to what the change touches, with the parallelism this machine
# survives.
#
# Two levels (RD-120-58). Without --full a run is BRANCH level: clippy and all tests of the
# touched crates, the library and binary tests of one level of reverse dependencies, rd-api's
# library, and only the rd-api integration suites the change needs (scripts/lib/rd-api-tests.map).
# --full runs everything, and belongs at the end of a wave on `development` and in the release
# chain; a tag and a Windows package refuse a tree without one. Nothing is dropped — the branch
# level moves the wide tests to the one run per wave that has to happen anyway. Of eight
# branch checks on 2026-09-24, seven ran all rd-api batches, and every full run after the merges
# was green: the wide branch runs found nothing the full run would not have found.
#
# Three things are deliberately not done the obvious way.
#
#   * A run checks what the change touches, not everything, every time. The honest limit: the
#     reverse-dependency step is ONE level, not a transitive hull.
#   * The rd-api tests run in batches of four binaries. Each of its integration binaries links
#     the whole dependency graph; when there were 55 of them, a plain `--workspace` run built
#     them all at once and OOM-killed WSL even at JOBS=2. Fewer binaries at a time is the fix,
#     and since RD-150-10 there are six (the suites are their modules); a lower job count is
#     not, because it does not make a single link cheaper.
#   * `cargo clippy --workspace --all-targets --all-features` is NOT run by default: it has
#     taken WSL into swap and required a hard restart more than once. Lint what you touched:
#
#   scripts/check.sh --clippy rd-scheduler rd-files
#   scripts/check.sh --clippy-all          # the full run, at JOBS=2, on purpose deliberate
#
# Usage:
#   scripts/check.sh                       # branch level: what the change touches
#   scripts/check.sh --full                # everything — wave end on development, release, tag
#   scripts/check.sh --full --again        # ... even when a --full green covers this content
#   scripts/check.sh --defer               # postpone a triviality; does NOT record a green
#   scripts/check.sh --rust                # skip the web half
#   scripts/check.sh --web                 # skip the Rust half
#   scripts/check.sh --windows             # only the Windows lint: cargo xwin clippy, every crate
#   scripts/check.sh --windows --again     # ... even when its green covers this content
#   scripts/check.sh --gate                # integrate.sh's gate: clippy for Linux and Windows,
#                                          # whole workspace, --keep-going, every error in one run
#   JOBS=2 scripts/check.sh                # lower parallelism further
#   TEST_THREADS=16 scripts/check.sh       # run tests wider than the build (scripts/lib/jobs.sh)
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
# --defer runs no cargo at all, so it does not queue behind somebody else's build. Read from
# "$@" rather than from the parsed flags because the lock has to be taken before anything else.
case " $* " in *" --defer "*) RD_NO_LOCK=1 ;; esac

# Not twice (RD-160-06; owner, 2026-09-28: "unnötige Doppelprüfung immer vermeiden"): a --full run
# of content a --full green already covers — the same tree, or one that differs in documentation
# only, recorded by any checkout on this target — records the green for this tree and ends. The
# rule is the one the tag and the Windows package apply (rd_full_gate). The Windows lint alone
# (--windows) keeps its green the same way, as the half `windows`. Decided before the lock, so it
# never queues behind somebody else's build; --again runs everything anyway.
reuse=0
full_halves=(rust web)
for argument in "$@"; do
    case "$argument" in
        --full) [[ "$reuse" -eq -1 ]] || reuse=1 ;;
        --rust|--web)
            if [[ ${#full_halves[@]} -eq 2 ]]; then full_halves=("${argument#--}"); else reuse=-1; fi ;;
        *) reuse=-1 ;;
    esac
done
if [[ "$*" == "--windows" ]]; then
    reuse=1
    full_halves=(windows)
fi
# The gate's two lints (RD-1100-13) are kept the same way, as the halves `clippy` and `windows`.
if [[ "$*" == "--gate" ]]; then
    reuse=1
    full_halves=(clippy windows)
fi
if [[ "$reuse" -eq 1 ]]; then
    # shellcheck source=lib/verified.sh
    source "$ROOT/scripts/lib/verified.sh"
    if rd_full_already_green "$ROOT" "${full_halves[@]}"; then
        if [[ "${full_halves[*]}" == "rust web" ]]; then
            rd_record_verified "$ROOT" "$(git -C "$ROOT" rev-parse HEAD)"
            echo "==> recorded green at $(git -C "$ROOT" rev-parse --short HEAD) in $(rd_verified_marker "$ROOT")"
        fi
        echo
        echo "==> all requested checks passed"
        exit 0
    fi
fi

rd_take_lock "$@"
cd "$ROOT"

# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
# shellcheck source=lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"

# Where cargo actually writes: the main checkout's target/ for a feature worktree, or its own for
# one made with `worktree.sh new --own-target` (scripts/lib/lanes.sh; the lock exported it). Every
# question about built artefacts — the checkout marker, the plugin components — is asked there.
TARGET_DIR="$(rd_target_dir "$ROOT")"
BASE="${RD_BASE:-development}"

run_rust=1
run_web=1
full=0
defer=0
clippy_crates=()
clippy_all=0
windows=0
gate=0
again=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rust) run_web=0; shift ;;
        --web) run_rust=0; shift ;;
        --full) full=1; shift ;;
        --again) again=1; shift ;;
        --defer) defer=1; shift ;;
        --clippy-all) clippy_all=1; shift ;;
        --windows) windows=1; shift ;;
        --gate) gate=1; shift ;;
        --clippy)
            shift
            while [[ $# -gt 0 && "$1" != --* ]]; do clippy_crates+=("$1"); shift; done
            ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ "$full" -eq 1 && "$defer" -eq 1 ]]; then
    echo "--full and --defer are opposites" >&2
    exit 2
fi
if [[ "$windows" -eq 1 || "$gate" -eq 1 ]] \
    && [[ "$full" -eq 1 || "$defer" -eq 1 || "$clippy_all" -eq 1 || ${#clippy_crates[@]} -gt 0 \
        || "$run_rust" -eq 0 || "$run_web" -eq 0 || "$windows$gate" == 11 ]]; then
    echo "--windows and --gate each run alone; start the other run separately" >&2
    exit 2
fi

# Every stage is timed, skips are listed, failures collected (scripts/lib/stages.sh).
RUN_KIND=branch
if [[ "$full" -eq 1 ]]; then RUN_KIND=full; fi
if [[ "$windows" -eq 1 ]]; then RUN_KIND=windows; fi
if [[ "$gate" -eq 1 ]]; then RUN_KIND=gate; fi
if [[ "$defer" -eq 1 ]]; then RUN_KIND=defer; fi
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

# The Windows half of the workspace (RD-140-23). Nothing else here reads `cfg(windows)` code, and
# v1.3.0 shipped with 42 Windows test failures that only GitHub's runner found. Clippy links
# nothing, so this is minutes (4m40s at -j 2 on 2026-09-25, 76-85 s warm), not the hour a Windows
# test run would be; it is a run of its own because it shares nothing with the Linux scope below.
# Under the lock taken above, which also stamped this checkout's sources. Its green is recorded by
# tree as the half `windows` (RD-160-06), never as a revision: it verifies one platform's lint,
# and a later --windows over content it covers up to documentation ends before the lock.
if [[ "$windows" -eq 1 ]]; then
    windows_tree="$(rd_worktree_tree "$ROOT")"
    step "the Windows lint"
    attempt rd_lint_windows
    rd_stages_report
    rd_stages_exit_if_failed
    if [[ -n "$windows_tree" ]]; then
        rd_record_full "$ROOT" windows "$windows_tree"
        echo "==> recorded the Windows lint's green for tree ${windows_tree:0:12} in $(rd_full_marker "$ROOT")"
    fi
    echo "==> all requested checks passed"
    exit 0
fi

# The integration gate (RD-1100-13): both whole-workspace lints, before scripts/integrate.sh runs
# a generator. The 1.9.1 integration's first run died in api-contract.sh on the first compile
# error, and four fix rounds over 24 files followed; here both lints run with --keep-going and
# the second whatever the first said, so one round shows every error of both platforms. Each
# green is recorded by tree, as the halves `clippy` and `windows`, and a half its green already
# covers up to documentation is not run again (--again runs it anyway).
if [[ "$gate" -eq 1 ]]; then
    gate_tree="$(rd_worktree_tree "$ROOT")"
    for half in clippy windows; do
        if [[ "$again" -eq 0 && -n "$gate_tree" && -n "$(rd_full_covering "$ROOT" "$half" "$gate_tree")" ]]; then
            echo "==> $half: a recorded green covers this content; not run again"
            continue
        fi
        failed_before=${#failed_stages[@]}
        if [[ "$half" == clippy ]]; then
            step "the Linux lint"
            attempt rd_lint_linux
        else
            step "the Windows lint"
            attempt rd_lint_windows
        fi
        if [[ ${#failed_stages[@]} -eq "$failed_before" && -n "$gate_tree" ]]; then
            rd_record_full "$ROOT" "$half" "$gate_tree"
        fi
    done
    rd_stages_report
    rd_stages_exit_if_failed
    [[ -z "$gate_tree" ]] || echo "==> recorded the gate's green for tree ${gate_tree:0:12} in $(rd_full_marker "$ROOT")"
    echo "==> all requested checks passed"
    exit 0
fi

# Runs one test selection through nextest, or through cargo test when nextest is absent. Both
# with --no-fail-fast: a run lists every failing test, not the first (RD-1100-13).
# `-j` is nextest's own alias for `--test-threads`, so passing both is an error there.
run_tests() {
    if command -v cargo-nextest > /dev/null; then
        CARGO_BUILD_JOBS="$JOBS" cargo nextest run "$@" --no-fail-fast --test-threads "$TEST_THREADS"
    else
        CARGO_BUILD_JOBS="$JOBS" cargo test "$@" --no-fail-fast -j "$JOBS"
    fi
}
command -v cargo-nextest > /dev/null || skip "nextest" "cargo-nextest is not installed; cargo test ran instead"

# ---------------------------------------------------------------------------------------------
# What changed
# ---------------------------------------------------------------------------------------------

boundary="$(rd_scope_boundary "$BASE" "$ROOT")"
changed="$(rd_scope_changed "$boundary")"
full_tree=""
if [[ "$full" -eq 1 ]]; then full_tree="$(rd_worktree_tree "$ROOT")"; fi
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

step "git diff --check"
attempt git diff --check "$boundary"

# The scripts themselves: bash -n, shellcheck and every test under scripts/tests/ (RD-140-22),
# whenever something under scripts/ changed — a script included, not only its libraries — and
# actionlint whenever .github/ did (RD-191-09). Python and bash, seconds, so no reason to wait
# for the Rust half.
# shellcheck source=lib/script-checks.sh
source "$ROOT/scripts/lib/script-checks.sh"
rd_script_checks

# The job layout (RD-140-19): a finished job left in docs/roadmap/jobs/, or an open one in its
# archive/, fails here, whatever the change touched — a status line is edited in a documentation
# commit, and that is exactly the change that must not leave the file where it was. Reads files
# only, well under a second.
step "the job layout: finished jobs archived, open ones not"
attempt scripts/archive-jobs.sh --check

# One version (2026-09-28): Cargo.toml's workspace version is the source, and every copy
# (web/package.json, the extension manifest, the generated OpenAPI document) must agree. Reads
# files only.
step "the version: every copy agrees with Cargo.toml"
attempt scripts/set-version.sh --check

# Every action a workflow uses is pinned to a commit (1.8): a moved tag runs other code with the
# workflow's token. Reads files only.
step "the workflows: every action pinned to a commit"
attempt scripts/check-actions-pinned.sh

# ---------------------------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------------------------

if [[ "$run_rust" -eq 1 ]]; then
    # Seconds, so it is not worth scoping: a stray .rs anywhere fails the run either way.
    step "cargo fmt --all --check"
    attempt cargo fmt --all --check

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

    # The rd-api test map is only as good as its upkeep, so its failure modes stop the run here,
    # in a second: a row naming a suite that is gone, a suite that no row names, a suite its
    # binary does not declare, a test file outside the binaries.
    step "the rd-api test map against the test suites"
    map_problems="$(rd_api_test_map_problems "$RD_API_MAP")"
    if [[ -n "$map_problems" ]]; then
        printf '!! %s\n' "$map_problems" >&2
        echo "   Give each rd-api integration suite its row in $RD_API_MAP." >&2
        exit 1
    fi
    echo "    ${#rd_api_all[@]} suites in $(rd_api_test_binaries | wc -l) binaries, every one mapped"

    # The same for the files outside crates/ that Rust tests read (RD-191-09): a path a Rust
    # source names without its row would leave a change to it untested at branch level again.
    step "the Rust test inputs map against the sources"
    if ! python3 scripts/lib/rust-test-inputs.py . "$RD_RUST_INPUTS_MAP" >&2; then
        echo "!! Give each such path its row in $RD_RUST_INPUTS_MAP." >&2
        exit 1
    fi
    echo "    every file outside crates/ and plugins/ a Rust test reads has its row"

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
        step "clippy over the whole workspace (JOBS=2 — this is the heavy one)"
        attempt rd_lint_linux
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
    skip "the whole Rust half" "--web was given"
fi

# ---------------------------------------------------------------------------------------------
# Web and extension
# ---------------------------------------------------------------------------------------------

rd_check_web

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
rd_stages_exit_if_failed

# The green record only moves for a run that covered both halves. Half a run says nothing about
# the other half, and a docs-only conclusion is only meaningful relative to a previous record.
if [[ "$run_rust" -eq 1 && "$run_web" -eq 1 ]]; then
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

# The full green is recorded per half and by tree, for the tag and the Windows package; see
# scripts/lib/verified.sh. The tree was taken before the run, so a run that rewrote a tracked
# file records the content it tested, and the gates then refuse the rewritten one.
if [[ "$full" -eq 1 && -n "$full_tree" ]]; then
    [[ "$run_rust" -eq 1 ]] && rd_record_full "$ROOT" rust "$full_tree"
    [[ "$run_web" -eq 1 ]] && rd_record_full "$ROOT" web "$full_tree"
    echo "==> recorded a --full green for tree ${full_tree:0:12} in $(rd_full_marker "$ROOT")"
fi

echo
echo "==> all requested checks passed"
