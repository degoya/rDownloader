#!/usr/bin/env bash
#
# The rd-api suite selection in scripts/lib/scope.sh (RD-120-58, RD-150-10) against a scratch
# tree: suites are modules of crates/rd-api/tests/<binary>/main.rs, the map names suites, and
# check.sh runs the binaries holding the selected ones through one nextest filter. What is
# tested is which suites a changed path selects, which binaries and filter that gives, and the
# upkeep the map check refuses — not nextest.
#
# Pure bash, no cargo. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/rd-api-suites.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"

cd "$SCRATCH"
tests=crates/rd-api/tests
mkdir -p "$tests/common" "$tests/access" "$tests/mcp/mcp"
touch "$tests/common/mod.rs" "$tests/access/auth.rs" "$tests/access/mfa.rs" \
    "$tests/mcp/mcp.rs" "$tests/mcp/mcp/everything.rs"
printf 'mod common;\n\nmod auth;\nmod mfa;\n' > "$tests/access/main.rs"
printf 'mod common;\n\nmod mcp;\n' > "$tests/mcp/main.rs"
cat > map <<'EOF'
# a comment
^crates/rd-api/src/lib\.rs$            all
^crates/rd-api/src/auth\.rs$           auth mfa
^crates/rd-api/src/routes/             + mcp
^crates/rd-api/src/mcp/                mcp
^crates/rd-api/src/tests\.rs$          -
EOF

demands() { printf '%s\n' "$@" | rd_api_test_demands map; }

# --- the layout ----------------------------------------------------------------------------------
expect "the binaries are the directories with a main.rs" "access mcp" "$(rd_api_test_binaries | tr '\n' ' ' | sed 's/ $//')"
expect "the suites are their other files, submodules and common not counted" \
    "auth access|mcp mcp|mfa access" "$(rd_api_test_suites | tr '\n' '|' | sed 's/|$//')"
expect "a suite's binary" "access" "$(rd_api_test_binaries_of mfa)"
expect "several suites, each binary once" "access mcp" "$(rd_api_test_binaries_of auth mcp mfa | tr '\n' ' ' | sed 's/ $//')"
expect "no suite, no binary" "" "$(rd_api_test_binaries_of)"
expect "the filter names the suites' module paths" 'test(/^(auth|mcp)::/)' "$(rd_api_test_filter auth mcp)"

# --- what a changed path selects -----------------------------------------------------------------
expect "a suite file selects itself" "mfa $tests/access/mfa.rs" "$(demands "$tests/access/mfa.rs")"
expect "a suite's submodule selects the suite" "mcp $tests/mcp/mcp/everything.rs" \
    "$(demands "$tests/mcp/mcp/everything.rs")"
expect "a binary's main.rs selects its suites" "auth $tests/access/main.rs|mfa $tests/access/main.rs" \
    "$(demands "$tests/access/main.rs" | tr '\n' '|' | sed 's/|$//')"
expect "the shared harness selects everything" "all $tests/common/mod.rs" "$(demands "$tests/common/mod.rs")"
expect "a deleted suite selects nothing" "" "$(demands "$tests/access/gone.rs")"
expect "a mapped source selects its row's suites" "auth crates/rd-api/src/auth.rs|mfa crates/rd-api/src/auth.rs" \
    "$(demands crates/rd-api/src/auth.rs | tr '\n' '|' | sed 's/|$//')"
expect "a '+' row adds without mapping, so the rest is everything" \
    "mcp crates/rd-api/src/routes/x.rs|all crates/rd-api/src/routes/x.rs" \
    "$(demands crates/rd-api/src/routes/x.rs | tr '\n' '|' | sed 's/|$//')"
expect "'-' maps to nothing" "" "$(demands crates/rd-api/src/tests.rs)"
expect "an unmapped rd-api path selects everything" "all crates/rd-api/src/new.rs" "$(demands crates/rd-api/src/new.rs)"
expect "an unmapped path in one of rd-api's crates selects everything" "all crates/rd-api-core/src/new.rs" \
    "$(demands crates/rd-api-core/src/new.rs)"
expect "another crate selects nothing" "" "$(demands crates/rd-http/src/lib.rs)"

# --- the upkeep the map check refuses ------------------------------------------------------------
expect "a sound map has no findings" "" "$(rd_api_test_map_problems map)"
touch "$tests/access/sessions.rs"
echo '^crates/rd-api/src/gone\.rs$ vanished' >> map
problems="$(rd_api_test_map_problems map)"
expect_true_problem() { expect "$1" 1 "$(grep -cF -- "$2" <<< "$problems")"; }
expect_true_problem "a row naming a suite that does not exist" "a row names a suite that does not exist: vanished"
expect_true_problem "a suite no row names" "no row names the suite: sessions"
expect_true_problem "a suite its main.rs does not declare" "access/main.rs does not declare the suite: sessions"
touch "$tests/stray.rs"
problems="$(rd_api_test_map_problems map)"
expect_true_problem "a test file outside the binaries" "outside the binaries (make it a suite of one): $tests/stray.rs"

finish_tests rd-api-suites
