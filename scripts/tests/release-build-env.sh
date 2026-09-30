#!/usr/bin/env bash
#
# scripts/release-build-env.sh on a scratch repository with a fixed commit time (RD-180-12): the
# stamp comes from the commit and not the clock, a Linux target gets its paths remapped through
# the target's own variables and never RUSTFLAGS, Windows keeps .cargo/config.toml's flags
# untouched, two checkouts in different places map to the same prefixes, and a release tag gets
# rd_build_stamp's `Release-Build X.Y.Z (Basis <sha>)` or stops when it is not Cargo.toml's version.
#
#   scripts/tests/release-build-env.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

env_of() { "$ROOT/scripts/release-build-env.sh" "$@"; }
# The value of KEY in the last run's output.
value() { sed -n "s/^$1=//p" <<< "$output"; }

repo="$SCRATCH/checkout"
git init -q "$repo"
printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.8.0"\n\n[profile.release]\nversion = "9"\n' > "$repo/Cargo.toml"
git -C "$repo" add Cargo.toml
GIT_COMMITTER_DATE="2026-09-30T12:00:00Z" GIT_AUTHOR_DATE="2026-09-30T12:00:00Z" \
    git -C "$repo" -c user.name=t -c user.email=t@example.invalid commit -q -m one
commit="$(git -C "$repo" rev-parse --short=8 HEAD)"

run_status env_of --target x86_64-unknown-linux-gnu --source "$repo"
expect_status "a Linux target" 0
expect "the epoch is the commit's" "1790769600" "$(value SOURCE_DATE_EPOCH)"
expect "the build time is the commit's, not the clock's" "2026-09-30T12:00:00Z" "$(value RD_BUILD_TIME)"
expect "the commit has eight characters and no -dirty" "$commit" "$(value RD_BUILD_COMMIT)"
expect_output "the checkout is remapped for the target" \
    "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=--remap-path-prefix=$repo=/rdownloader "
expect_output "the C code of the target too" "CFLAGS_x86_64_unknown_linux_gnu=-ffile-prefix-map=$repo=/rdownloader "
expect "RUSTFLAGS itself is never set" "" "$(grep -E '^(CARGO_ENCODED_)?RUSTFLAGS=' <<< "$output" || true)"
expect "no cargo home is exported unless one is asked for" "" "$(value CARGO_HOME)"
expect "the version is the workspace's" "1.8.0" "$(value RD_VERSION)"
linux_a="$(value CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS)"

run_status env_of --target aarch64-unknown-linux-gnu --source "$repo"
expect_output "the arm target is named for itself" "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS=--remap-path-prefix="

# A tracked change would make build.rs say -dirty; the exported stamp names the commit regardless.
echo change > "$repo/tracked"
git -C "$repo" add tracked
run_status env_of --target x86_64-pc-windows-msvc --source "$repo"
expect_status "a Windows target" 0
expect "Windows gets the stamp, even from a dirty tree" "$commit" "$(value RD_BUILD_COMMIT)"
expect "Windows' target flags stay .cargo/config.toml's" "" "$(grep -E 'RUSTFLAGS|FLAGS_' <<< "$output" || true)"

moved="$SCRATCH/rebuild/elsewhere"
mkdir -p "$moved"
git clone -q "$repo" "$moved/rdownloader"
run_status env_of --target x86_64-unknown-linux-gnu --source "$moved/rdownloader" --cargo-home "$SCRATCH/cargo-b"
expect_status "a second checkout with its own cargo home" 0
expect "the cargo home is created and exported" "$SCRATCH/cargo-b" "$(value CARGO_HOME)"
expect_output "and remapped to the same prefix" "--remap-path-prefix=$SCRATCH/cargo-b=/cargo"
expect "both checkouts map to the same flags once their paths are replaced" \
    "${linux_a//$repo=/SRC=}" "$(value CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS | sed "s|$moved/rdownloader=|SRC=|; s|$SCRATCH/cargo-b=|${CARGO_HOME:-$HOME/.cargo}=|")"

run_status env_of --target x86_64-unknown-linux-gnu --release-tag v1.8.0 --source "$repo"
expect_status "a release tag that is Cargo.toml's version" 0
expect "the release's commit line, as VERSION.txt has it" "Release-Build 1.8.0 (Basis $commit)" "$(value RD_BUILD_COMMIT)"
expect "the release's time is still the commit's" "2026-09-30T12:00:00Z" "$(value RD_BUILD_TIME)"
expect "an exported stamp is not taken over" "Release-Build 1.8.0 (Basis $commit)" \
    "$(RD_BUILD_COMMIT=stale RD_BUILD_TIME=stale env_of --target x86_64-unknown-linux-gnu --release-tag v1.8.0 --source "$repo" | sed -n 's/^RD_BUILD_COMMIT=//p')"

run_status env_of --target x86_64-unknown-linux-gnu --release-tag v1.8.1 --source "$repo"
expect_status "a tag that is not Cargo.toml's version stops the build" 1
expect_output "and says why" "RD_RELEASE_VERSION is 1.8.1, but the package is 1.8.0"

run_status env_of --source "$repo"
expect_status "no target is a usage error" 2

finish_tests release-build-env
