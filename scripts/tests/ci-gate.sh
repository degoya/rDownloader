#!/usr/bin/env bash
#
# scripts/ci-gate.sh, the `gate` job of ci.yml, against a stub scripts/ci-tree-greens.sh: every
# event but a push to `main` runs every image and every once-per-run job, `ONCE` and `CALLED`
# alike; a push to `main` takes the images and `ONCE` jobs the tree greens leave and always the
# `CALLED` ones, and warms the Linux and Windows images it left out; a dispatch's `jobs` input
# keeps only the jobs it names, `[]` none (RD-1120-07), and a `jobs` that is no JSON list fails.
#
# Pure bash and jq. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/ci-gate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# The gate under test in a scratch checkout, beside a ci-tree-greens.sh that answers from a file.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts"
cp "$ROOT/scripts/ci-gate.sh" "$TREE/scripts/"
cat > "$TREE/scripts/ci-tree-greens.sh" <<'EOF'
#!/usr/bin/env bash
echo "$*" > "$FAKE/greens.args"
cat "$FAKE/greens"
EOF
chmod +x "$TREE/scripts/ci-tree-greens.sh"
git init -q "$TREE"
git -C "$TREE" -c user.name=test -c user.email=test@example.invalid commit -q --allow-empty -m base
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE"

export REQUESTED='["ubuntu-24.04","windows-2025","macos-15"]'
export ONCE='["docker","scripts"]'
export CALLED='["components"]'
export GITHUB_REPOSITORY=o/r
# Runs the gate for event $1 on ref $2; its outputs land in $out_<name>.
gate() {
    export GITHUB_EVENT_NAME="$1" GITHUB_REF="$2" GITHUB_OUTPUT="$SCRATCH/output"
    : > "$GITHUB_OUTPUT"
    run_status bash -c "cd '$TREE' && scripts/ci-gate.sh"
    out_platforms="$(sed -n 's/^platforms=//p' "$GITHUB_OUTPUT")"
    out_check="$(sed -n 's/^check=//p' "$GITHUB_OUTPUT")"
    out_jobs="$(sed -n 's/^jobs=//p' "$GITHUB_OUTPUT")"
    out_warm="$(sed -n 's/^warm=//p' "$GITHUB_OUTPUT")"
}

# --- a pull request, a branch push: everything ---------------------------------------------------
gate pull_request refs/pull/1/merge
expect_status "a pull request: the gate answers" 0
expect "every image" "$REQUESTED" "$out_platforms"
expect "every once-per-run job, the called ones too" '["docker","scripts","components"]' "$out_jobs"
expect "checked" "true" "$out_check"
expect "nothing to warm" "[]" "$out_warm"
expect_true "the tree greens are not asked" '[[ ! -f "$FAKE/greens.args" ]]'

# --- a push to main: what the tree greens leave, and the called jobs always ----------------------
printf '%s\n' '["macos-15"]' '[]' > "$FAKE/greens"
gate push refs/heads/main
expect_status "a push to main: the gate answers" 0
expect "the images the greens leave" '["macos-15"]' "$out_platforms"
expect "no ONCE job, but components for main's cache" '["components"]' "$out_jobs"
expect "the Linux and Windows images left out are warmed" '["ubuntu-24.04","windows-2025"]' "$out_warm"
expect_true "the greens are asked for the images and the ONCE jobs" \
    'grep -q -- "ubuntu-24.04 windows-2025 macos-15 -- docker scripts$" "$FAKE/greens.args"'
printf '%s\n' '[]' '["scripts"]' > "$FAKE/greens"
gate push refs/heads/main
expect "a tree green everywhere: no platform" '[]' "$out_platforms"
expect "and not checked" "false" "$out_check"
expect "the ONCE job not yet green and components" '["scripts","components"]' "$out_jobs"
rm -f "$FAKE/greens.args"

# --- a dispatch: the platforms and jobs it names (RD-1120-07) ------------------------------------
REQUESTED='["macos-15"]' REQUESTED_JOBS='[]' gate workflow_dispatch refs/heads/ci/1.12.0
expect_status "a dispatch for macOS alone with jobs=[]" 0
expect "macOS" '["macos-15"]' "$out_platforms"
expect "no once-per-run job, components neither" "[]" "$out_jobs"
REQUESTED_JOBS='["docker","components","nonesuch"]' gate workflow_dispatch refs/heads/ci/x
expect "only the named jobs, in the gate's order, an unknown name dropped" '["docker","components"]' "$out_jobs"
REQUESTED_JOBS='' gate workflow_dispatch refs/heads/ci/x
expect "an empty jobs input: all of them" '["docker","scripts","components"]' "$out_jobs"
REQUESTED_JOBS='docker' gate workflow_dispatch refs/heads/ci/x
expect_status "a jobs input that is no JSON list fails" 1
expect_output "saying so" "the jobs input is not a JSON list: docker"
expect_true "the tree greens were never asked off main" '[[ ! -f "$FAKE/greens.args" ]]'

finish_tests "ci-gate"
