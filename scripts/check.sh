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
#   JOBS=2 scripts/check.sh                # lower parallelism further
#   TEST_THREADS=16 scripts/check.sh       # run tests wider than the build (scripts/lib/jobs.sh)
#   RD_BASE=main scripts/check.sh          # compare against another base branch
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
if [[ "$reuse" -eq 1 ]]; then
    # shellcheck source=lib/verified.sh
    source "$ROOT/scripts/lib/verified.sh"
    if rd_full_already_green "$ROOT" "${full_halves[@]}"; then
        if [[ ${#full_halves[@]} -eq 2 ]]; then
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

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rust) run_web=0; shift ;;
        --web) run_rust=0; shift ;;
        --full) full=1; shift ;;
        --again) shift ;;
        --defer) defer=1; shift ;;
        --clippy-all) clippy_all=1; shift ;;
        --windows) windows=1; shift ;;
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

# The Windows half of the workspace (RD-140-23). Nothing else here reads `cfg(windows)` code, and
# v1.3.0 shipped with 42 Windows test failures that only GitHub's runner found. Clippy links
# nothing, so this is minutes (4m40s at -j 2 on 2026-09-25), not the hour a Windows test run
# would be; it is a run of its own because it shares nothing with the Linux scope below. Every
# crate by name — `--` arguments reach the selected packages only — and all targets and
# features, because tests compile differently on Windows (RD-120-67). Under the lock taken above,
# which also stamped this checkout's sources. Its green is recorded by tree as the half `windows`
# (RD-160-06), never as a revision: it verifies one platform's lint, and a later --windows over
# content it covers up to documentation ends before the lock.
if [[ "$windows" -eq 1 ]]; then
    if [[ "$full" -eq 1 || "$defer" -eq 1 || "$clippy_all" -eq 1 || ${#clippy_crates[@]} -gt 0 ]]; then
        echo "--windows runs alone; start the other run separately" >&2
        exit 2
    fi
    cargo xwin --version > /dev/null 2>&1 \
        || { echo "cargo xwin is not installed (cargo install cargo-xwin --version 0.23.1 --locked)" >&2; exit 1; }
    args=()
    for manifest in crates/*/Cargo.toml; do args+=(-p "$(basename "$(dirname "$manifest")")"); done
    echo "==> cargo xwin clippy for x86_64-pc-windows-msvc over $(( ${#args[@]} / 2 )) crates (-j 2)"
    started=$SECONDS
    windows_tree="$(rd_worktree_tree "$ROOT")"
    CARGO_BUILD_JOBS=2 cargo xwin clippy --target x86_64-pc-windows-msvc -j 2 "${args[@]}" \
        --all-targets --all-features -- -D warnings
    echo
    echo "==> the Windows lint took $((SECONDS - started))s"
    if [[ -n "$windows_tree" ]]; then
        rd_record_full "$ROOT" windows "$windows_tree"
        echo "==> recorded the Windows lint's green for tree ${windows_tree:0:12} in $(rd_full_marker "$ROOT")"
    fi
    echo "==> all requested checks passed"
    exit 0
fi

# Every stage is timed, and the table at the end is what a before/after measurement reads.
stage_names=()
stage_seconds=()
stage_name=""
stage_started=0
close_stage() {
    if [[ -n "$stage_name" ]]; then
        stage_names+=("$stage_name")
        stage_seconds+=($((SECONDS - stage_started)))
    fi
    stage_name=""
}
step() { close_stage; stage_name="$*"; stage_started=$SECONDS; echo; echo "==> $*"; }

skipped=()
skip() { skipped+=("$1 — $2"); }

# Runs one test selection through nextest, or through cargo test when nextest is absent.
# `-j` is nextest's own alias for `--test-threads`, so passing both is an error there.
run_tests() {
    if command -v cargo-nextest > /dev/null; then
        CARGO_BUILD_JOBS="$JOBS" cargo nextest run "$@" --test-threads "$TEST_THREADS"
    else
        CARGO_BUILD_JOBS="$JOBS" cargo test "$@" -j "$JOBS"
    fi
}

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
    classes=()
    while read -r path; do
        [[ -n "$path" ]] || continue
        verdict="$(rd_defer_class "$boundary" "$path")"
        if [[ "$verdict" == "no" ]]; then
            echo >&2
            echo "!! --defer refused: $path is not a triviality." >&2
            echo "   Deferrable: docs/ and *.md, web/src/locales/**, web/src/assets/**," >&2
            echo "   and a .vue or .css change that does not touch a <script> block." >&2
            echo "   Run scripts/check.sh without --defer." >&2
            exit 1
        fi
        classes+=("$verdict")
    done <<< "$changed"

    step "git diff --check"
    git diff --check "$boundary"

    if printf '%s\n' "${classes[@]+"${classes[@]}"}" | grep -qx locales; then
        step "the four locale catalogues agree"
        pnpm --dir web test src/i18n/locales.test.ts
    fi
    if printf '%s\n' "${classes[@]+"${classes[@]}"}" | grep -qx appearance; then
        step "pnpm run typecheck:full"
        pnpm --dir web run typecheck:full
    fi

    echo
    echo "==> deferred, not verified"
    echo "    The green record was NOT moved forward. The next ordinary run measures its change"
    echo "    set against ${boundary:0:12} and therefore takes these commits with it."
    echo "    A deferred state is not 'tests pass': scripts/worktree.sh finish and"
    echo "    scripts/release-pipeline.sh refuse it."
    exit 0
fi

# ---------------------------------------------------------------------------------------------
# What the change demands
# ---------------------------------------------------------------------------------------------

# crates/rd-core/recovery-matrix.md is deliberately NOT harmless text: a test compares it
# against rd_core::failpoint::CRASH_POINTS, so editing it is a code change wearing a .md
# extension. crates/rd-api/mcp-coverage.md is the same kind: rd-api's library include_str!s it
# and mcp_coverage::doc_tests compares it with the capability table.
docs_only=0
if [[ -n "$changed" ]] && ! grep -qvE '^docs/|\.md$' <<< "$changed"; then
    docs_only=1
fi
if touches '^crates/rd-core/recovery-matrix\.md$|^crates/rd-api/mcp-coverage\.md$'; then docs_only=0; fi
if [[ "$full" -eq 1 ]]; then docs_only=0; fi

# Whether anything the Rust build reads changed at all. A web-only or scripts-only change
# compiles nothing, so it gets no Rust test — not even rd-api's library.
rust_touched=0
if [[ "$full" -eq 1 ]] || touches '^crates/|^plugins/|^Cargo\.(toml|lock)$|^rust-toolchain\.toml$|\.sql$|^\.config/nextest\.toml$|^deny\.toml$'; then
    rust_touched=1
fi

mapfile -t crate_dirs < <(rd_scope_crate_dirs <<< "$changed")
packages=()
for directory in "${crate_dirs[@]+"${crate_dirs[@]}"}"; do
    name="$(rd_scope_package_name "$directory")"
    if [[ -n "$name" ]]; then packages+=("$name"); fi
done

# A change under plugins/ or to the WIT contract is guest code the contract tests in
# rd-plugin-ext and rd-plugin-host run. Not in the plan's table, added here because without it a
# scoped run of a plugin change would execute no Rust test at all.
if touches '^plugins/|^crates/rd-plugin-api/wit/'; then
    packages+=(rd-plugin-ext rd-plugin-host)
fi

# rd-api's library tests hold the About page's licence list to web/pnpm-lock.yaml and to the
# helper tools' licence texts (RD-130-12), so an npm dependency or a vendor text is a Rust change
# here: without this a new npm package would reach development with no licence entry. Setting
# the flag is enough — every Rust run includes rd-api's library.
if touches '^web/pnpm-lock\.yaml$|^resources/vendor-licenses/'; then
    rust_touched=1
fi

# The whole workspace at branch level too, when the blast radius is everything: the toolchain,
# the nextest or the deny configuration. Until RD-120-58 touching rd-core, rd-db, rd-scheduler or
# rd-files, or more than six crates, did the same; now those get their own tests plus one level
# of reverse dependencies, and the transitive rest waits for --full.
#
# The root Cargo.toml and Cargo.lock widened too until RD-130-17 — on 2026-09-24 once only for
# a new dev-dependency. Now scripts/lib/lock-scope.py names the members whose resolved tree
# changed, and they count as touched; a change it cannot narrow (a profile, a new member, a
# [patch], anything outside [workspace.dependencies]) or a failure to answer still widens.
wide_reason=""
lock_paths=""
governing="$(rd_scope_build_governing <<< "$changed")"
if [[ -n "$governing" ]]; then
    wide_reason="$governing governs the whole build"
elif [[ "$full" -eq 0 ]] && touches '^Cargo\.(toml|lock)$'; then
    lock_scope="$(rd_scope_lock_crates "$boundary")" \
        || lock_scope="WIDE scripts/lib/lock-scope.py could not answer"
    if [[ "$lock_scope" == WIDE\ * ]]; then
        wide_reason="Cargo.toml/Cargo.lock: ${lock_scope#WIDE }"
    else
        echo
        echo "==> Cargo.toml/Cargo.lock: the members whose resolved tree changed"
        plugin_members=0
        while read -r package directory why; do
            [[ -n "$package" ]] || continue
            if [[ "$directory" == plugins/* ]]; then
                # Like a change under plugins/: the contract tests below, added once.
                plugin_members=$((plugin_members + 1))
                continue
            fi
            echo "    $package — $why"
            packages+=("$package")
            # For the rd-api map, the change counts as one to that crate's manifest.
            lock_paths+="$directory/Cargo.toml"$'\n'
        done <<< "$lock_scope"
        if [[ "$plugin_members" -gt 0 ]]; then
            echo "    $plugin_members plugin crates — their contract tests (rd-plugin-ext, rd-plugin-host)"
            packages+=(rd-plugin-ext rd-plugin-host)
        fi
        [[ -n "$lock_scope" ]] || echo "    none — the change resolves to the same tree"
    fi
fi
if [[ "$full" -eq 1 ]]; then wide_reason="--full"; fi

# Branch level: the touched crates in full, and one level of reverse dependencies with their
# library and binary tests only. rd-api is removed from both; it has a rule of its own below.
test_packages=()
dependant_packages=()
if [[ -z "$wide_reason" && ${#packages[@]} -gt 0 ]]; then
    mapfile -t test_packages < <(printf '%s\n' "${packages[@]}" | sort -u | grep -vx rd-api || true)
    mapfile -t dependant_packages < <(printf '%s\n' "${packages[@]}" | rd_scope_reverse_deps \
        | sort -u | grep -vxF -f <(printf '%s\n' "${packages[@]}" rd-api) || true)
fi

# The rd-api integration suites: all of them in a wide run, otherwise what the map demands.
# They run as the binaries holding them (RD-150-10), filtered to them when not all are selected.
RD_API_MAP="scripts/lib/rd-api-tests.map"
mapfile -t rd_api_all < <(rd_api_test_suites | cut -d' ' -f1)
rd_api_selected=()
rd_api_reason=""
if [[ -n "$wide_reason" ]]; then
    rd_api_selected=("${rd_api_all[@]}")
    rd_api_reason="$wide_reason"
elif [[ -n "$changed" ]]; then
    rd_api_demands="$(rd_api_test_demands "$RD_API_MAP" <<< "$changed"$'\n'"$lock_paths")"
    if grep -q '^all ' <<< "$rd_api_demands"; then
        rd_api_selected=("${rd_api_all[@]}")
        rd_api_reason="every suite, for $(grep '^all ' <<< "$rd_api_demands" | head -1 | cut -d' ' -f2-)"
    elif [[ -n "$rd_api_demands" ]]; then
        mapfile -t rd_api_selected < <(cut -d' ' -f1 <<< "$rd_api_demands" | LC_ALL=C sort -u)
        rd_api_reason="mapped from the change ($RD_API_MAP)"
    fi
fi
mapfile -t rd_api_binaries < <(rd_api_test_binaries_of "${rd_api_selected[@]+"${rd_api_selected[@]}"}")
# With nextest, a partial selection runs only the selected suites' tests of those binaries.
rd_api_filter=()
if [[ ${#rd_api_selected[@]} -gt 0 && ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]] \
    && command -v cargo-nextest > /dev/null; then
    rd_api_filter=(-E "$(rd_api_test_filter "${rd_api_selected[@]}")")
fi

failpoints=0
if [[ "$full" -eq 1 ]] \
    || touches '^crates/rd-core/src/failpoint\.rs$|^crates/rd-core/recovery-matrix\.md$' \
    || printf '%s\n' "${packages[@]+"${packages[@]}"}" | grep -qxE 'rd-core|rd-http|rd-scheduler|rd-usenet|rd-object-storage|rd-backup|rd-plugin-host|rd-extract|rd-api-core|rd-torrent|rd-plugin-transfer|rd-api-admin|rd-update'; then
    failpoints=1
fi

sqlx=0
if [[ "$full" -eq 1 ]] || touches '^crates/rd-db/|\.sql$'; then sqlx=1; fi

web_changed=0
if [[ "$full" -eq 1 ]] || touches '^web/'; then web_changed=1; fi

extension_changed=0
if [[ "$full" -eq 1 ]] || touches '^extension/'; then extension_changed=1; fi

step "git diff --check"
git diff --check "$boundary"
echo "    no whitespace damage"

# The scripts themselves: bash -n, shellcheck and every test under scripts/tests/ (RD-140-22),
# whenever something under scripts/ changed — a script included, not only its libraries. Python
# and bash, seconds, so no reason to wait for the Rust half.
# shellcheck source=lib/script-checks.sh
source "$ROOT/scripts/lib/script-checks.sh"
rd_script_checks

# The job layout (RD-140-19): a finished job left in docs/roadmap/jobs/, or an open one in its
# archive/, fails here, whatever the change touched — a status line is edited in a documentation
# commit, and that is exactly the change that must not leave the file where it was. Reads files
# only, well under a second.
step "the job layout: finished jobs archived, open ones not"
scripts/archive-jobs.sh --check

# One version (2026-09-28): Cargo.toml's workspace version is the source, and every copy
# (web/package.json, the extension manifest, the generated OpenAPI document) must agree. Reads
# files only.
step "the version: every copy agrees with Cargo.toml"
scripts/set-version.sh --check

# Every action a workflow uses is pinned to a commit (1.8): a moved tag runs other code with the
# workflow's token. Reads files only.
step "the workflows: every action pinned to a commit"
scripts/check-actions-pinned.sh

# ---------------------------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------------------------

if [[ "$run_rust" -eq 1 ]]; then
    # Seconds, so it is not worth scoping: a stray .rs anywhere fails the run either way.
    step "cargo fmt --all --check"
    cargo fmt --all --check

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
    scripts/check-capture-linux-tree.sh

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

    # shellcheck source=lib/components.sh
    source "$ROOT/scripts/lib/components.sh"
    rd_component_gates

    # Branch level lints what it tests: the touched crates, all targets — except rd-api, whose
    # integration binaries are linted only as far as they are selected, never all at once.
    clippy_auto=()
    if [[ "$full" -eq 0 && "$docs_only" -eq 0 && "$rust_touched" -eq 1 ]]; then
        clippy_auto=("${packages[@]+"${packages[@]}"}")
    fi
    if [[ "$clippy_all" -eq 1 ]]; then
        step "clippy over the whole workspace (JOBS=2 — this is the heavy one)"
        CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings
    elif [[ ${#clippy_crates[@]} -gt 0 ]]; then
        step "clippy on ${clippy_crates[*]}"
        args=()
        for crate in "${clippy_crates[@]}"; do args+=(-p "$crate"); done
        CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" --all-targets -j "$JOBS" -- -D warnings
    elif [[ ${#clippy_auto[@]} -gt 0 ]]; then
        mapfile -t clippy_auto < <(printf '%s\n' "${clippy_auto[@]}" | sort -u)
        args=()
        names=()
        for crate in "${clippy_auto[@]}"; do
            [[ "$crate" == rd-api ]] || { args+=(-p "$crate"); names+=("$crate"); }
        done
        if [[ ${#args[@]} -gt 0 ]]; then
            step "clippy on the touched crates (${names[*]})"
            CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" --all-targets -j "$JOBS" -- -D warnings
        fi
        if printf '%s\n' "${clippy_auto[@]}" | grep -qx rd-api; then
            args=(-p rd-api --lib)
            if [[ ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]]; then
                for binary in "${rd_api_binaries[@]+"${rd_api_binaries[@]}"}"; do args+=(--test "$binary"); done
            else
                skip "clippy on rd-api's integration binaries" "every one is selected, and linting all at once is what AGENTS.md forbids"
            fi
            step "clippy on rd-api (library$([[ ${#args[@]} -gt 3 ]] && echo ' and the selected binaries'))"
            CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" -j "$JOBS" -- -D warnings
        fi
    else
        echo "    (clippy skipped — pass --clippy <crates> or --clippy-all)"
    fi

    if [[ "$docs_only" -eq 1 || "$rust_touched" -eq 0 ]]; then
        reason="the change is documentation only"
        [[ "$docs_only" -eq 1 ]] || reason="nothing the Rust build reads changed"
        skip "rust tests, rd-api, the crash matrix and sqlx" "$reason"
    else
        if [[ -n "$wide_reason" ]]; then
            step "tests (everything except rd-api) — $wide_reason"
            run_tests --workspace --exclude rd-api
        else
            if [[ ${#test_packages[@]} -gt 0 ]]; then
                step "tests (touched: ${test_packages[*]})"
                args=()
                for crate in "${test_packages[@]}"; do args+=(-p "$crate"); done
                run_tests "${args[@]}"
            else
                skip "tests of the touched crates" "no crate under crates/ other than rd-api was touched"
            fi
            if [[ ${#dependant_packages[@]} -gt 0 ]]; then
                step "tests (one level of reverse dependencies, library and binaries: ${#dependant_packages[@]} crates)"
                echo "    ${dependant_packages[*]}"
                # `--lib` fails outright when no selected package has a library (rdownloader,
                # rd-capture), so it is only asked for when one does.
                args=(--bins)
                for crate in "${dependant_packages[@]}"; do
                    args+=(-p "$crate")
                    if [[ -f "crates/$crate/src/lib.rs" ]] || grep -q '^\[lib\]' "crates/$crate/Cargo.toml" 2> /dev/null; then
                        [[ " ${args[*]} " == *" --lib "* ]] || args+=(--lib)
                    fi
                done
                run_tests "${args[@]}"
                skip "integration tests of ${#dependant_packages[@]} reverse dependencies" "branch level"
            fi
        fi

        # rd-api is split out because its integration binaries each link the entire dependency
        # graph, and a plain `--workspace` run builds all of them at once. That has OOM-killed
        # WSL even at JOBS=2: lowering the job count does not make a single link cheaper, so
        # the fix is to build fewer binaries at a time rather than to build them more slowly.
        step "tests (rd-api library)"
        run_tests -p rd-api --lib

        if [[ ${#rd_api_selected[@]} -gt 0 ]]; then
            echo
            echo "==> rd-api integration: ${#rd_api_selected[@]} of ${#rd_api_all[@]} suites in ${#rd_api_binaries[@]} binaries — $rd_api_reason"
            batch=()
            names=()
            batch_index=0
            batch_count=$(( (${#rd_api_binaries[@]} + 3) / 4 ))
            for binary in "${rd_api_binaries[@]}"; do
                batch+=(--test "$binary")
                names+=("$binary")
                # Four binaries per batch: eight entries, each contributing `--test NAME`.
                if [[ ${#batch[@]} -ge 8 ]]; then
                    batch_index=$((batch_index + 1))
                    step "tests (rd-api integration, batch $batch_index of $batch_count: ${names[*]})"
                    run_tests -p rd-api "${batch[@]}" "${rd_api_filter[@]+"${rd_api_filter[@]}"}"
                    batch=()
                    names=()
                fi
            done
            if [[ ${#batch[@]} -gt 0 ]]; then
                batch_index=$((batch_index + 1))
                step "tests (rd-api integration, batch $batch_index of $batch_count: ${names[*]})"
                run_tests -p rd-api "${batch[@]}" "${rd_api_filter[@]+"${rd_api_filter[@]}"}"
            fi
        fi
        if [[ ${#rd_api_selected[@]} -lt ${#rd_api_all[@]} ]]; then
            skip "$(( ${#rd_api_all[@]} - ${#rd_api_selected[@]} )) of ${#rd_api_all[@]} rd-api integration suites" \
                "the change does not map to them ($RD_API_MAP)"
        fi

        if [[ "$failpoints" -eq 1 ]]; then
            step "crash and restart matrix"
            # Off in every other run, including the one above: with the feature disabled the
            # crash points expand to nothing, which is the point. See
            # crates/rd-core/recovery-matrix.md.
            # Every owning crate's own feature, not just rd-core's: each crash-test file is
            # gated on the feature of the crate that owns the point, and rd-core/failpoints does
            # not turn those on — a binary compiled to nothing reports success.
            run_tests --features rd-http/failpoints,rd-scheduler/failpoints,rd-usenet/failpoints,rd-object-storage/failpoints,rd-backup/failpoints,rd-plugin-host/failpoints,rd-extract/failpoints,rd-api-core/failpoints,rd-torrent/failpoints,rd-plugin-transfer/failpoints,rd-update/failpoints \
                -p rd-core -p rd-http -p rd-scheduler -p rd-usenet -p rd-object-storage -p rd-backup -p rd-plugin-host -p rd-extract -p rd-api-core -p rd-torrent -p rd-plugin-transfer -p rd-update
            # The plugin update's two points sit in rd-api-admin and are driven through the
            # admin suite, so only those cases of rd-api run here (RD-180-12).
            run_tests --features rd-api/failpoints -p rd-api --test admin stopped_updates
        else
            skip "crash and restart matrix" "none of rd-core, rd-http, rd-scheduler, rd-usenet, rd-object-storage, rd-backup, rd-plugin-host, rd-extract, rd-api-core, rd-torrent, rd-plugin-transfer, rd-api-admin, rd-update, failpoint.rs or the recovery matrix changed"
        fi

        if [[ "$sqlx" -eq 1 ]]; then
            step "sqlx offline data"
            if cargo sqlx --version > /dev/null 2>&1; then
                # sqlx-cli 0.9 wants a database URL even offline; see .github/workflows/ci.yml.
                SQLX_OFFLINE=true DATABASE_URL="${DATABASE_URL:-sqlite::memory:}" \
                    cargo sqlx prepare --check --workspace
            else
                echo "    sqlx-cli not installed; skipping (install: cargo install sqlx-cli --version 0.9.0 \\"
                echo "      --locked --no-default-features --features sqlite-unbundled)"
            fi
        else
            skip "sqlx offline data" "no rd-db source and no .sql file changed"
        fi
    fi
else
    skip "the whole Rust half" "--web was given"
fi

# ---------------------------------------------------------------------------------------------
# Web and extension
# ---------------------------------------------------------------------------------------------

if [[ "$run_web" -eq 1 && "$web_changed" -eq 1 ]]; then
    # Non-incremental on every run, the one CI and the release chain run: the incremental
    # `typecheck` trusts web/tsconfig.*.tsbuildinfo, and two type errors it passed reached
    # GitHub on 2026-09-27 (RD-150-22).
    step "pnpm run typecheck:full"
    pnpm --dir web run typecheck:full
    step "pnpm run test"
    pnpm --dir web run test
    # Refused rather than run-and-warn. A feature worktree links web/dist to the main checkout's
    # (scripts/worktree.sh), and a build would empty and rewrite that one. Its own
    # web/node_modules (RD-150-14) is no longer a hazard: until 1.5 that was a link too, and the
    # unplugin generators wrote the other checkout's paths into the tracked declarations.
    if [[ -L web/dist ]]; then
        skip "pnpm run build" "this is a feature worktree — web/dist is a symlink"
        echo
        echo "    pnpm run build is refused here: it would write into the main checkout's web/dist."
        echo "    'rm web/dist' (the link only) first to build in this worktree."
    elif scripts/web-dist-stale.sh > /dev/null; then
        skip "pnpm run build" "web/dist is newer than every source that goes into it"
    else
        step "pnpm run build"
        pnpm --dir web run build
        # Belt and braces: these are generated and tracked, so a surprise diff is worth naming
        # even outside a worktree — it usually means the component inventory really did change
        # and the regenerated files belong in the commit.
        if ! git diff --quiet -- web/components.d.ts web/auto-imports.d.ts; then
            echo
            echo "!! web/components.d.ts or web/auto-imports.d.ts changed — review and commit them." >&2
        fi
    fi
elif [[ "$run_web" -eq 1 ]]; then
    skip "typecheck, vitest and the web build" "nothing under web/ changed"
fi

if [[ "$run_web" -eq 1 && "$extension_changed" -eq 1 ]]; then
    step "browser extension"
    scripts/build-extension.sh
elif [[ "$run_web" -eq 1 ]]; then
    skip "the browser extension" "nothing under extension/ changed"
fi
if [[ "$run_web" -eq 0 ]]; then skip "the whole web half" "--rust was given"; fi

# ---------------------------------------------------------------------------------------------
# What this run did not do, and the green record
# ---------------------------------------------------------------------------------------------

close_stage
echo
echo "==> time per stage"
total=0
for index in "${!stage_names[@]}"; do
    printf '    %5ds  %s\n' "${stage_seconds[$index]}" "${stage_names[$index]}"
    total=$((total + stage_seconds[index]))
done
printf '    %5ds  total (all stages)\n' "$total"

echo
if [[ ${#skipped[@]} -eq 0 ]]; then
    echo "==> nothing was skipped"
else
    echo "==> skipped, and why"
    for line in "${skipped[@]}"; do echo "    - $line"; done
fi
if [[ "$full" -eq 0 ]]; then
    echo
    echo "==> branch level: scripts/check.sh --full is still due."
    echo "    It runs everything left out above, and belongs at the end of the wave on"
    echo "    development and in the release chain; a tag and a Windows package refuse a tree"
    echo "    without it."
fi

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
