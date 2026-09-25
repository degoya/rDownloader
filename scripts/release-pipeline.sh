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
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *) VERSION="$1"; shift ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    echo "usage: scripts/release-pipeline.sh <version> [--push] [--resume] [--plan]" >&2
    exit 2
fi
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "not a release version: $VERSION" >&2
    exit 2
fi

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
# ---------------------------------------------------------------------------------------------
STEP_IDS=(
    preflight version-bump test clippy web sign-plugins build-linux build-windows
    verify-artifacts smoke docs-gate commit-guard commit merge-main evidence-gate public-ci tag
    push publish-public
)

step_command() {
    case "$1" in
        preflight)        echo "step_preflight" ;;
        version-bump)     echo "step_version_bump" ;;
        test)             echo "step_test" ;;
        clippy)           echo "step_clippy" ;;
        web)              echo "step_web" ;;
        sign-plugins)     echo "step_sign_plugins" ;;
        build-linux)      echo "step_build_linux" ;;
        build-windows)    echo "step_build_windows" ;;
        verify-artifacts) echo "step_verify_artifacts" ;;
        smoke)            echo "step_smoke" ;;
        docs-gate)        echo "step_docs_gate" ;;
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

# Everything the evidence gate demands a clean record for. The gate itself, the public CI run, the
# tag, the push and the public export come after it, so they are not in the list.
GATE_REQUIRES=(
    preflight version-bump test clippy web sign-plugins build-linux build-windows
    verify-artifacts smoke docs-gate commit-guard commit merge-main
)

if [[ "$PLAN_ONLY" -eq 1 ]]; then
    echo "release $VERSION — $(( ${#STEP_IDS[@]} - 1 )) steps, push $([[ $DO_PUSH -eq 1 ]] && echo enabled || echo disabled)"
    for id in "${STEP_IDS[@]}"; do
        [[ "$id" =~ ^(push|public-ci)$ && "$DO_PUSH" -eq 0 ]] && { echo "  - $id (skipped: --push not given)"; continue; }
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

marker_field() { sed -n "s/.* $2=\([^ ]*\).*/\1/p" <<< "$1"; }

# The last recorded attempt at a step, for this run only. Empty when there is none.
last_marker() {
    grep -h "^##RD-STEP id=$1 nonce=$NONCE " "$LOG" 2>/dev/null | tail -1 || true
}

step_is_green() {
    local marker; marker="$(last_marker "$1")"
    [[ -n "$marker" ]] || return 1
    [[ "$(marker_field "$marker" exit)" == "0" ]] || return 1
    [[ "$(marker_field "$marker" bytes)" -gt 0 ]] || return 1
    return 0
}

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

run_step() {
    local id="$1"; shift
    local -a codes
    local started ended before after status bytes

    if [[ "$RESUME" -eq 1 ]] && step_is_green "$id"; then
        echo "==> [$id] already green in this run — skipped"
        return 0
    fi

    printf '\n===== STEP %s (%s) =====\n' "$id" "$(date -Is)" >> "$LOG"
    started="$(date -Is)"
    before="$(stat -c %s "$LOG")"

    echo
    echo "==> [$id]"
    # The exit status comes from PIPESTATUS[0]. `set +e` is what lets us read it at all: with
    # `set -e` still armed the script would leave before the assignment. Nothing is filtered
    # between the command and tee, so nothing can turn a failure into a success.
    set +e
    "$@" 2>&1 | tee -a "$LOG"
    codes=("${PIPESTATUS[@]}")
    set -e
    status="${codes[0]}"

    after="$(stat -c %s "$LOG")"
    bytes="$(( after - before ))"
    ended="$(date -Is)"
    printf '##RD-STEP id=%s nonce=%s version=%s exit=%s bytes=%s started=%s ended=%s\n' \
        "$id" "$NONCE" "$VERSION" "$status" "$bytes" "$started" "$ended" >> "$LOG"

    if [[ "$status" -ne 0 ]]; then
        echo >&2
        echo "!! [$id] failed with exit $status — the pipeline stops here." >&2
        echo "   evidence so far: $LOG" >&2
        echo "   fix it, then: scripts/release-pipeline.sh $VERSION --resume" >&2
        exit "$status"
    fi
    if [[ "$bytes" -le 0 ]]; then
        echo >&2
        echo "!! [$id] exited 0 but produced no output; that is missing evidence, not a pass." >&2
        exit 1
    fi
}

# ---------------------------------------------------------------------------------------------
# Steps
# ---------------------------------------------------------------------------------------------

step_preflight() {
    echo "release $VERSION from $RELEASE_BRANCH into $MAIN_BRANCH"
    echo "run nonce: $NONCE"

    local branch; branch="$(git rev-parse --abbrev-ref HEAD)"
    [[ "$branch" == "$RELEASE_BRANCH" ]] || {
        echo "on '$branch', but a release is cut from '$RELEASE_BRANCH'" >&2; return 1; }

    if [[ -n "$(git status --porcelain)" ]]; then
        echo "the working tree is dirty; a release starts from a committed state" >&2
        git status --short >&2
        return 1
    fi

    # A worktree links web/node_modules and web/dist into the main checkout, and a build here
    # rewrites the tracked declaration files with paths from the wrong tree.
    if [[ -L web/node_modules || -L web/dist ]]; then
        echo "web/node_modules or web/dist is a symlink — this is a feature worktree." >&2
        echo "release from the main checkout instead." >&2
        return 1
    fi

    if git rev-parse -q --verify "refs/tags/v$VERSION" > /dev/null; then
        echo "tag v$VERSION already exists" >&2
        return 1
    fi

    local tool
    for tool in cargo cargo-nextest node npm python3 zip; do
        command -v "$tool" > /dev/null || { echo "missing tool: $tool" >&2; return 1; }
    done
    cargo xwin --version > /dev/null 2>&1 || { echo "missing: cargo xwin" >&2; return 1; }
    echo "tools: $(cargo --version), $(node --version), $(cargo xwin --version)"

    # Concurrency is the other way this machine dies. One heavy job at a time, always.
    local others
    others="$(pgrep -c -f 'cargo (build|test|clippy|nextest|xwin)' || true)"
    if [[ "${others:-0}" -gt 0 ]]; then
        echo "$others cargo job(s) already running; refusing to pile on" >&2
        pgrep -a -f 'cargo (build|test|clippy|nextest|xwin)' >&2 || true
        return 1
    fi

    # A release never starts from a deferred state. scripts/check.sh --defer postpones a
    # triviality to the next ordinary run; the postponement expires here at the latest, and the
    # message names how many commits have never been through a green run.
    rd_verified_gate "$ROOT" "$RELEASE_BRANCH" || return 1

    echo "preflight ok at $(git rev-parse --short HEAD)"
}

step_version_bump() {
    scripts/set-version.sh "$VERSION"
    local reported; reported="$(scripts/set-version.sh)"
    [[ "$reported" == "$VERSION" ]] || { echo "version is $reported after the bump" >&2; return 1; }
    echo "workspace, web/package.json and Cargo.lock report $reported"
}

# The full Rust suite, through check.sh because it owns the one thing a naive `cargo test
# --workspace` gets fatally wrong here: rd-api's 55 integration binaries each link the whole
# dependency graph, and building them at once has OOM-killed WSL even at JOBS=2. check.sh runs
# them four binaries at a time. This also covers fmt, the failpoint crash matrix and sqlx offline.
# --full, because a branch-level run leaves out what a release must not (RD-120-58), and because
# tag-release.sh and package-windows.sh refuse a tree without a --full green of both halves.
step_test() { JOBS="$JOBS" scripts/check.sh --rust --full; }

# Deliberately the full workspace, deliberately alone, deliberately at 2 jobs. AGENTS.md calls
# this the run that has needed a hard restart; nothing else is running beside it here.
step_clippy() {
    CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings
}

# typecheck, vitest, the production web build and the browser extensions.
step_web() { JOBS="$JOBS" scripts/check.sh --web --full; }

step_sign_plugins() {
    # The script refuses a plugin that changed under a signed version and exits 1 after
    # packaging the rest; without the check here, the older package of that version still sat in
    # dist/plugins/, the count below matched, and 1.2.2 shipped two stale packages as green.
    scripts/build-plugins.sh || { echo "signing refused at least one plugin" >&2; return 1; }
    local signed; signed="$(ls -1 dist/plugins/*.rdplug 2>/dev/null | wc -l)"
    local expected; expected="$(scripts/build-plugins.sh --list-packageable | wc -l)"
    echo "signed $signed of $expected packageable plugins"
    [[ "$signed" -eq "$expected" ]] || { echo "signing is short of a plugin" >&2; return 1; }
}

step_build_linux() { JOBS="$JOBS" scripts/package-linux.sh; }

# cargo xwin, straight from WSL. Not the Docker cross-build: it is slower, and the artifact stage
# drops the COPY'd asset directories. --skip-web reuses the web/dist the web step just built.
step_build_windows() { JOBS="$JOBS" scripts/package-windows.sh --skip-web; }

step_verify_artifacts() {
    local missing=0 path
    for path in artifacts/linux/rdownloader artifacts/linux/rdownloader-capture \
                artifacts/windows/rdownloader.exe artifacts/windows/rdownloader-capture.exe \
                artifacts/rdownloader-windows-x86_64.zip \
                artifacts/rdownloader-site-rules.json; do
        if [[ -s "$path" ]]; then
            echo "ok   $path ($(stat -c %s "$path") bytes)"
        else
            echo "MISS $path" >&2; missing=1
        fi
    done
    # The packaged plugins must be the signed ones, and every package must carry the full set.
    local linux_plugins windows_plugins expected
    expected="$(scripts/build-plugins.sh --list-packageable | wc -l)"
    linux_plugins="$(ls -1 artifacts/linux/plugins/*.rdplug 2>/dev/null | wc -l)"
    windows_plugins="$(ls -1 artifacts/windows/plugins/*.rdplug 2>/dev/null | wc -l)"
    echo "plugins: linux $linux_plugins, windows $windows_plugins, expected $expected"
    [[ "$linux_plugins" -eq "$expected" && "$windows_plugins" -eq "$expected" ]] || missing=1

    local built; built="$(artifacts/linux/rdownloader --version 2>&1 || true)"
    echo "built binary reports: $built"
    grep -q "$VERSION" <<< "$built" || {
        echo "the built binary does not report $VERSION" >&2; missing=1; }

    [[ "$missing" -eq 0 ]]
}

# Launches the binary that was just built and talks to it over real HTTP, then drives the real UI.
step_smoke() { scripts/release-smoke.sh "$VERSION"; }

# Not a generator — a gate. A changelog entry is judgement, and a pipeline that writes its own
# release notes is a pipeline that certifies its own work. This only refuses to continue when
# the documents AGENTS.md names have not been brought up to date by hand.
step_docs_gate() {
    local failed=0

    if ! grep -q "^## \[$VERSION\]" CHANGELOG.md; then
        echo "CHANGELOG.md has no '## [$VERSION]' section" >&2; failed=1
    else
        echo "CHANGELOG.md: $(grep -c . <<< "$(awk -v v="## [$VERSION]" '
            $0 ~ "^## \\[" { inside = (index($0, v) == 1) } inside' CHANGELOG.md)") lines for $VERSION"
    fi

    # The version has moved; anything still naming the previous one is stale by definition.
    #
    # The last *shipped* release is the honest reference, and the last tag is what says so.
    # Reading it out of HEAD's Cargo.toml only works while the pipeline's own version-bump step
    # is the single thing that moves the version — bump it by hand beforehand, as a release
    # prepared over several sessions will, and `previous` becomes the version being released,
    # `v$previous` does not exist, and the fallback then asks whether the working tree is dirty.
    # Preflight refuses to start from a dirty tree, so that branch can only ever fail. Measured
    # on 1.0.9: the changelog had 860 new lines against v1.0.8 and the gate called it untouched.
    local previous
    previous="$(git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//')"
    if [[ -z "$previous" || "$previous" == "$VERSION" ]]; then
        previous="$(git show HEAD:Cargo.toml | sed -n '/^\[workspace\.package\]/,/^\[/p' \
            | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
    fi
    echo "previous version: ${previous:-unknown}"

    # What this has to establish is that the changelog was written for *this* release, and
    # the honest comparison is against the last one that shipped. Asking whether the file is
    # dirty right now cannot work: preflight refuses to start from an uncommitted tree, and
    # nothing between the two steps touches CHANGELOG.md, so the two gates contradicted each
    # other and the chain could never reach its tag. The working-tree check stays as the
    # fallback for the first release, when there is no previous tag to compare against.
    if git rev-parse -q --verify "refs/tags/v$previous" >/dev/null 2>&1; then
        if git diff --quiet "v$previous" -- CHANGELOG.md; then
            echo "CHANGELOG.md is unchanged since v$previous" >&2; failed=1
        else
            echo "CHANGELOG.md: changed since v$previous"
        fi
    elif ! git diff --quiet -- CHANGELOG.md; then
        echo "CHANGELOG.md: modified in this release"
    else
        echo "CHANGELOG.md was not touched for $VERSION" >&2; failed=1
    fi

    local doc
    for doc in README.md docs/roadmap.md; do
        [[ -f "$doc" ]] || { echo "missing $doc" >&2; failed=1; continue; }
        echo "$doc: present, $(wc -l < "$doc") lines"
    done

    # A roadmap that still calls this milestone planned, after the milestone shipped, is the
    # drift AGENTS.md forbids. Named rather than auto-corrected.
    if grep -rn "$VERSION" docs/roadmap.md > /dev/null 2>&1; then
        echo "docs/roadmap.md mentions $VERSION"
    else
        echo "docs/roadmap.md never mentions $VERSION" >&2; failed=1
    fi

    [[ "$failed" -eq 0 ]]
}

# Requirement: nothing linked, nothing generated, nothing built may enter the release commit.
# Runs against the staged index, before the commit exists and long before the merge.
step_commit_guard() {
    git add -A
    local failed=0

    echo "--- staged ---"
    git diff --cached --name-status

    local links
    links="$(git diff --cached --raw | awk '$2 == "120000" || $1 == ":120000" { print $NF }')"
    if [[ -n "$links" ]]; then
        echo "symlinks staged:" >&2; echo "$links" >&2; failed=1
    else
        echo "symlinks: none"
    fi

    # web/dist and node_modules are gitignored twice over — with and without the trailing slash,
    # because a worktree makes them symlinks and a directory pattern would miss those. This is
    # the belt to that braces: even if the ignore rules were edited away, the commit is refused.
    local forbidden
    # dist/plugins/.keep is tracked on purpose — it is the placeholder that makes the directory
    # exist, not build output — so it is the one path this pattern must not catch.
    forbidden="$(git diff --cached --name-only \
        | grep -E '(^|/)node_modules(/|$)|(^|/)dist(/|$)|^artifacts/|^target/|\.rdplug$' \
        | grep -vxF 'dist/plugins/.keep' || true)"
    if [[ -n "$forbidden" ]]; then
        echo "build output or dependencies staged:" >&2; echo "$forbidden" >&2; failed=1
    else
        echo "build output: none staged"
    fi

    # An empty index is only a defect when the release content is still missing. A release
    # prepared over several sessions has the version bump, the changelog section and the roadmap
    # status committed long before the pipeline runs, and then there is nothing left to commit —
    # which is a finished state, not an unstarted one. Measured on 1.0.9, where every one of the
    # three was already on `development`.
    if [[ -z "$(git diff --cached --name-only)" ]]; then
        if [[ "$(scripts/set-version.sh)" == "$VERSION" ]] && grep -q "^## \[$VERSION\]" CHANGELOG.md; then
            echo "nothing staged: the version bump and the changelog are already committed"
        else
            echo "nothing staged, and the release content is not committed either" >&2; failed=1
        fi
    fi

    [[ "$failed" -eq 0 ]]
}

step_commit() {
    if [[ -z "$(git diff --cached --name-only)" ]]; then
        echo "nothing to commit; the release content is already on this branch"
        git --no-pager show --stat --oneline HEAD
        return 0
    fi
    git commit -m "chore(release): $VERSION"
    git --no-pager show --stat --oneline HEAD
}

step_merge_main() {
    local from; from="$(git rev-parse --abbrev-ref HEAD)"
    echo "merging $from into $MAIN_BRANCH"
    git checkout "$MAIN_BRANCH"
    if ! git merge --no-ff "$from" -m "Merge branch '$from' — release $VERSION"; then
        echo "the merge conflicted; leaving $MAIN_BRANCH untouched" >&2
        git merge --abort || true
        git checkout "$from"
        return 1
    fi
    git --no-pager log --oneline -1
    # The guard again, on what the merge actually produced.
    local links
    links="$(git ls-tree -r HEAD | awk '$1 == "120000" { print $4 }')"
    if [[ -n "$links" ]]; then
        echo "the merged tree contains symlinks:" >&2; echo "$links" >&2; return 1
    fi
    echo "merged tree: no symlinks"
}

step_evidence_gate() {
    local id marker exit_code bytes failed=0
    echo "verifying evidence for run $NONCE in $LOG"
    if ! grep -q "^##RD-RELEASE version=$VERSION nonce=$NONCE " "$LOG"; then
        echo "the log has no header for this run" >&2; return 1
    fi
    for id in "${GATE_REQUIRES[@]}"; do
        marker="$(last_marker "$id")"
        if [[ -z "$marker" ]]; then
            printf '  %-18s NO EVIDENCE\n' "$id" >&2; failed=1; continue
        fi
        exit_code="$(marker_field "$marker" exit)"
        bytes="$(marker_field "$marker" bytes)"
        if [[ "$exit_code" != "0" ]]; then
            printf '  %-18s exit=%s\n' "$id" "$exit_code" >&2; failed=1
        elif [[ "${bytes:-0}" -le 0 ]]; then
            printf '  %-18s exit=0 but 0 bytes of output\n' "$id" >&2; failed=1
        else
            printf '  %-18s exit=0, %s bytes\n' "$id" "$bytes"
        fi
    done
    if [[ "$failed" -ne 0 ]]; then
        echo "refusing to tag: the evidence is incomplete" >&2
        return 1
    fi
    echo "all ${#GATE_REQUIRES[@]} steps have passing, non-empty evidence"
}

step_tag() {
    scripts/tag-release.sh "$VERSION"
    # `head` closes the pipe as soon as it has its lines, git dies of SIGPIPE, and PIPESTATUS[0]
    # reports 141 — so the step failed on its own summary while the tag it exists to create was
    # already made by the line above. `sed` reads the whole stream instead of walking away.
    git --no-pager show --stat --oneline "v$VERSION" | sed -n '1,20p'
}

step_push() {
    git push origin "$MAIN_BRANCH"
    git push origin "$RELEASE_BRANCH"
    git push origin "v$VERSION"
    echo "pushed $MAIN_BRANCH, $RELEASE_BRANCH and v$VERSION to origin"
}

# The public CI on the candidate, before the tag (RD-130-23). GitHub's free runners check Linux,
# Windows and macOS, which this machine cannot; so the merged candidate goes to the public
# repository as the branch ci/<version>, and the tag is only made once its CI run is green.
#
# The trade-off, deliberately accepted: while the branch exists the candidate is public before
# it is a release. On green the branch is deleted at once. On red it stays, so the failed run
# can be read next to its tree, and the next run removes it — the same version's branch is
# replaced by the force push, any other ci/* branch is deleted before the new one is watched.
# Outward, so only with --push, like `push` itself.
PUBLIC_DIR="${RD_PUBLIC_DIR:-$HOME/projects/rDownloader-public}"
PUBLIC_REPO="${RD_PUBLIC_REPO:-degoya/rDownloader}"
PUBLIC_CI_TIMEOUT="${RD_PUBLIC_CI_TIMEOUT:-5400}"
PUBLIC_CI_POLL="${RD_PUBLIC_CI_POLL:-60}"

step_public_ci() {
    local branch="ci/$VERSION" sha stale runs failed deadline
    command -v gh > /dev/null || { echo "gh is required to watch the public CI" >&2; return 1; }
    gh auth status --hostname github.com > /dev/null 2>&1 \
        || { echo "gh is not signed in to github.com (gh auth login)" >&2; return 1; }

    scripts/export-public.sh "$VERSION" --ref HEAD --branch "$branch" || return 1
    # run_step calls a step without errexit, so every command that matters is checked here.
    sha="$(git -C "$PUBLIC_DIR" rev-parse "refs/heads/$branch")" || return 1

    while read -r stale; do
        [[ -n "$stale" && "$stale" != "$branch" ]] || continue
        echo "deleting $stale, left from an earlier run"
        git -C "$PUBLIC_DIR" push origin --delete "$stale" || echo "could not delete $stale" >&2
    done < <(git -C "$PUBLIC_DIR" ls-remote --heads origin 'refs/heads/ci/*' \
        | awk '{ sub("^refs/heads/", "", $2); print $2 }')

    echo "waiting for the CI of $PUBLIC_REPO on $branch at ${sha:0:12} (at most ${PUBLIC_CI_TIMEOUT}s)"
    deadline=$(( SECONDS + PUBLIC_CI_TIMEOUT ))
    while :; do
        # A failed query is a network hiccup until the deadline says otherwise.
        runs="$(gh run list --repo "$PUBLIC_REPO" --branch "$branch" --commit "$sha" \
            --json status,conclusion,name,url \
            --jq '.[] | "\(.status) \(.conclusion) \(.name) \(.url)"' 2> /dev/null)" || runs=""
        if [[ -n "$runs" ]] && ! grep -qv '^completed ' <<< "$runs"; then
            break
        fi
        if (( SECONDS >= deadline )); then
            echo "the public CI did not finish within ${PUBLIC_CI_TIMEOUT}s; $branch is kept" >&2
            [[ -z "$runs" ]] || echo "$runs" >&2
            return 1
        fi
        if [[ -z "$runs" ]]; then
            echo "  $(date +%H:%M:%S) no run for ${sha:0:12} yet"
        else
            echo "  $(date +%H:%M:%S) $(wc -l <<< "$runs") run(s), $(grep -vc '^completed ' <<< "$runs") not finished"
        fi
        sleep "$PUBLIC_CI_POLL"
    done

    echo "$runs"
    failed="$(grep -vE '^completed (success|skipped|neutral) ' <<< "$runs" || true)"
    if [[ -n "$failed" ]]; then
        echo "the public CI is red; $branch is kept for inspection and the tag is not made:" >&2
        echo "$failed" >&2
        return 1
    fi
    git -C "$PUBLIC_DIR" push origin --delete "$branch" || return 1
    # Before the first release the clone has no main to return to and still stands on the
    # branch; its local ref then simply stays until the next export.
    git -C "$PUBLIC_DIR" branch -D "$branch" > /dev/null 2>&1 || true
    echo "the public CI is green on ${sha:0:12}; $branch deleted"
}

# The public repository gets the tag as one fresh commit (RD-130-23). It runs after `push` so
# that nothing reaches the public side before the private one has it, and it publishes only
# when this run was asked to push; otherwise the export waits committed in the local clone.
# The user handbook follows into the repository's GitHub wiki, and the website's release facts
# follow both, all under the same --push rule. The website is never deployed here: the owner
# uploads the directory update-website.sh names by hand.
step_publish_public() {
    local push=()
    [[ "$DO_PUSH" -eq 0 ]] || push=(--push)
    scripts/export-public.sh "$VERSION" "${push[@]}" || return 1
    scripts/export-wiki.sh "$VERSION" "${push[@]}" || return 1
    scripts/update-website.sh "$VERSION" "${push[@]}"
}

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
        echo "    publish with: git push origin $MAIN_BRANCH $RELEASE_BRANCH v$VERSION"
        continue
    fi
    run_step "$id" "$(step_command "$id")"
done

echo
echo "==> $VERSION released$([[ $DO_PUSH -eq 1 ]] && echo " and pushed" || echo ", not pushed")"
echo "    evidence: $LOG"
