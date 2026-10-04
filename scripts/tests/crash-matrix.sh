#!/usr/bin/env bash
#
# scripts/lib/crash-matrix.sh (RD-191-09): the nextest runs and the trigger crates it derives from
# scripts/lib/crash-matrix.list, on a scratch tree, then the real list against the workspace —
# every listed crate has a `failpoints` feature, and every crate with one is listed or reached
# through a listed crate's feature, so a new owning crate cannot be forgotten.
#
# Pure bash, no cargo. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/crash-matrix.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/crash-matrix.sh
source "$ROOT/scripts/lib/crash-matrix.sh"

cd "$SCRATCH"
mkdir -p crates/rd-core crates/rd-a crates/rd-b crates/rd-api crates/rd-api-admin
printf '[features]\nfailpoints = []\n' > crates/rd-core/Cargo.toml
printf '[features]\nfailpoints = ["rd-core/failpoints"]\n' \
    | tee crates/rd-a/Cargo.toml crates/rd-b/Cargo.toml crates/rd-api-admin/Cargo.toml > /dev/null
printf '[features]\nfailpoints = ["rd-api-admin/failpoints"]\n' > crates/rd-api/Cargo.toml
cat > list <<'EOF2'
# a comment
rd-core
rd-a

rd-api   --test admin stopped_updates
rd-b     --test b_crash
EOF2
export RD_CRASH_MATRIX_LIST=list

expect "the shared run first, then each run with a selection alone" \
    "--features rd-core/failpoints,rd-a/failpoints -p rd-core -p rd-a
--features rd-api/failpoints -p rd-api --test admin stopped_updates
--features rd-b/failpoints -p rd-b --test b_crash" "$(rd_crash_matrix_runs)"
expect "owners trigger; a driver's forwarded crate does, the driver does not" \
    "rd-a rd-api-admin rd-b rd-core" "$(rd_crash_matrix_triggers | paste -sd' ' -)"
printf 'rd-a --test only\n' > list
expect "no shared run without a package for it" "--features rd-a/failpoints -p rd-a --test only" \
    "$(rd_crash_matrix_runs)"

# --- the real list ---------------------------------------------------------------------------------

cd "$ROOT"
RD_CRASH_MATRIX_LIST=scripts/lib/crash-matrix.list
missing=""
while read -r package _; do
    [[ -n "$(rd_crash_matrix_feature "$package")" || "$package" == rd-core ]] || missing+=" $package"
done < <(rd_crash_matrix_rows)
expect_true "the list was read" '[[ "$(rd_crash_matrix_rows | wc -l)" -gt 10 ]]'
expect "every listed crate has a failpoints feature" "" "$missing"
reached="$( { rd_crash_matrix_rows | cut -d' ' -f1; rd_crash_matrix_triggers; } | LC_ALL=C sort -u)"
unlisted=""
for manifest in crates/*/Cargo.toml; do
    grep -q '^failpoints *=' "$manifest" || continue
    crate="$(basename "$(dirname "$manifest")")"
    grep -qx "$crate" <<< "$reached" || unlisted+=" $crate"
done
expect "every crate with a failpoints feature is in the matrix" "" "$unlisted"

finish_tests "crash-matrix"
