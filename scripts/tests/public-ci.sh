#!/usr/bin/env bash
#
# scripts/public-ci.sh and scripts/lib/public-ci.sh against stubs (RD-140-22): `gh` is a script
# that answers from files, the export a script that commits into a scratch clone whose origin is
# a bare repository. What is tested is the flow — the platforms become ci.yml's JSON input, a
# dispatched run is watched by its event and its push run skipped, green deletes the public
# branch, red and a timeout keep it and fail — not GitHub.
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
cat > "$SCRATCH/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
case "$1 $2" in
    "auth status") exit 0 ;;
    "workflow run") exit 0 ;;
    "run list") [[ -f "$FAKE/runs" ]] && cat "$FAKE/runs" ;;
    *) echo "fake gh: unexpected $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH"

# The repository under test: public-ci.sh and its library, a stub export, one branch.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib"
cp "$ROOT/scripts/public-ci.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/public-ci.sh" "$TREE/scripts/lib/"
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
public_ci() { rm -f "$FAKE/gh.calls"; run_status "$TREE/scripts/public-ci.sh" "$@"; }

# --- the platform list ---------------------------------------------------------------------------
# shellcheck source=../lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"
expect "short names become ci.yml's runner images" '["ubuntu-24.04","windows-2025"]' "$(rd_public_ci_platforms linux,windows)"
expect "a runner image is taken as it is" '["macos-15","windows-2022"]' "$(rd_public_ci_platforms macos,windows-2022)"
run_status rd_public_ci_platforms linux,beos
expect_status "an unknown platform is refused" 2

public_ci integration/1.4-w4 --platforms amiga
expect_status "public-ci.sh refuses it before exporting anything" 2
expect_true "nothing was exported" '[[ ! -f "$FAKE/export.args" ]]'
public_ci no/such-branch
expect_status "a branch that does not exist" 2
public_ci
expect_status "no branch at all" 2

# --- a dispatched run, green ---------------------------------------------------------------------
echo "completed success CI https://example.invalid/runs/1" > "$FAKE/runs"
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

# --- without platforms: the push's own run -------------------------------------------------------
public_ci integration/1.4-w4
expect_status "green on every platform" 0
expect_true "no [skip ci] export" '! grep -q -- "--skip-push-ci" "$FAKE/export.args"'
expect_true "nothing dispatched" '! grep -q "^workflow run" "$FAKE/gh.calls"'
expect_true "no event filter" '! grep -q -- "--event" "$FAKE/gh.calls"'

# --- red, and a run that never finishes ----------------------------------------------------------
printf '%s\n' "completed success web https://example.invalid/runs/2" \
    "completed failure CI https://example.invalid/runs/3" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms windows
expect_status "a red run fails" 1
expect_output "naming the run" "completed failure CI https://example.invalid/runs/3"
expect_output "and how to read it" "scripts/ci-log.sh"
expect_true "the public branch is kept for inspection" 'remote_has ci/integration-1.4-w4'

echo "completed cancelled CI https://example.invalid/runs/4" > "$FAKE/runs"
public_ci integration/1.4-w4 --platforms windows
expect_status "a cancelled run is not green" 1

echo "in_progress  CI https://example.invalid/runs/5" > "$FAKE/runs"
RD_PUBLIC_CI_TIMEOUT=0 public_ci integration/1.4-w4 --platforms windows
expect_status "a run past the deadline fails" 1
expect_output "saying so" "did not finish within 0s"

finish_tests public-ci
