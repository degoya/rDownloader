#!/usr/bin/env bash
#
# A red scripts/check.sh --full still records the halves whose own stages were green
# (RD-1270-22, scripts/lib/check-record.sh), and the next --full over the same tree reuses them
# and runs only the red one (lib/check-reuse.sh). The stages are stand-ins in check.sh's order —
# a preflight stage, the web half beside or after the Rust half (pnpm as a stand-in), cargo fmt
# and the Rust tests — and the markers lie in a scratch target, never the real one.
#
#   scripts/tests/check-record.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
# pnpm: logs its call, red for Vitest while $FAKE/vitest-red exists.
cat > "$SCRATCH/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
echo "pnpm $*" >> "$FAKE/calls"
if [[ "$*" == *" test" && -f "$FAKE/vitest-red" ]]; then echo "FAIL [ 5.000s] a timeout"; exit 1; fi
EOF
chmod +x "$SCRATCH/bin/pnpm"
export PATH="$SCRATCH/bin:$PATH"

repo="$SCRATCH/repo"
git init -q -b development "$repo"
cd "$repo"
mkdir -p scripts web/src crates/core/src
printf '#!/usr/bin/env bash\necho build-extension >> "$FAKE/calls"\n' > scripts/build-extension.sh
printf '#!/usr/bin/env bash\nexit 0\n' > scripts/web-dist-stale.sh
chmod +x scripts/*.sh
echo "<template />" > web/src/App.vue
echo "pub fn f() {}" > crates/core/src/lib.rs
echo "web/dist" > .gitignore
ln -s /nonexistent web/dist
git add -A
git commit -qm "the state under test"

# check.sh --full in its order, its stages stand-ins: the reuse plan, the preflight's stages, the
# web half (beside the Rust half unless RD_WEB_SERIAL=1), cargo fmt and the Rust tests, the halves
# recorded, the end. Sets $status and $output; the stages run are in $FAKE/calls.
full_run() {
    rm -f "$FAKE/calls"
    run_status bash -c '
        set -euo pipefail
        ROOT="$1" lib="$2/scripts/lib"
        for file in stages check-web verified check-reuse check-record; do source "$lib/$file.sh"; done
        # A stage: logs its call, red while $FAKE/<name>-red exists.
        stand_in() { echo "$1" >> "$FAKE/calls"; [[ ! -f "$FAKE/$1-red" ]]; }
        CHECK_LOGS="$3/logs" RUN_KIND=full
        rd_stages_init
        if rd_check_reuse_plan "$ROOT" --full; then echo "==> all requested checks passed"; exit 0; fi
        full=1 run_rust=1 run_web=1 web_changed=1 extension_changed=1
        rd_check_reuse_apply
        full_tree="$reuse_tree"
        step "git diff --check"
        attempt stand_in diff-check
        rd_check_web_start
        if [[ "$run_rust" -eq 1 ]]; then
            stage_half="rust preflight"
            step "cargo fmt --all --check"
            attempt stand_in fmt
            stage_half=rust
            step "tests"
            attempt stand_in rust-tests
        else
            skip "the whole Rust half" "$rust_skip_reason"
        fi
        rd_check_web_join
        rd_stages_report
        rd_check_record_halves
        rd_stages_exit_if_failed
        rd_record_verified "$ROOT" "$(git rev-parse HEAD)"
        echo "==> all requested checks passed"
    ' full-run "$repo" "$ROOT" "$SCRATCH"
}
recorded() { cut -d' ' -f1 "$(cd "$repo" && source "$ROOT/scripts/lib/verified.sh" && rd_full_marker "$repo")" 2> /dev/null | sort | paste -sd' ' -; }
calls() { LC_ALL=C sort -u "$FAKE/calls" | paste -sd' ' -; }
# A change both halves read, so each runs.
change() { echo "// $1" >> crates/core/src/lib.rs; echo "<!-- $1 -->" >> web/src/App.vue; }
verified_marker() { (source "$ROOT/scripts/lib/verified.sh" && rd_verified_marker "$repo"); }
tree() { (source "$ROOT/scripts/lib/verified.sh" && rd_worktree_tree "$repo"); }

# --- the web half red, beside the Rust half -------------------------------------------------------
touch "$FAKE/vitest-red"
full_run
expect_status "a red web half: the run ends non-zero" 1
expect "the Rust half and the preflight recorded, the web half not" "preflight rust" "$(recorded)"
expect_output "the end names what was recorded although the web half failed" \
    "==> recorded the rust and preflight halves for tree $(tree | cut -c1-12)"
expect_output "and which half failed" "although the web half failed;"
expect_output "the failure list says no revision was recorded" "No revision was recorded green; only the rust and preflight halves"
expect_true "the revision is not recorded" '[[ ! -e "$(verified_marker)" ]]'

# --- the second --full over the same tree runs only the red half ----------------------------------
rm "$FAKE/vitest-red"
full_run
expect_status "the second --full: green" 0
expect "it ran the preflight stage and the web half, not the Rust half" \
    "build-extension diff-check pnpm --dir web run test pnpm --dir web run typecheck:full" "$(calls)"
expect_output "naming the green the Rust half relies on" "the whole Rust half — a recorded green covers it (tree $(tree | cut -c1-12), this very content)"
expect "now every half is recorded" "preflight rust web" "$(recorded)"
expect_true "and the revision" '[[ "$(cat "$(verified_marker)")" == "$(git -C "$repo" rev-parse HEAD)" ]]'
expect_output "a green run's line is the one before" "==> recorded a --full green for tree $(tree | cut -c1-12)"

full_run
expect_status "a third --full: covered whole, ends at once" 0
expect_true "running nothing" '[[ ! -e "$FAKE/calls" ]]'

# --- cargo fmt red: the Rust half and the preflight, the web half recorded ------------------------
change g
touch "$FAKE/fmt-red"
full_run
expect_status "cargo fmt red: non-zero" 1
expect "only the web half recorded for the new tree" "web" \
    "$(grep " $(tree)$" "$(cd "$repo" && source "$ROOT/scripts/lib/verified.sh" && rd_full_marker "$repo")" | cut -d' ' -f1 | paste -sd' ' -)"
expect_output "named" "==> recorded the web half for tree $(tree | cut -c1-12)"
expect_output "with both red halves" "although the rust and preflight halves failed;"
rm "$FAKE/fmt-red"
full_run
expect_status "the next --full: green" 0
expect "the web half reused, the Rust half and the preflight run" "diff-check fmt rust-tests" "$(calls)"

# --- a stage without a half red: the preflight alone -----------------------------------------------
change h
touch "$FAKE/diff-check-red"
full_run
expect_status "git diff --check red: non-zero" 1
expect_output "the Rust and web halves recorded" "==> recorded the rust and web halves for tree $(tree | cut -c1-12)"
expect_output "although the preflight failed" "although the preflight half failed;"
rm "$FAKE/diff-check-red"
full_run
expect_status "the next --full: green" 0
expect "only the preflight's stage ran" "diff-check" "$(calls)"

# --- the web half red after the Rust half (RD_WEB_SERIAL=1) ----------------------------------------
change i
touch "$FAKE/vitest-red"
export RD_WEB_SERIAL=1
full_run
unset RD_WEB_SERIAL
expect_status "a red web half after the Rust half: non-zero" 1
expect_true "it ran after it" '! grep -qF "runs beside the Rust half" <<< "$output"'
expect_output "the same record" "==> recorded the rust and preflight halves for tree $(tree | cut -c1-12)"
rm "$FAKE/vitest-red"

# --- every half red: nothing recorded --------------------------------------------------------------
change j
touch "$FAKE/vitest-red" "$FAKE/fmt-red"
full_run
expect_status "every half red: non-zero" 1
expect_output "and no green recorded" "No green was recorded."
expect_true "nothing for the tree" '! grep -q " $(tree)$" "$CARGO_TARGET_DIR/.rd-verified-full/"*'

finish_tests check-record
