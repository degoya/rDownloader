#!/usr/bin/env bash
#
# scripts/migration-baselines.sh against a scratch tree (RD-1280-23): from copies of the real
# suite files it adds a release's row, drops the one older than two minor versions with its test,
# replaces a patch release's minor, reads the number off a tag when there is one, leaves a
# pre-release alone, changes nothing on a second run, and names a per-migration test left behind.
#
# Pure bash, python3 and git, no cargo: check.sh runs it whenever something under scripts/
# changes.
#
#   scripts/tests/migration-baselines.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
SUITE="$TREE/crates/rd-db/tests/database"
MIGRATIONS="$TREE/crates/rd-db/migrations"
mkdir -p "$TREE/scripts" "$SUITE/migration_forward" "$MIGRATIONS"
cp "$ROOT/scripts/migration-baselines.sh" "$TREE/scripts/"
cp "$ROOT/crates/rd-db/tests/database/migration_forward.rs" "$SUITE/"
cp "$ROOT/crates/rd-db/tests/database/migration_forward/baselines.rs" "$SUITE/migration_forward/"
cp "$ROOT/crates/rd-db/tests/database/migration_forward/per_migration.rs" "$SUITE/migration_forward/"
baselines() { run_status "$TREE/scripts/migration-baselines.sh" "$@"; }
table() { grep -oE 'const BASELINES: &\[\(&str, i64\)\] = &\[.*\];' "$SUITE/migration_forward.rs"; }
tests() { grep -oE 'the_[0-9_]+_baseline_upgrades_with_its_queue_intact => "[0-9.]+"' "$SUITE/migration_forward/baselines.rs" | tr '\n' '|'; }
migration() { echo "SELECT 1;" > "$MIGRATIONS/$1"; }

# The real files move with every release the script runs for, so their copies start from one
# fixed ratchet, 1.26.0 and 1.27.0 at 0145, in the real files' own format (1.28.1 broke the test
# that read them as they stood).
python3 - "$SUITE/migration_forward.rs" "$SUITE/migration_forward/baselines.rs" << 'EOF'
import re, sys
table, tests = sys.argv[1], sys.argv[2]
s = open(table).read()
s, n = re.subn(r'const BASELINES: &\[\(&str, i64\)\] = &\[.*\];',
               'const BASELINES: &[(&str, i64)] = &[("1.26.0", 145), ("1.27.0", 145)];', s)
assert n == 1, "the BASELINES table"
open(table, "w").write(s)
s = open(tests).read()
s, n = re.subn(r'(every_release_baseline! \{\n)(?:    the_[0-9_]+_baseline_upgrades_with_its_queue_intact => "[0-9.]+",\n)+',
               r'\1    the_1_26_0_baseline_upgrades_with_its_queue_intact => "1.26.0",\n'
               r'    the_1_27_0_baseline_upgrades_with_its_queue_intact => "1.27.0",\n', s)
assert n == 1, "the every_release_baseline! lines"
open(tests, "w").write(s)
EOF

# The copies are where 1.27.0 left them: released at 0145, and nothing came since.
migration 0144_before.sql
migration 0145_category_selectable.sql
baselines --check 1.27.0
expect_status "the real files read as the 1.27.0 ratchet" 0
baselines --check 1.28.0
expect_status "and as not yet at 1.28.0" 1

migration 0146_next.sql
baselines 1.28.0
expect_status "a release moves the ratchet" 0
expect "its row, the 1.26.0 row gone" \
    'const BASELINES: &[(&str, i64)] = &[("1.27.0", 145), ("1.28.0", 146)];' "$(table)"
expect "one test per row, the dropped row's test gone" \
    'the_1_27_0_baseline_upgrades_with_its_queue_intact => "1.27.0"|the_1_28_0_baseline_upgrades_with_its_queue_intact => "1.28.0"|' \
    "$(tests)"
expect_output "says what it dropped" 'dropped [('"'"'1.26.0'"'"', 145)]'
before="$(sha256sum "$SUITE/migration_forward.rs" "$SUITE/migration_forward/baselines.rs")"

baselines 1.28.0
expect_status "a second run" 0
expect "changes nothing" "$before" "$(sha256sum "$SUITE/migration_forward.rs" "$SUITE/migration_forward/baselines.rs")"
baselines --check 1.28.0
expect_status "and --check agrees" 0

baselines 1.28.1
expect_status "a patch release" 0
expect "takes its minor's row" 'const BASELINES: &[(&str, i64)] = &[("1.27.0", 145), ("1.28.1", 146)];' "$(table)"

before="$(sha256sum "$SUITE/migration_forward.rs" "$SUITE/migration_forward/baselines.rs")"
baselines 1.29.0-beta.1
expect_status "a pre-release" 0
expect "keeps the baselines" "$before" "$(sha256sum "$SUITE/migration_forward.rs" "$SUITE/migration_forward/baselines.rs")"

baselines 1.29
expect_status "a version that is no release is refused" 2

# A tag: its migrations count, not the working tree's.
git -C "$TREE" init -q
git -C "$TREE" -c user.name=test -c user.email=test@example.invalid add -A
git -C "$TREE" -c user.name=test -c user.email=test@example.invalid commit -qm tree
git -C "$TREE" tag v1.29.0
migration 0147_after_the_tag.sql
cat >> "$SUITE/migration_forward/per_migration.rs" <<'EOF'
async fn left_behind() { schema_at(directory.path(), 120); }
async fn still_new() { schema_at(directory.path(), 146); }
EOF
baselines 1.29.0
expect_status "a tagged release" 0
expect "reads its number off the tag" 'const BASELINES: &[(&str, i64)] = &[("1.28.1", 146), ("1.29.0", 146)];' "$(table)"
expect_output "names a per-migration test both kept releases ship" "the test on migration 0121 goes"
expect "and only that one" "1" "$(grep -c 'per_migration.rs:' <<< "$output")"

finish_tests migration-baselines
