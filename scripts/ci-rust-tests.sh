#!/usr/bin/env bash
#
# The Rust tests of ci.yml's `rust` job, in groups (RD-120-67), and with --no-run the same build
# for its `warm-cache` job (RD-1100-13), which has to compile exactly what `rust` compiles for the
# cache it saves to be the one `rust` restores. CI only; locally scripts/check.sh runs the tests.
#
# In groups, each group's test executables deleted once it ran. The first run on GitHub
# (2026-09-25) filled the disk under one `--workspace` run: the step log stops at "linking with cc
# failed: exit status 1", and the runner died with "No space left on device" writing its own
# diagnostics. That run kept every test executable of the workspace at once, and each rd-api
# integration binary is ~550 MB (this repository's `debug = "line-tables-only"`, which the test
# profile inherits: 120 MB of code, 250 MB of symbol names, 3 MB of debug info). There were 55 of
# them — ~30 GB; since RD-150-10 the suites are modules of six binaries, one per subject, which
# fit in one group. The heavy crates run each alone. The split follows scripts/check.sh, which
# does it for memory on a workstation.
#
# Every group runs to its end, failing or not, and the script fails at the end: one 45-minute run
# has to show every failure, not the first (RD-120-67 took three runs).
#
#   scripts/ci-rust-tests.sh            # -P ci, JUnit per group into $RUNNER_TEMP/junit
#   scripts/ci-rust-tests.sh --no-run   # build every group's executables, run none
set -euo pipefail

# --no-fail-fast only for a real run: nextest 0.9.146 refuses it beside --no-run, and the
# release push's warm-cache job, the one caller with --no-run, failed on v1.10.0 for it.
no_run=()
fail_fast=(--no-fail-fast)
case "${1:-}" in
    --no-run) no_run=(--no-run); fail_fast=() ;;
    "") ;;
    *) echo "usage: scripts/ci-rust-tests.sh [--no-run]" >&2; exit 2 ;;
esac

temp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
stamp="${temp}/rd-tests.stamp"
# Each group overwrites target/nextest/ci/junit.xml, so every report is copied out.
reports="${temp}/junit"
mkdir -p "${reports}"
group=0
failed=0

# One nextest selection, then the executables it linked go: nothing later needs them. No
# extension (or `.exe`) in target/debug/deps is a test or binary executable; libraries, proc
# macros and metadata all carry one and stay.
run_group() {
    echo "::group::nextest ${no_run[*]+${no_run[*]} }$*"
    touch "${stamp}"
    group=$((group + 1))
    cargo nextest run -P ci ${fail_fast[@]+"${fail_fast[@]}"} ${no_run[@]+"${no_run[@]}"} "$@" || failed=1
    if [[ -f target/nextest/ci/junit.xml ]]; then
        mv target/nextest/ci/junit.xml "${reports}/group-${group}.xml"
    fi
    find target/debug/deps -maxdepth 1 -type f -newer "${stamp}" \
        \( ! -name '*.*' -o -name '*.exe' \) -print \
        | while IFS= read -r executable; do
            # With it everything named after it: `.pdb` on Windows, the `.o` that macOS keeps
            # for its debug info, `.dwo`, `.d`.
            stem="${executable%.exe}"
            rm -f "${executable}" "${stem}".*
        done
    df -h . | tail -n 1
    echo "::endgroup::"
}

run_group --workspace --exclude rd-api --exclude rd-plugin-ext --exclude rdownloader
run_group -p rd-plugin-ext
run_group -p rdownloader
run_group -p rd-api --lib
# The rd-api integration binaries, one per crates/rd-api/tests/<subject>/main.rs, in one group:
# six of them keep ~3.5 GB of executables at once.
args=()
for main in crates/rd-api/tests/*/main.rs; do
    args+=(--test "$(basename "$(dirname "${main}")")")
done
run_group -p rd-api "${args[@]}"
exit "${failed}"
