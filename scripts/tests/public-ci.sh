#!/usr/bin/env bash
#
# scripts/public-ci.sh and scripts/lib/public-ci.sh against stubs (RD-140-22): `gh` is a script
# that answers from files, the export a script that commits into a scratch clone whose origin is
# a bare repository. What is tested is the flow — the platforms become ci.yml's JSON input, a
# dispatched run is watched by its event and its push run skipped, green deletes the public
# branch, red, a run that never appears and one past the ceiling keep it and fail, and a long
# run is waited for past the start deadline — by its id once seen, through an empty list and a
# failed look, its completion read twice (RD-1120-06); a green is recorded per platform, and a platform
# already green for the tree is not dispatched again (RD-160-06), and with Linux and Windows green
# ci.yml's once-per-run jobs neither (RD-1120-07); a job that fails is reported
# the moment it is seen and what did not change is not printed again (RD-1100-13); with
# --release-workflows the release workflows are dispatched beside ci.yml, each only while no green
# covers it, and recorded under their file names, and a red job or run ends the wait at once,
# naming it, while the other runs go on (RD-1270-10) — not GitHub.
#
#   scripts/tests/public-ci.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export RD_PUBLIC_DIR="$SCRATCH/public"
export RD_PUBLIC_REPO="owner/repo"
export RD_PUBLIC_CI_POLL=0
export RD_PUBLIC_CI_TIMEOUT=30
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"

# gh: every call is logged; `run list` prints $FAKE/runs, and fails while it does not exist.
# A $FAKE/runs.next replaces $FAKE/runs after one look, so a run can finish while it is watched.
# With RD_PUBLIC_CI_CLOCK set every `run list` moves that clock one second on: a deadline passes
# after a counted number of looks, not after the machine's wall clock (RD-1300-02).
# Lines as the --jq of scripts/lib/public-ci.sh prints them: `<id> <status> <conclusion|-> <url>
# <name>`. `run view <id> --json jobs` prints the jobs in $FAKE/jobs-<id>, in the same shape
# without the id, and a $FAKE/jobs-<id>.next replaces it after one look; `run view <id>` otherwise answers with the first line of $FAKE/view-<id> and
# drops it (`FAIL` fails, `EMPTY` prints nothing), and without one with the run's line in
# $FAKE/runs, failing when there is none.
cat > "$SCRATCH/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
case "$1 $2" in
    "auth status") exit 0 ;;
    "workflow run") exit 0 ;;
    "run list")
        [[ -z "${RD_PUBLIC_CI_CLOCK:-}" ]] \
            || echo $(( $(cat "$RD_PUBLIC_CI_CLOCK") + 1 )) > "$RD_PUBLIC_CI_CLOCK"
        [[ -f "$FAKE/runs" ]] || exit 1
        cat "$FAKE/runs"
        [[ ! -f "$FAKE/runs.next" ]] || mv "$FAKE/runs.next" "$FAKE/runs"
        ;;
    "run view")
        if [[ "$*" == *"--json jobs"* ]]; then
            cat "$FAKE/jobs-$3" 2> /dev/null || true
            [[ ! -f "$FAKE/jobs-$3.next" ]] || mv "$FAKE/jobs-$3.next" "$FAKE/jobs-$3"
            exit 0
        fi
        if [[ -s "$FAKE/view-$3" ]]; then
            line="$(head -n 1 "$FAKE/view-$3")"
            sed -i 1d "$FAKE/view-$3"
            case "$line" in FAIL) exit 1 ;; EMPTY) exit 0 ;; *) echo "$line" ;; esac
        else
            grep "^$3 " "$FAKE/runs" 2> /dev/null || exit 1
        fi
        ;;
    *) echo "fake gh: unexpected $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH"

# The repository under test: public-ci.sh and its library, a stub export, one branch.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib"
cp "$ROOT/scripts/public-ci.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{public-ci,verified,verified-ci,lanes,workspace-version,inert-paths}.sh "$TREE/scripts/lib/"
cat > "$TREE/scripts/export-public.sh" <<'EOF'
#!/usr/bin/env bash
# Stub: records its arguments, commits onto --branch in the public clone and pushes it.
set -euo pipefail
echo "$*" > "$FAKE/export.args"
branch="$(sed -n 's/.*--branch \([^ ]*\).*/\1/p' <<< "$*")"
git -C "$RD_PUBLIC_DIR" checkout -q -B "$branch"
git -C "$RD_PUBLIC_DIR" commit -q --allow-empty -m "export"
git -C "$RD_PUBLIC_DIR" push -q --force origin "$branch"
git -C "$RD_PUBLIC_DIR" checkout -q main
EOF
chmod +x "$TREE/scripts/export-public.sh"
printf '[workspace.package]\nversion = "1.4.0-dev"\n' > "$TREE/Cargo.toml"
git init -q -b development "$TREE"
git -C "$TREE" add -A
git -C "$TREE" commit -qm base
git -C "$TREE" branch integration/1.4-w4

git init -q --bare -b main "$SCRATCH/public.git"
git clone -q "$SCRATCH/public.git" "$RD_PUBLIC_DIR" 2> /dev/null
git -C "$RD_PUBLIC_DIR" commit -q --allow-empty -m main
git -C "$RD_PUBLIC_DIR" push -q origin main
remote_has() { git -C "$SCRATCH/public.git" rev-parse --verify --quiet "refs/heads/$1" > /dev/null; }
RECORD="$TREE/.git/rd-verified-ci"
# Every case starts without a recorded green unless it says KEEP=1.
public_ci() {
    rm -f "$FAKE/gh.calls" "$FAKE/export.args"
    [[ "${KEEP:-0}" -eq 1 ]] || rm -f "$RECORD"
    run_status "$TREE/scripts/public-ci.sh" "$@"
}

# --- the platform list ---------------------------------------------------------------------------
# shellcheck source=../lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"
expect "short names become ci.yml's runner images" '["ubuntu-24.04","windows-2025"]' "$(rd_public_ci_platforms linux,windows)"
expect "a runner image is taken as it is" '["macos-15","windows-2022"]' "$(rd_public_ci_platforms macos,windows-2022)"
run_status rd_public_ci_platforms linux,beos
expect_status "an unknown platform is refused" 2

# ci.yml's `jobs` input (RD-1120-07): `[]` once recorded Linux and Windows greens cover the tree.
JOBS_TREE="$(git -C "$TREE" rev-parse 'HEAD^{tree}')"
expect "no record: the default, every once-per-run job" "" "$(rd_public_ci_once_jobs "$TREE" "$JOBS_TREE")"
echo "ubuntu-24.04 $JOBS_TREE x" > "$RECORD"
expect "Linux alone on record: still every one" "" "$(rd_public_ci_once_jobs "$TREE" "$JOBS_TREE")"
echo "windows-2025 $JOBS_TREE x" >> "$RECORD"
expect "Linux and Windows on record: none" "[]" "$(rd_public_ci_once_jobs "$TREE" "$JOBS_TREE")"
rm -f "$RECORD"

public_ci integration/1.4-w4 --platforms amiga
expect_status "public-ci.sh refuses it before exporting anything" 2
expect_true "nothing was exported" '[[ ! -f "$FAKE/export.args" ]]'
public_ci no/such-branch
expect_status "a branch that does not exist" 2
public_ci
expect_status "no branch at all" 2

# --- a dispatched run, green ---------------------------------------------------------------------
echo "1 completed success https://example.invalid/runs/1 CI" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms linux,windows
expect_status "green on the named platforms" 0
expect_output "says so" "integration/1.4-w4"
expect "the export: the workspace version, the branch's commit, ci/<flattened name>, no push CI" \
    "1.4.0 --ref $(git -C "$TREE" rev-parse integration/1.4-w4) --branch ci/integration-1.4-w4 --skip-push-ci" \
    "$(cat "$FAKE/export.args")"
expect_true "ci.yml is started on that branch with the JSON list" \
    'grep -qF "workflow run ci.yml --repo owner/repo --ref ci/integration-1.4-w4 -f platforms=[\"ubuntu-24.04\",\"windows-2025\"]" "$FAKE/gh.calls"'
expect_true "and only the dispatched run is watched" 'grep -q "^run list .*--event workflow_dispatch" "$FAKE/gh.calls"'
expect_true "the public branch is deleted" '! remote_has ci/integration-1.4-w4'

expect "the green is recorded per platform, for the branch's tree" \
    "ubuntu-24.04 $(git -C "$TREE" rev-parse 'integration/1.4-w4^{tree}')|windows-2025 $(git -C "$TREE" rev-parse 'integration/1.4-w4^{tree}')" \
    "$(cut -d' ' -f1,2 "$RECORD" | paste -sd'|' -)"

# --- what is green already is not run again (RD-160-06) -------------------------------------------
KEEP=1 public_ci integration/1.4-w4 --platforms linux,windows
expect_status "every named platform green for this tree: passes" 0
expect_output "without a run" "nothing was run"
expect_true "nothing exported" '[[ ! -f "$FAKE/export.args" ]]'
expect_true "nothing asked of gh" '[[ ! -f "$FAKE/gh.calls" ]]'

KEEP=1 public_ci integration/1.4-w4
expect_status "without platforms: only the one not yet green" 0
expect_output "naming what it relies on" "windows-2025: green for this tree already"
expect_true "macOS alone is dispatched" \
    'grep -qF "workflow run ci.yml --repo owner/repo --ref ci/integration-1.4-w4 -f platforms=[\"macos-15\"]" "$FAKE/gh.calls"'
expect_true "without the once-per-run jobs the Linux and Windows runs passed (RD-1120-07)" \
    'grep -q "platforms=\[\"macos-15\"\] -f jobs=\[\]$" "$FAKE/gh.calls"'
expect_true "with [skip ci] on the export" 'grep -q -- "--skip-push-ci" "$FAKE/export.args"'
expect "and recorded" "3" "$(wc -l < "$RECORD")"

# The release candidate: the same content plus documentation and a version bump.
git -C "$TREE" checkout -q integration/1.4-w4
printf '[workspace.package]\nversion = "1.4.0"\n' > "$TREE/Cargo.toml"
echo "## [1.4.0]" > "$TREE/CHANGELOG.md"
git -C "$TREE" add -A
git -C "$TREE" commit -qm "chore(release): 1.4.0"
git -C "$TREE" checkout -q development
KEEP=1 public_ci integration/1.4-w4
expect_status "a tree differing only in documentation and version lines: passes" 0
expect_output "saying which green it relies on" "differs only in documentation and version lines"
expect_true "without a run" '[[ ! -f "$FAKE/export.args" ]]'
git -C "$TREE" branch -q -f integration/1.4-w4 integration/1.4-w4~1

# --- without platforms: all three, dispatched ----------------------------------------------------
public_ci integration/1.4-w4
expect_status "green on every platform" 0
expect_true "ci.yml is dispatched for all three" \
    'grep -qF "platforms=[\"ubuntu-24.04\",\"windows-2025\",\"macos-15\"]" "$FAKE/gh.calls"'
expect_true "with every once-per-run job, ci.yml's default" '! grep -q -- "-f jobs=" "$FAKE/gh.calls"'
expect_true "and no release workflow without --release-workflows" '! grep -q "^workflow run e2e.yml" "$FAKE/gh.calls"'

# --- the release workflows beside ci.yml (RD-1270-10) --------------------------------------------
dispatched_workflows() {
    sed -n 's|^workflow run \([a-z0-9-]*\.yml\) --repo owner/repo --ref ci/integration-1.4-w4$|\1|p' "$FAKE/gh.calls" \
        | paste -sd'|' -
}
echo "11 completed success https://example.invalid/runs/11 CI" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms linux,windows --release-workflows
expect_status "ci.yml and the release workflows, green" 0
expect_true "ci.yml for the named platforms" \
    'grep -qF "workflow run ci.yml --repo owner/repo --ref ci/integration-1.4-w4 -f platforms=[\"ubuntu-24.04\",\"windows-2025\"]" "$FAKE/gh.calls"'
expect "beside it each release workflow with its defaults, on the same branch" \
    "e2e.yml|recovery.yml|self-update.yml|installers.yml" "$(dispatched_workflows)"
expect "every green recorded, the workflows under their file names" \
    "e2e.yml|installers.yml|recovery.yml|self-update.yml|ubuntu-24.04|windows-2025" \
    "$(cut -d' ' -f1 "$RECORD" | sort | paste -sd'|' -)"

KEEP=1 public_ci integration/1.4-w4 --release-workflows
expect_status "the workflows green on record, macOS not" 0
expect_true "macOS alone is dispatched" 'grep -qF "platforms=[\"macos-15\"]" "$FAKE/gh.calls"'
expect "and no release workflow again" "" "$(dispatched_workflows)"
expect_output "naming the green it relies on" "e2e.yml: green for this tree already"

# The release candidate: the same content plus documentation and a version bump. Self-update
# raises the tree's own version, so its green before the bump covers the candidate too.
git -C "$TREE" checkout -q integration/1.4-w4
printf '[workspace.package]\nversion = "1.4.0"\n' > "$TREE/Cargo.toml"
echo "## [1.4.0]" > "$TREE/CHANGELOG.md"
git -C "$TREE" add -A
git -C "$TREE" commit -qm "chore(release): 1.4.0"
git -C "$TREE" checkout -q development
KEEP=1 public_ci integration/1.4-w4 --release-workflows
expect_status "the candidate: every platform and workflow covered" 0
expect_output "the workflows by the integration tree's greens" \
    "self-update.yml: green already for tree $(git -C "$TREE" rev-parse 'integration/1.4-w4~1^{tree}' | cut -c1-12)"
expect_true "nothing run" '[[ ! -f "$FAKE/export.args" ]]'
git -C "$TREE" branch -q -f integration/1.4-w4 integration/1.4-w4~1

grep -v '\.yml ' "$RECORD" > "$SCRATCH/record" && mv "$SCRATCH/record" "$RECORD"
KEEP=1 public_ci integration/1.4-w4 --platforms linux,windows --release-workflows
expect_status "the platforms on record, the workflows not" 0
expect_true "ci.yml is not dispatched" '! grep -q "^workflow run ci.yml" "$FAKE/gh.calls"'
expect "the workflows are" "e2e.yml|recovery.yml|self-update.yml|installers.yml" "$(dispatched_workflows)"

printf '%s\n' "12 in_progress - https://example.invalid/runs/12 CI" \
    "13 completed failure https://example.invalid/runs/13 E2E" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms linux,windows --release-workflows
expect_status "a red release workflow fails" 1
expect_output "at once, naming it, while ci.yml still runs" \
    "the public CI is red: run E2E: failure (https://example.invalid/runs/13); the wait ends here"
expect "after one look" "1" "$(grep -c "^run list" "$FAKE/gh.calls")"
expect_true "the public branch is kept" 'remote_has ci/integration-1.4-w4'
expect_true "and nothing is recorded" '[[ ! -s "$RECORD" ]]'

# --- red, and a run that never finishes ----------------------------------------------------------
printf '%s\n' "2 completed success https://example.invalid/runs/2 web" \
    "3 completed failure https://example.invalid/runs/3 CI" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms windows
expect_status "a red run fails" 1
expect_output "naming the run" "completed failure CI https://example.invalid/runs/3"
expect_output "and how to read it" "scripts/ci-log.sh"
expect_true "the public branch is kept for inspection" 'remote_has ci/integration-1.4-w4'
expect_true "and nothing is recorded" '[[ ! -s "$RECORD" ]]'

echo "4 completed cancelled https://example.invalid/runs/4 CI" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms windows
expect_status "a cancelled run is not green" 1

echo "5 in_progress - https://example.invalid/runs/5 CI" > "$FAKE/runs"
echo "5 completed success https://example.invalid/runs/5 CI" > "$FAKE/runs.next"
RD_PUBLIC_CI_TIMEOUT=0 public_ci integration/1.4-w4 --platforms windows
expect_status "a run still going is waited for past the start deadline" 0
expect_true "and looked at until it finished" '[[ $(grep -c "^run list" "$FAKE/gh.calls") -eq 2 ]]'

echo "6 in_progress - https://example.invalid/runs/6 CI" > "$FAKE/runs"
RD_PUBLIC_CI_CEILING=0 public_ci integration/1.4-w4 --platforms windows
expect_status "a run past the ceiling fails" 1
expect_output "saying so" "did not finish within 0s"
expect_true "the public branch is kept" 'remote_has ci/integration-1.4-w4'

# --- a job that fails while its run goes on: the wait ends there (RD-1100-13, RD-1270-10) --------
echo "7 in_progress - https://example.invalid/runs/7 CI" > "$FAKE/runs"
echo "in_progress - https://example.invalid/jobs/72 rust (ubuntu-24.04)" > "$FAKE/jobs-7"
printf '%s\n' "in_progress - https://example.invalid/jobs/72 rust (ubuntu-24.04)" \
    "completed failure https://example.invalid/jobs/71 rust (windows-2025)" > "$FAKE/jobs-7.next"
public_ci integration/1.4-w4 --platforms linux,windows
expect_status "a run with a failed job fails" 1
expect_output "the failed job is named with how to read it" \
    "!! rust (windows-2025): failure — scripts/ci-log.sh https://example.invalid/jobs/71"
expect_output "and the wait ends with its name" \
    "the public CI is red: rust (windows-2025): failure (https://example.invalid/jobs/71); the wait ends here"
expect "at the look that saw it, while its run still goes on" "2" \
    "$(grep -c '^run view 7 --repo owner/repo --json jobs' "$FAKE/gh.calls")"
expect "a job that did not change is not printed again" "1" \
    "$(grep -c 'rust (ubuntu-24.04): started' <<< "$output")"
expect_true "and nothing is recorded" '[[ ! -s "$RECORD" ]]'
rm -f "$FAKE/jobs-7"

# --- a run seen, then an empty list and failed looks, then finished (RD-1120-06, audit A3) --------
# 1.11 wave 2: the list came back empty after the run had been seen, and past the start deadline
# that read as "no CI run appeared". Seen once, a run is asked for by its id, and a failed or empty
# answer keeps what was seen last.
echo "8 in_progress - https://example.invalid/runs/8 CI" > "$FAKE/runs"
: > "$FAKE/runs.next"
printf '%s\n' FAIL EMPTY "8 in_progress - https://example.invalid/runs/8 CI" \
    "8 completed success https://example.invalid/runs/8 CI" \
    "8 completed success https://example.invalid/runs/8 CI" > "$FAKE/view-8"
RD_PUBLIC_CI_TIMEOUT=0 public_ci integration/1.4-w4 --platforms windows
expect_status "a run seen, then an empty list and failed looks, then green: green" 0
expect_true "past the start deadline without giving up" '! grep -q "no CI run appeared" <<< "$output"'
expect_true "asked for by its id after the first look" 'grep -q "^run view 8 --repo owner/repo --json databaseId,status,conclusion,name,url" "$FAKE/gh.calls"'
expect "every view answer used, the second completed one as the confirmation" "" "$(cat "$FAKE/view-8")"
expect "completed confirmed by a second list" "2" "$(grep -c "^run list" "$FAKE/gh.calls")"
expect_true "and recorded" 'grep -q "^windows-2025 " "$RECORD"'

# The confirmation finds a run that appeared meanwhile, and waits for it too.
echo "9 completed success https://example.invalid/runs/9 CI" > "$FAKE/runs"
printf '%s\n' "9 completed success https://example.invalid/runs/9 CI" \
    "10 in_progress - https://example.invalid/runs/10 web" > "$FAKE/runs.next"
printf '%s\n' "10 completed failure https://example.invalid/runs/10 web" \
    "10 completed failure https://example.invalid/runs/10 web" > "$FAKE/view-10"
public_ci integration/1.4-w4 --platforms windows
expect_status "a run that appeared before the confirmation decides: red" 1
expect_output "naming it" "completed failure web https://example.invalid/runs/10"
rm -f "$FAKE/view-10"

# --- never a run --------------------------------------------------------------------------------
rm -f "$FAKE/runs"
RD_PUBLIC_CI_TIMEOUT=0 public_ci integration/1.4-w4 --platforms windows
expect_status "no run at all fails at the start deadline" 1
expect_output "saying so" "no CI run appeared within 0s"
# Two seconds on the stand-in clock, which each look moves by one: under load a wall-clock second
# held one look (RD-1300-02).
echo 0 > "$FAKE/clock"
RD_PUBLIC_CI_CLOCK="$FAKE/clock" RD_PUBLIC_CI_TIMEOUT=2 public_ci integration/1.4-w4 --platforms windows
expect_status "nor within a deadline that is waited for" 1
expect "after more than one look, and none past the deadline" "2" "$(grep -c "^run list" "$FAKE/gh.calls")"
expect_output "saying so" "no CI run appeared within 2s"

finish_tests public-ci
