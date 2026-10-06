#!/usr/bin/env bash
#
# The release chain, run end to end, with evidence that cannot be faked into a green.
#
# scripts/release.sh is the interactive version of this: it stops after packaging and hands the
# judgement parts back. This one goes all the way to the tag, and is the thing /release drives.
# The difference that matters is not automation but proof — every step appends its raw output to
# an evidence log, records how many bytes it produced, and the tag is refused unless *this run's*
# nonce has a passing, non-empty record for every step before it.
#
# Three properties are the whole point:
#
#   * A step's exit status is read from PIPESTATUS[0], never from the tail of a pipe. Output is
#     teed, never grepped: a filter in front of `tee` would let a failing command report success.
#   * Evidence is per-run. The log header carries a nonce, every marker repeats it, and the gate
#     ignores markers from any other run. A log left over from a previous attempt proves nothing.
#   * A step that produced no output is treated as missing evidence, not as a quiet success.
#
# Usage:
#   scripts/release-pipeline.sh 1.0.1                 # everything up to the tag and the public
#                                                     # export; pushes nothing
#   scripts/release-pipeline.sh 1.0.1 --push          # ... with the public CI before the tag, and
#                                                     # publishes main, the branch, the tag and
#                                                     # the public export
#   scripts/release-pipeline.sh 1.0.1 --resume        # continue the run this log already started
#   scripts/release-pipeline.sh 1.0.1 --plan          # print the steps and exit
#   scripts/release-pipeline.sh 1.8.0-beta.1 --push   # a pre-release (see below)
#
# A pre-release is `X.Y.Z-beta.N`, and nothing else (owner, 2026-09-30). It runs the same steps
# with the same evidence, except three: merge-main is skipped and not required by the gate — the
# tag goes on the release commit on the release branch, and main stays on the last stable
# release; push publishes the release branch and the tag, not main; publish-public runs only the
# public export, whose main and README then show the beta until the stable release, while the
# public wiki and the website wait for it. The CHANGELOG gets a `## [X.Y.Z-beta.N]` section per
# beta, and docs/roadmap.md names the beta, as docs-gate checks for any release.
#
# There is deliberately no --skip-tests, --no-verify or --force. Every flag this script does not
# have is a green it cannot report falsely.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# The lock itself is taken further down, once --plan has had its say.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
# shellcheck source=lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"
# shellcheck source=lib/release-tag.sh
source "$ROOT/scripts/lib/release-tag.sh"
cd "$ROOT"

# Capped for the same reason every other script here caps it (scripts/lib/jobs.sh). Clippy over
# the whole workspace gets 2.
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
RELEASE_BRANCH="${RELEASE_BRANCH:-development}"
MAIN_BRANCH="${MAIN_BRANCH:-main}"

VERSION=""
DO_PUSH=0
RESUME=0
PLAN_ONLY=0
# The parse below shifts every argument away; the lock re-executes this script with the
# original ones, so they are kept here first.
ORIGINAL_ARGS=("$@")

while [[ $# -gt 0 ]]; do
    case "$1" in
        --push) DO_PUSH=1; shift ;;
        --resume) RESUME=1; shift ;;
        --plan) PLAN_ONLY=1; shift ;;
        -h|--help) sed -n '2,39p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *) VERSION="$1"; shift ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    echo "usage: scripts/release-pipeline.sh <version> [--push] [--resume] [--plan]" >&2
    exit 2
fi
if ! rd_release_version "$VERSION"; then
    echo "not a release version: $VERSION (X.Y.Z, or X.Y.Z-beta.N for a pre-release)" >&2
    exit 2
fi
PRERELEASE=0
rd_is_prerelease "$VERSION" && PRERELEASE=1

# artifacts/ is gitignored, which is exactly where the evidence log belongs: it must not be able
# to end up in the release commit it is evidence for.
mkdir -p artifacts
LOG="$ROOT/artifacts/release-evidence-$VERSION.log"

# ---------------------------------------------------------------------------------------------
# The steps, in the order they have to happen.
#
# sign-plugins sits *before* the packaging steps on purpose. The user-facing order says "build,
# then sign", but package-linux.sh and package-windows.sh copy dist/plugins into the package and
# refuse a package short of a plugin, so a signature produced afterwards would never reach the
# artifact. Signing first and verifying the packaged result afterwards keeps both halves honest.
#
# archive-jobs sits between docs-gate and commit-guard: the job files the release finished move
# into docs/roadmap/jobs/archive/ once their status is written, and are staged with the rest.
#
# compat sits right after preflight (RD-170-08): a break of the REST API or the plugin contract
# that nobody acknowledged is a decision still to take, and it is cheaper to learn that before
# the hours of test and packaging than after them.
#
# doc-facts sits after test on purpose (RD-140-24): it rewrites documentation lines, and test only
# carries a pre-bump --full green over a bump that changed version lines alone.
#
# build-linux and build-windows are independent — two targets, two output directories, both read
# the web/dist the web step built and the dist/plugins sign-plugins signed — so with a second lane
# (RD_LANES > 1, scripts/lib/lock.sh) they run at once, Windows in a target directory of its own
# (target/lanes/windows). The other steps stay in order: web must finish before either package
# embeds it, sign-plugins before either package copies the plugins, and test and clippy share the
# one debug target the packages do not use. Each keeps its own evidence record (run_steps_parallel).
# Right before them, also on a --resume, rd_release_web_dist rebuilds a web/dist that
# scripts/web-dist-stale.sh calls stale (RD-1130-01).
# ---------------------------------------------------------------------------------------------
STEP_IDS=(
    preflight compat version-bump test clippy web sign-plugins build-linux build-windows
    verify-artifacts smoke doc-facts docs-gate archive-jobs commit-guard commit merge-main
    evidence-gate public-ci tag push publish-public
)

step_command() {
    case "$1" in
        preflight)        echo "step_preflight" ;;
        compat)           echo "step_compat" ;;
        version-bump)     echo "step_version_bump" ;;
        test)             echo "step_test" ;;
        clippy)           echo "step_clippy" ;;
        web)              echo "step_web" ;;
        sign-plugins)     echo "step_sign_plugins" ;;
        build-linux)      echo "step_build_linux" ;;
        build-windows)    echo "step_build_windows" ;;
        verify-artifacts) echo "step_verify_artifacts" ;;
        smoke)            echo "step_smoke" ;;
        doc-facts)        echo "step_doc_facts" ;;
        docs-gate)        echo "step_docs_gate" ;;
        archive-jobs)     echo "step_archive_jobs" ;;
        commit-guard)     echo "step_commit_guard" ;;
        commit)           echo "step_commit" ;;
        merge-main)       echo "step_merge_main" ;;
        evidence-gate)    echo "step_evidence_gate" ;;
        public-ci)        echo "step_public_ci" ;;
        tag)              echo "step_tag" ;;
        push)             echo "step_push" ;;
        publish-public)   echo "step_publish_public" ;;
    esac
}

# The steps that run side by side when there is a second lane; each list is started at its first
# step and the rest of it is skipped by the main loop.
PARALLEL_STEPS=(build-linux build-windows)

# Everything the evidence gate demands a clean record for. The gate itself, the public CI run, the
# tag, the push and the public export come after it, so they are not in the list.
GATE_REQUIRES=(
    preflight compat version-bump test clippy web sign-plugins build-linux build-windows
    verify-artifacts smoke doc-facts docs-gate archive-jobs commit-guard commit merge-main
)
# A pre-release is never merged into main, so its gate cannot ask for that step's record.
if [[ "$PRERELEASE" -eq 1 ]]; then
    mapfile -t GATE_REQUIRES < <(printf '%s\n' "${GATE_REQUIRES[@]}" | grep -vx merge-main)
fi

if [[ "$PLAN_ONLY" -eq 1 ]]; then
    echo "release $VERSION$([[ $PRERELEASE -eq 1 ]] && echo " (pre-release)") — $(( ${#STEP_IDS[@]} - 1 )) steps, push $([[ $DO_PUSH -eq 1 ]] && echo enabled || echo disabled)"
    lanes="$(rd_lanes)" || exit 2
    for id in "${STEP_IDS[@]}"; do
        [[ "$id" =~ ^(push|public-ci)$ && "$DO_PUSH" -eq 0 ]] && { echo "  - $id (skipped: --push not given)"; continue; }
        [[ "$id" == merge-main && "$PRERELEASE" -eq 1 ]] && { echo "  - $id (skipped: a pre-release is not merged into $MAIN_BRANCH)"; continue; }
        [[ "$id" == publish-public && "$PRERELEASE" -eq 1 ]] && { echo "  - $id (the public export only: no wiki, no website)"; continue; }
        if [[ "$lanes" -gt 1 && " ${PARALLEL_STEPS[*]} " == *" $id "* ]]; then
            echo "  - $id (in parallel with the other package, RD_LANES=$lanes)"
            continue
        fi
        echo "  - $id"
    done
    echo "evidence: $LOG"
    exit 0
fi

# From here on the run compiles, so it serialises itself against every other heavy script.
# Not while this file is merely sourced for its step functions: re-running it would be the one
# thing RELEASE_PIPELINE_LIB exists to avoid.
[[ -n "${RELEASE_PIPELINE_LIB:-}" ]] || rd_take_lock "${ORIGINAL_ARGS[@]}"

# ---------------------------------------------------------------------------------------------
# Evidence
# ---------------------------------------------------------------------------------------------

# The markers, run_step and run_steps_parallel.
# shellcheck source=lib/release-evidence.sh
source "$ROOT/scripts/lib/release-evidence.sh"

# Decided once, after the lock: the Windows package gets a lane of its own when there is one.
LANES="$(rd_lanes)" || exit 2
WINDOWS_LANE="$(rd_target_dir "$ROOT")/lanes/windows"

if [[ "$RESUME" -eq 1 ]]; then
    [[ -f "$LOG" ]] || { echo "--resume, but $LOG does not exist" >&2; exit 1; }
    NONCE="$(sed -n 's/^##RD-RELEASE .*nonce=\([^ ]*\).*/\1/p' "$LOG" | tail -1)"
    logged_version="$(sed -n 's/^##RD-RELEASE version=\([^ ]*\).*/\1/p' "$LOG" | tail -1)"
    [[ -n "$NONCE" ]] || { echo "$LOG has no run header to resume" >&2; exit 1; }
    [[ "$logged_version" == "$VERSION" ]] || {
        echo "$LOG is evidence for $logged_version, not $VERSION" >&2; exit 1; }
    echo "==> resuming run $NONCE from $LOG"
else
    NONCE="$(date +%s)-$$-$RANDOM"
    : > "$LOG"
    printf '##RD-RELEASE version=%s nonce=%s head=%s branch=%s started=%s host=%s\n' \
        "$VERSION" "$NONCE" "$(git rev-parse HEAD)" "$(git rev-parse --abbrev-ref HEAD)" \
        "$(date -Is)" "$(hostname)" >> "$LOG"
fi

# ---------------------------------------------------------------------------------------------
# Steps
# ---------------------------------------------------------------------------------------------

# shellcheck source=lib/release-steps-build.sh
source "$ROOT/scripts/lib/release-steps-build.sh"
# shellcheck source=lib/release-steps-publish.sh
source "$ROOT/scripts/lib/release-steps-publish.sh"

# ---------------------------------------------------------------------------------------------

# Sourcing this file with RELEASE_PIPELINE_LIB=1 defines the steps and the evidence machinery
# without running a release, so the gate and the commit guard can be tested for what they refuse
# rather than only for what they allow. Nothing else reads this variable.
if [[ -n "${RELEASE_PIPELINE_LIB:-}" ]]; then
    return 0 2> /dev/null || exit 0
fi

for id in "${STEP_IDS[@]}"; do
    # From the version bump on, the packages are the release: VERSION.txt then names it and the
    # commit it was built on instead of `<commit>-dirty` (scripts/lib/version-file.sh). Set here
    # rather than inside the step, which runs in a pipe's subshell and is skipped on --resume.
    [[ "$id" == "version-bump" ]] && export RD_RELEASE_VERSION="$VERSION"
    if [[ "$id" == "public-ci" && "$DO_PUSH" -eq 0 ]]; then
        echo
        echo "==> [public-ci] not requested — the candidate is tagged without the public CI."
        continue
    fi
    if [[ "$id" == "push" && "$DO_PUSH" -eq 0 ]]; then
        echo
        echo "==> [push] not requested — nothing is pushed."
        echo "    publish with: git push origin $([[ $PRERELEASE -eq 0 ]] && echo "$MAIN_BRANCH ")$RELEASE_BRANCH v$VERSION"
        continue
    fi
    if [[ "$id" == "merge-main" && "$PRERELEASE" -eq 1 ]]; then
        echo
        echo "==> [merge-main] a pre-release is not merged into $MAIN_BRANCH; the tag goes on $RELEASE_BRANCH."
        continue
    fi
    # Before either package embeds web/dist, also on a --resume (rd_release_web_dist, RD-1130-01).
    if [[ "$id" == build-linux ]] && ! rd_release_web_dist 2>&1 | tee -a "$LOG"; then
        echo "!! web/dist could not be made current for the packages — the pipeline stops here." >&2
        echo "   fix it, then: scripts/release-pipeline.sh $VERSION --resume" >&2
        exit 1
    fi
    if [[ "$LANES" -gt 1 && " ${PARALLEL_STEPS[*]} " == *" $id "* ]]; then
        [[ "$id" == "${PARALLEL_STEPS[0]}" ]] && run_steps_parallel "${PARALLEL_STEPS[@]}"
        continue
    fi
    if [[ "$id" == "merge-main" ]]; then
        trap return_to_release_branch EXIT
        if [[ "$RESUME" -eq 1 ]] && step_is_green merge-main \
            && [[ "$(git rev-parse --abbrev-ref HEAD)" != "$MAIN_BRANCH" ]]; then
            echo "==> merge-main is green in this run; back onto $MAIN_BRANCH for the steps after it"
            git checkout -q "$MAIN_BRANCH"
        fi
    fi
    run_step "$id" "$(step_command "$id")"
done

echo
echo "==> $VERSION released$([[ $DO_PUSH -eq 1 ]] && echo " and pushed" || echo ", not pushed")"
echo "    evidence: $LOG"
