#!/usr/bin/env bash
#
# scripts/check-actions-pinned.sh against fixture workflows in a temp directory: commit pins,
# quoted or with their release comment, local actions and a docker image by digest pass; a tag,
# a branch, an abbreviated commit, a docker image by tag and an action without any ref fail, each
# named with its file and line; a commented-out `uses:` is not read. Last, this checkout's own
# workflows, which have to pass as they are.
#
# Pure bash: it runs in well under a second.
#
#   scripts/tests/check-actions-pinned.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/check-actions-pinned.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

SHA="0123456789abcdef0123456789abcdef01234567"
DIGEST="$(printf 'a%.0s' {1..64})"

cat > "$SCRATCH/pinned.yml" <<EOF
jobs:
  a:
    steps:
      - uses: actions/checkout@$SHA # v7.0.1
      - name: quoted
        uses: "Swatinem/rust-cache@$SHA"
      - uses: './.github/actions/free-runner-disk'
      - &anchored
        uses: taiki-e/install-action@$SHA # v2.87.22
        with:
          tool: nextest
      - uses: docker://alpine@sha256:$DIGEST
      # - uses: actions/checkout@v7
  b:
    uses: owner/repo/.github/workflows/reusable.yml@$SHA # v1.0.0
EOF

cat > "$SCRATCH/unpinned.yml" <<EOF
jobs:
  a:
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@${SHA:0:12}
      - uses: docker://alpine:3.22
      - uses: some/action
      - uses: actions/setup-node@$SHA # v7.0.0
EOF

run_status "$SCRIPT" "$SCRATCH/pinned.yml"
expect_status "commit pins, local actions and a digest pass" 0
expect_output "every reference is counted" "6 action reference(s) in 1 file(s)"

run_status "$SCRIPT" "$SCRATCH/pinned.yml" "$SCRATCH/unpinned.yml"
expect_status "a file with unpinned references fails" 1
expect_output "a major tag is named with its line" "$SCRATCH/unpinned.yml:4: actions/checkout@v7 is not pinned"
expect_output "a branch is named" "unpinned.yml:5: dtolnay/rust-toolchain@stable is not pinned"
expect_output "an abbreviated commit is not a pin" "unpinned.yml:6: actions/cache@${SHA:0:12} is not pinned"
expect_output "a docker image by tag is not a pin" "unpinned.yml:7: docker://alpine:3.22 is not pinned"
expect_output "an action without a ref is not a pin" "unpinned.yml:8: some/action is not pinned"
expect_output "the count covers both files" "5 of 12 action reference(s) not pinned"
expect "the pinned line of the same file is not named" "0" \
    "$(grep -c 'unpinned.yml:9:' <<< "$output" || true)"

: > "$SCRATCH/empty.yml"
run_status "$SCRIPT" "$SCRATCH/empty.yml"
expect_status "a file without actions passes" 0

run_status "$SCRIPT"
expect_status "this checkout's workflows are pinned" 0

finish_tests check-actions-pinned
