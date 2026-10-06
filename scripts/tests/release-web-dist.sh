#!/usr/bin/env bash
#
# The release chain's web/dist before the packages (RD-1130-01). The 1.12.0 chain refused both
# packages twice with "web/dist is stale: web/package.json is newer than web/dist/index.html":
# once on a --resume after the checkout had gone to main and back, which rewrote web/package.json
# with the content it already had, and once in a fresh chain whose bump to the version the tree
# already carried rewrote it again (scripts/tests/set-version.sh holds that one). Both times the
# web step built nothing afterwards — skipped as green, or covered by a recorded web green.
#
# rd_release_web_dist runs before the package steps, also on a --resume: it rebuilds a web/dist
# that scripts/web-dist-stale.sh calls stale and leaves a current one alone. The freshness test
# itself is not loosened: a version line is content the bundle carries (__RD_BUILD_VERSION__,
# RD-1120-16), so a changed one is rebuilt, and a changed source is still stale.
#
# The pipeline is sourced with RELEASE_PIPELINE_LIB=1 from a scratch copy; `pnpm` is a stub that
# writes web/dist/index.html and counts its calls.
#
#   scripts/tests/release-web-dist.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib" "$TREE/web/src" "$SCRATCH/bin"
cp "$ROOT/scripts/release-pipeline.sh" "$ROOT/scripts/web-dist-stale.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{lock,lanes,verified,jobs,public-ci,release-tag,release-evidence,release-steps-build,release-steps-publish,workspace-version,inert-paths}.sh "$TREE/scripts/lib/"

CALLS="$SCRATCH/pnpm.calls"
: > "$CALLS"
cat > "$SCRATCH/bin/pnpm" <<EOF
#!/bin/sh
# pnpm --dir <dir> run build
echo "\$*" >> "$CALLS"
[ -z "\${FAKE_PNPM_FAIL:-}" ] || { echo "vite: build failed" >&2; exit 1; }
mkdir -p "\$2/dist" && echo built > "\$2/dist/index.html"
echo "vite: built"
EOF
chmod +x "$SCRATCH/bin/pnpm"
export PATH="$SCRATCH/bin:$PATH"

printf '{\n  "name": "web",\n  "version": "9.9.8"\n}\n' > "$TREE/web/package.json"
echo 'export const app = 1' > "$TREE/web/src/main.ts"
echo '<!doctype html>' > "$TREE/web/index.html"
printf '/web/dist\n/artifacts\n' > "$TREE/.gitignore"
git init -q -b main "$TREE"
git -C "$TREE" add -A
git -C "$TREE" commit -q -m "release 9.9.8"
git -C "$TREE" checkout -q -b development
sed -i 's/"9.9.8"/"9.9.9"/' "$TREE/web/package.json"
git -C "$TREE" commit -q -am "release 9.9.9"

# web/dist built after every source, as the web step left it.
built_after_the_sources() {
    find "$TREE/web/src" "$TREE/web/index.html" "$TREE/web/package.json" -exec touch -d '2001-01-01 00:00' {} +
    mkdir -p "$TREE/web/dist"
    echo built > "$TREE/web/dist/index.html"
    touch -d '2001-01-01 01:00' "$TREE/web/dist/index.html"
}
stale() { run_status "$TREE/scripts/web-dist-stale.sh"; }
builds() { wc -l < "$CALLS" | tr -d ' '; }

# A run of 9.9.9 whose steps up to sign-plugins are green, as the 1.12.0 chain stood when it
# stopped before the packages.
LOG="$TREE/artifacts/release-evidence-9.9.9.log"
# Each case runs in a subshell of its own: the pipeline's `exit` must end the case, not this script.
pipeline() {
    # shellcheck disable=SC1091  # the scratch copy
    RELEASE_PIPELINE_LIB=1 source "$TREE/scripts/release-pipeline.sh" 9.9.9 "$@"
}
green() {
    local id
    for id in "$@"; do
        printf '##RD-STEP id=%s nonce=%s version=9.9.9 exit=0 bytes=12 started=x ended=y\n' "$id" "$NONCE" >> "$LOG"
    done
}
bash -c "$(declare -f pipeline green); LOG='$LOG'; TREE='$TREE'
    pipeline > /dev/null; green preflight compat version-bump test clippy web sign-plugins"
resumed() {
    run_status bash -c "$(declare -f pipeline green); LOG='$LOG'; TREE='$TREE'
        pipeline --resume > /dev/null; $1"
}

# --- the cause --------------------------------------------------------------------------------

built_after_the_sources
stale
expect_status "web/dist built after every source: current" 0

git -C "$TREE" checkout -q main
git -C "$TREE" checkout -q development
expect "a checkout to main and back leaves web/package.json as it was" "" "$(git -C "$TREE" status --porcelain)"
stale
expect_status "but rewrites it, and web/dist counts as stale by time" 1
expect_output "naming web/package.json" "web/package.json is newer than web/dist/index.html"

# --- the chain rebuilds it before the packages, also on a --resume -----------------------------

resumed rd_release_web_dist
expect_status "a resumed run with the web step green: web/dist is made current" 0
expect_output "and says it rebuilds" "rebuilding web/dist before the packages embed it"
expect "with one build" 1 "$(builds)"
stale
expect_status "the package steps' --skip-web check passes" 0

resumed rd_release_web_dist
expect_status "a current web/dist again" 0
expect "is not built again" 1 "$(builds)"

built_after_the_sources
sed -i 's/"9.9.9"/"9.9.10"/' "$TREE/web/package.json"
stale
expect_status "a changed version line is content the bundle carries: stale" 1
resumed rd_release_web_dist
expect_status "and rebuilt" 0
expect "with a second build" 2 "$(builds)"
stale
expect_status "after which the packages take it" 0
git -C "$TREE" checkout -q -- web/package.json

built_after_the_sources
echo 'export const app = 2' > "$TREE/web/src/main.ts"
stale
expect_status "a changed source still makes web/dist stale" 1
expect_output "naming it" "web/src/main.ts is newer than web/dist/index.html"
resumed rd_release_web_dist
expect_status "and the chain rebuilds it" 0
expect "with a third build" 3 "$(builds)"

built_after_the_sources
touch "$TREE/web/package.json"
run_status bash -c "export FAKE_PNPM_FAIL=1; $(declare -f pipeline green); LOG='$LOG'; TREE='$TREE'
    pipeline --resume > /dev/null; rd_release_web_dist"
expect_status "a build that fails: refused" 1
stale
expect_status "and web/dist stays stale" 1

resumed "green build-linux build-windows; rd_release_web_dist"
expect_status "both packages already green in the resumed run: nothing to do" 0
expect "and nothing is built" 4 "$(builds)"

# The main loop calls it before the first package step, sequential or side by side.
loop_call="$(grep -n 'rd_release_web_dist 2>&1 | tee -a "$LOG"' "$TREE/scripts/release-pipeline.sh" | cut -d: -f1)"
parallel_start="$(grep -n 'run_steps_parallel "${PARALLEL_STEPS\[@\]}"' "$TREE/scripts/release-pipeline.sh" | cut -d: -f1)"
if [[ -n "$loop_call" && -n "$parallel_start" && "$loop_call" -lt "$parallel_start" ]]; then
    ok "the pipeline's loop calls it before the package steps start"
else
    fail "the pipeline's loop calls it before the package steps start" \
        "call at line ${loop_call:-none}, package steps at line ${parallel_start:-none}"
fi

finish_tests release-web-dist
