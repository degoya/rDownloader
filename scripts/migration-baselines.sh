#!/usr/bin/env bash
#
# Moves the migration-forward suite's ratchet to a release (RD-1280-23): the suite keeps the
# newest release of each of the last two minor versions (owner, 2026-10-13), so a release adds
# its own row and drops the row that falls out, with that row's test.
#
# Run before scripts/release-start.sh, and commit the result: the release pipeline starts from a
# clean tree, and its check before the version bump holds the newest row to the version being
# released (`the_baselines_are_the_last_two_releases`, RD_RELEASE_VERSION). A release that added
# no migration gets a row all the same, at the head of the chain.
#
# It writes two places in crates/rd-db/tests/database/:
#   migration_forward.rs            the BASELINES table: release and the highest migration number
#   migration_forward/baselines.rs  the `every_release_baseline!` lines, one test per row
# and names the tests in migration_forward/per_migration.rs that measure a migration the older
# kept release already ships: those go by hand, and a test fails until they have.
#
# The number is the release tag's highest migration when the tag v<version> exists, else the
# highest one in the working tree, which is what the release is about to ship. A pre-release keeps
# the baselines. Running it again for the same version changes nothing.
#
# Usage:
#   scripts/migration-baselines.sh 1.28.0
#   scripts/migration-baselines.sh --check 1.28.0   # exit 1 when the files are not at 1.28.0 yet
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUITE="$ROOT/crates/rd-db/tests/database"
MIGRATIONS="crates/rd-db/migrations"

check=0
version=""
for argument in "$@"; do
    case "$argument" in
        --check) check=1 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $argument" >&2; exit 2 ;;
        *) version="$argument" ;;
    esac
done
if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-beta\.[1-9][0-9]*$ ]]; then
    echo "$version is a pre-release; the baselines stay as they are"
    exit 0
fi
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: scripts/migration-baselines.sh [--check] <X.Y.Z>" >&2
    exit 2
fi

# The highest migration number the release ships, without leading zeros.
if git -C "$ROOT" rev-parse -q --verify "refs/tags/v$version" > /dev/null 2>&1; then
    newest="$(git -C "$ROOT" ls-tree -r --name-only "v$version" "$MIGRATIONS" | sort | tail -1)"
    source_of_number="tag v$version"
else
    newest="$(find "$ROOT/$MIGRATIONS" -maxdepth 1 -name '[0-9]*_*.sql' -printf '%f\n' | sort | tail -1)"
    source_of_number="the working tree"
fi
number="$(basename "$newest" | grep -oE '^[0-9]+' || true)"
if [[ -z "$number" ]]; then
    echo "no migration found in $source_of_number" >&2
    exit 1
fi
number=$((10#$number))

python3 - "$SUITE" "$version" "$number" "$check" <<'PY'
import re
import sys

suite, version, number, check = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4] == '1'
table_path = f'{suite}/migration_forward.rs'
tests_path = f'{suite}/migration_forward/baselines.rs'
measured_path = f'{suite}/migration_forward/per_migration.rs'
table_source = open(table_path).read()
tests_source = open(tests_path).read()

table = re.search(r'const BASELINES: &\[\(&str, i64\)\] = &\[(.*?)\];', table_source, re.S)
if not table:
    sys.exit(f'{table_path}: no BASELINES table')
rows = [(release, int(count)) for release, count in re.findall(r'\("(\d+\.\d+\.\d+)",\s*(\d+)\)', table.group(1))]


def key(release):
    return tuple(int(part) for part in release.split('.'))


def minor(release):
    return key(release)[:2]


kept = [row for row in rows if minor(row[0]) != minor(version)] + [(version, number)]
kept.sort(key=lambda row: key(row[0]))
kept = kept[-2:]
if [count for _, count in kept] != sorted(count for _, count in kept):
    sys.exit(f'a newer release would ship fewer migrations: {kept}')

rendered_rows = ', '.join(f'("{release}", {count})' for release, count in kept)
new_table = f'const BASELINES: &[(&str, i64)] = &[{rendered_rows}];'
macro = re.search(r'every_release_baseline! \{\n(.*?)\n\}', tests_source, re.S)
if not macro:
    sys.exit(f'{tests_path}: no every_release_baseline! block')
lines = '\n'.join(
    f'    the_{release.replace(".", "_")}_baseline_upgrades_with_its_queue_intact => "{release}",'
    for release, _ in kept
)
new_macro = 'every_release_baseline! {\n' + lines + '\n}'

new_table_source = table_source[:table.start()] + new_table + table_source[table.end():]
new_tests_source = tests_source[:macro.start()] + new_macro + tests_source[macro.end():]
changed = new_table_source != table_source or new_tests_source != tests_source
if check:
    if changed:
        print(f'the baselines are not at {version} yet: {rows} -> {kept}')
        sys.exit(1)
    print(f'the baselines are at {version}: {kept}')
    sys.exit(0)
if changed:
    open(table_path, 'w').write(new_table_source)
    open(tests_path, 'w').write(new_tests_source)
    dropped = [row for row in rows if row not in kept]
    print(f'baselines: {kept}' + (f'; dropped {dropped} with their tests' if dropped else ''))
else:
    print(f'baselines already at {version}: {kept}')

oldest = kept[0][1]
try:
    measured = open(measured_path).read()
except FileNotFoundError:
    measured = ''
stale = sorted({int(n) for n in re.findall(r'schema_at\(directory\.path\(\), (\d+)\)', measured) if int(n) < oldest})
for schema in stale:
    print(f'per_migration.rs: the test on migration {schema + 1:04} goes; both kept releases ship it')
PY
