#!/usr/bin/env bash
#
# scripts/ci-channels-gate.sh, the `gate` job of channels.yml, against a stub `gh`: a dispatch
# takes its inputs and refuses malformed ones; a green Release run of a plain tag checks its
# version and upgrades from the plain release before it, betas and drafts skipped (RD-1120-07);
# without an earlier release, or without an answer from GitHub, a fresh install with a warning;
# a failed or beta Release run checks nothing.
#
#   scripts/tests/ci-channels-gate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
# gh: `release list` prints $FAKE/releases (the --jq's output), and fails without it.
cat > "$SCRATCH/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
[[ "$1 $2" == "release list" && -f "$FAKE/releases" ]] || exit 1
cat "$FAKE/releases"
EOF
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH" GITHUB_REPOSITORY=o/r
# Runs the gate; $out_run, $out_expect and $out_from hold its outputs.
gate() {
    export GITHUB_OUTPUT="$SCRATCH/output"
    : > "$GITHUB_OUTPUT"
    rm -f "$FAKE/gh.calls"
    run_status env "$@" "$ROOT/scripts/ci-channels-gate.sh"
    out_run="$(sed -n 's/^run=//p' "$GITHUB_OUTPUT")"
    out_expect="$(sed -n 's/^expect=//p' "$GITHUB_OUTPUT")"
    out_from="$(sed -n 's/^from=//p' "$GITHUB_OUTPUT")"
}

# --- a dispatch ---------------------------------------------------------------------------------
gate EVENT=workflow_dispatch EXPECT=1.11.0 FROM=v1.10.1
expect_status "a dispatch" 0
expect "runs, with its inputs" "true 1.11.0 v1.10.1" "$out_run $out_expect $out_from"
expect_true "asks GitHub nothing" '[[ ! -f "$FAKE/gh.calls" ]]'
gate EVENT=workflow_dispatch EXPECT= FROM=
expect "without inputs: the current version, a fresh install" "true  " "$out_run $out_expect $out_from"
gate EVENT=workflow_dispatch FROM=1.10.1
expect_status "upgrade_from without its v" 1
expect_output "is refused" "upgrade_from must be a plain release tag like v1.6.1, not '1.10.1'"
gate EVENT=workflow_dispatch EXPECT=v1.11.0
expect_status "expect_version with a v" 1

# --- a Release run (RD-1120-07) -----------------------------------------------------------------
printf '%s\n' v1.12.0 v1.12.0-beta.1 v1.11.0 v1.10.1 v1.9.0 v1.11.1 > "$FAKE/releases"
gate EVENT=workflow_run CONCLUSION=success RUN_REF=v1.12.0
expect_status "a green release" 0
expect "checks its version and upgrades from the plain release before it" "true 1.12.0 v1.11.1" \
    "$out_run $out_expect $out_from"
expect_true "from the releases without drafts and pre-releases" \
    'grep -q -- "release list --repo o/r --exclude-drafts --exclude-pre-releases" "$FAKE/gh.calls"'
printf '%s\n' v1.11.1 v1.11.0 > "$FAKE/releases"
gate EVENT=workflow_run CONCLUSION=success RUN_REF=v1.12.0
expect "a release GitHub does not list yet still finds the one before" "v1.11.1" "$out_from"
gate EVENT=workflow_run CONCLUSION=success RUN_REF=v1.11.0
expect "an older tag re-run: the release below it" "" "$out_from"
expect_output "none below: a fresh install, said" "no plain release before v1.11.0 found"
rm -f "$FAKE/releases"
gate EVENT=workflow_run CONCLUSION=success RUN_REF=v1.12.0
expect_status "GitHub does not answer" 0
expect "the fresh install still runs" "true 1.12.0 " "$out_run $out_expect $out_from"
expect_output "with a warning" "::warning::no plain release before v1.12.0 found"
gate EVENT=workflow_run CONCLUSION=failure RUN_REF=v1.12.0
expect "a failed release checks nothing" "false" "$out_run"
gate EVENT=workflow_run CONCLUSION=success RUN_REF=v1.12.0-beta.1
expect "nor does a beta" "false" "$out_run"

finish_tests "ci-channels-gate"
