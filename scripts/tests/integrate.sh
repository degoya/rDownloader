#!/usr/bin/env bash
#
# The merge half of scripts/integrate.sh against a scratch repository (RD-140-22), with
# --merge-only: nothing here compiles. The integration worktree is created from the base, each
# branch merged once, a duplicate migration number or plugin id stops the run naming the files,
# a conflict only in generated files takes our side, and any other conflict stops the run with
# the merge left for a person — and a second run refuses until it is committed. The merge drivers
# (RD-1100-13): CHANGELOG.md as the union of both sides, the migration pins as their sorted union,
# a locale catalogue key by key, and a real conflict in either still stops the run.
#
# Past the merges, with stand-ins for check.sh, the generators and build-plugins.sh that log
# their calls (RD-1100-13): a red gate stops the run before any generator with every error shown,
# a green one lets the generators run and archive-jobs' rewrite into the generated commit, and
# the detached check runs --windows first and --full only after a green Windows lint.
#
#   scripts/tests/integrate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export CARGO_TARGET_DIR="$SCRATCH/target"
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE"
# worktree.sh installs web/node_modules with pnpm (RD-150-14); nothing here needs a real one.
mkdir -p "$SCRATCH/bin"
printf '#!/usr/bin/env bash\nexit 0\n' > "$SCRATCH/bin/pnpm"
chmod +x "$SCRATCH/bin/pnpm"
export PATH="$SCRATCH/bin:$PATH"

MAIN="$SCRATCH/repo"
git init -q -b development "$MAIN"
mkdir -p "$MAIN/scripts/lib" "$MAIN/web/node_modules" "$MAIN/web/dist" "$MAIN/crates/rd-db/migrations"
cp "$ROOT/scripts/integrate.sh" "$ROOT/scripts/worktree.sh" "$MAIN/scripts/"
cp "$ROOT/scripts/lib/integrate.sh" "$ROOT/scripts/lib/verified.sh" "$ROOT/scripts/lib/lanes.sh" \
    "$ROOT/scripts/lib/workspace-version.sh" "$MAIN/scripts/lib/"
cp -r "$ROOT/scripts/lib/merge-drivers" "$MAIN/scripts/lib/"
cp "$ROOT/.gitattributes" "$MAIN/"
printf 'node_modules/\ndist/\n' > "$MAIN/web/.gitignore"
echo '{"version": "base"}' > "$MAIN/web/openapi.json"
echo 'base' > "$MAIN/shared.txt"
echo 'CREATE TABLE a (id INTEGER);' > "$MAIN/crates/rd-db/migrations/0001_a.sql"
printf '# Changelog\n\n## [Unreleased]\n\n### Added\n\n## [1.0.0]\n' > "$MAIN/CHANGELOG.md"
echo 'aaa  0001_a.sql' > "$MAIN/crates/rd-db/migrations.sha384"
mkdir -p "$MAIN/web/src/locales/de" "$MAIN/docs/roadmap/jobs"
printf '{\n  "a": {\n    "x": "1"\n  }\n}\n' > "$MAIN/web/src/locales/de/common.json"
echo '| Job Inventory | 1 |' > "$MAIN/docs/roadmap/jobs/README.md"

# Stand-ins for everything past the merges: each logs its call. check.sh fails --gate while
# $FAKE/gate-red exists, with a failure list as the real one writes it, and --windows while
# $FAKE/windows-red does; archive-jobs.sh rewrites the job index as its recount would.
stub() {
    printf '#!/usr/bin/env bash\necho "%s $*" >> "$FAKE/calls"\n%s\n' "$1" "${2:-}" > "$MAIN/scripts/$1"
    chmod +x "$MAIN/scripts/$1"
}
stub check.sh '
if [[ "$*" == --gate && -f "$FAKE/gate-red" ]]; then
    printf "%s\n" "the Linux lint — exit 101, log gate-01.log" "    error[E0425]: cannot find value" > "$RD_CHECK_LOGS/failures"
    exit 1
fi
if [[ "$*" == --windows && -f "$FAKE/windows-red" ]]; then exit 1; fi
echo "==> all requested checks passed"'
stub api-contract.sh
stub mcp-coverage.sh
stub web-declarations.sh
stub licenses.sh
stub archive-jobs.sh 'echo "| Job Inventory | 2 |" > docs/roadmap/jobs/README.md'
stub build-plugins.sh
stub prune-target.sh
git -C "$MAIN" add -A
git -C "$MAIN" commit -qm base

# A branch from development with one commit that writes $2 into file $3.
branch() {
    git -C "$MAIN" checkout -q -b "$1" development
    mkdir -p "$(dirname "$MAIN/$3")"
    printf '%s\n' "$2" > "$MAIN/$3"
    git -C "$MAIN" add -A
    git -C "$MAIN" commit -qm "$1"
    git -C "$MAIN" checkout -q development
}
branch feat/one 'one' one.txt
branch feat/two 'two' two.txt
branch feat/migration 'CREATE TABLE b (id INTEGER);' crates/rd-db/migrations/0002_b.sql
branch feat/same-number 'CREATE TABLE c (id INTEGER);' crates/rd-db/migrations/0002_c.sql
branch feat/plugin-a 'id = "019d-0160"' plugins/a/manifest.toml
branch feat/plugin-b 'id = "019d-0160"' plugins/b/manifest.toml
branch feat/api-one '{"version": "one"}' web/openapi.json
branch feat/api-two '{"version": "two"}' web/openapi.json
branch feat/edit-one 'edited by one' shared.txt
branch feat/edit-two 'edited by two' shared.txt

integrate() { run_status "$MAIN/scripts/integrate.sh" "$@" --merge-only; }
TREE="$MAIN-integration-w1"

integrate integration/w1 feat/one feat/two feat/migration
expect_status "three clean branches" 0
expect "the integration branch lives in its own worktree" "integration/w1" "$(git -C "$TREE" branch --show-current)"
expect_true "and carries all three" 'for b in feat/one feat/two feat/migration; do git -C "$TREE" merge-base --is-ancestor "$b" HEAD || exit 1; done'
expect "each as a merge commit" "3" "$(git -C "$TREE" rev-list --merges --count development..HEAD)"
expect_output "no duplicates" "no duplicate migration number, no duplicate plugin id"
expect_true "the main checkout is not moved" '[[ "$(git -C "$MAIN" branch --show-current)" == development ]]'

integrate integration/w1 feat/one feat/two feat/migration
expect_status "a second run" 0
expect_output "skips what is merged" "feat/one: already merged"
expect "and merges nothing again" "3" "$(git -C "$TREE" rev-list --merges --count development..HEAD)"

integrate integration/w1 feat/same-number
expect_status "a second migration 0002 stops the run" 1
expect_output "naming both files" "duplicate migration number 0002: 0002_b.sql 0002_c.sql"
git -C "$TREE" reset -q --hard HEAD^

integrate integration/w1 feat/plugin-a feat/plugin-b
expect_status "a second plugin with the same id stops the run" 1
expect_output "naming both manifests" 'duplicate plugin id = "019d-0160": plugins/a/manifest.toml plugins/b/manifest.toml'
git -C "$TREE" reset -q --hard HEAD^

integrate integration/w1 feat/api-one feat/api-two
expect_status "a conflict only in a generated file is resolved" 0
expect_output "and says the generators rewrite it" "generated files in conflict took our side"
expect "our side, which the generators then replace" '{"version": "one"}' "$(cat "$TREE/web/openapi.json")"

integrate integration/w1 feat/edit-one feat/edit-two
expect_status "a conflict in a source file stops the run" 1
expect_output "naming the file" "shared.txt"
expect_true "the merge is left in progress for a person" 'git -C "$TREE" rev-parse --verify --quiet MERGE_HEAD > /dev/null'
integrate integration/w1 feat/edit-two
expect_status "a run while the merge is unresolved is refused" 1
expect_output "saying why" "a merge is in progress"

# --- the merge drivers (RD-1100-13) --------------------------------------------------------------
added() { printf '# Changelog\n\n## [Unreleased]\n\n### Added\n\n- %s\n\n## [1.0.0]\n' "$1"; }
branch feat/log-one "$(added one)" CHANGELOG.md
branch feat/log-two "$(added two)" CHANGELOG.md
branch feat/pin-one $'aaa  0001_a.sql\nbbb  0002_b.sql' crates/rd-db/migrations.sha384
branch feat/pin-two $'aaa  0001_a.sql\nccc  0003_c.sql' crates/rd-db/migrations.sha384
branch feat/pin-again $'aaa  0001_a.sql\nddd  0002_b.sql' crates/rd-db/migrations.sha384
branch feat/key-one $'{\n  "a": {\n    "x": "1",\n    "y": "ypsilon"\n  }\n}' web/src/locales/de/common.json
branch feat/key-two $'{\n  "a": {\n    "x": "1",\n    "z": "zett"\n  },\n  "b": "neu"\n}' web/src/locales/de/common.json
branch feat/key-again $'{\n  "a": {\n    "x": "eins",\n    "y": "anders"\n  }\n}' web/src/locales/de/common.json
TREE2="$MAIN-integration-w2"

integrate integration/w2 feat/log-one feat/log-two feat/pin-one feat/pin-two feat/key-one feat/key-two
expect_status "CHANGELOG, pins and a catalogue that git alone calls conflicts merge" 0
expect_true "the drivers are registered" '[[ "$(git -C "$MAIN" config merge.rd-pins.driver)" == *migration-pins.sh* ]]'
expect "CHANGELOG.md: both entries" "- one|- two" "$(grep '^- ' "$TREE2/CHANGELOG.md" | paste -sd'|' -)"
expect "the pins: every line once, in migration order" "aaa  0001_a.sql|bbb  0002_b.sql|ccc  0003_c.sql" \
    "$(paste -sd'|' - < "$TREE2/crates/rd-db/migrations.sha384")"
expect "the catalogue: both keys, ours first, as i18n-key.sh writes it" \
    "$(printf '{\n  "a": {\n    "x": "1",\n    "y": "ypsilon",\n    "z": "zett"\n  },\n  "b": "neu"\n}')" \
    "$(cat "$TREE2/web/src/locales/de/common.json")"

integrate integration/w2 feat/pin-again
expect_status "a migration pinned with two sums is a conflict" 1
expect_output "for a person" "crates/rd-db/migrations.sha384"
expect_true "with markers" 'grep -q "^<<<<<<<" "$TREE2/crates/rd-db/migrations.sha384"'
git -C "$TREE2" merge --abort

integrate integration/w2 feat/key-again
expect_status "a key both sides changed is a conflict" 1
expect_output "naming the catalogue" "web/src/locales/de/common.json"
expect_true "merged as text, with markers" 'grep -q "^<<<<<<<" "$TREE2/web/src/locales/de/common.json"'
git -C "$TREE2" merge --abort

# --- the gate, the generators and the detached check (RD-1100-13) ---------------------------------
# shellcheck disable=SC2034  # read inside the expect_true strings
TREE3="$MAIN-integration-w3"
export RD_INTEGRATE_LOGS="$SCRATCH/logs-w3"
touch "$FAKE/gate-red"
run_status "$MAIN/scripts/integrate.sh" integration/w3 feat/one --no-check
expect_status "a red gate stops the run" 1
expect_output "saying so" "the gate is red"
expect_output "with every error from the failure list" "error[E0425]: cannot find value"
expect_true "the gate ran in the integration worktree" 'grep -qx "check.sh --gate" "$FAKE/calls"'
expect_true "and no generator after it" '! grep -q "api-contract.sh" "$FAKE/calls"'

rm -f "$FAKE/gate-red" "$FAKE/calls"
run_status "$MAIN/scripts/integrate.sh" integration/w3 feat/one --no-check
expect_status "a green gate lets the generators run" 0
expect "the gate first, then the generators, archive-jobs among them" \
    "check.sh --gate|api-contract.sh |mcp-coverage.sh |web-declarations.sh |archive-jobs.sh " \
    "$(grep -E '^(check|api-contract|mcp-coverage|web-declarations|archive-jobs)' "$FAKE/calls" | paste -sd'|' -)"
expect_true "archive-jobs' rewrite is in the generated commit" \
    'git -C "$TREE3" log -1 --format=%s | grep -q "^chore(generated)" && git -C "$TREE3" show --name-only HEAD | grep -qx "docs/roadmap/jobs/README.md"'

# The detached check: --windows first, --full only after it is green.
wait_status() {
    local waited=0
    while [[ ! -f "$RD_INTEGRATE_LOGS/status" && "$waited" -lt 100 ]]; do sleep 0.1; waited=$((waited + 1)); done
    cat "$RD_INTEGRATE_LOGS/status" 2> /dev/null || echo "no status"
}
rm -f "$FAKE/calls"
run_status "$MAIN/scripts/integrate.sh" integration/w3 feat/one --no-gate
expect_status "the detached check starts" 0
expect "both green, and the prune" "full=0 windows=0 prune=0" "$(wait_status)"
expect "--windows before --full" "check.sh --windows|check.sh --full" \
    "$(grep '^check.sh' "$FAKE/calls" | paste -sd'|' -)"
expect_true "the run writes its failures beside its logs" 'grep -qx "export RD_CHECK_LOGS=.$RD_INTEGRATE_LOGS." "$RD_INTEGRATE_LOGS/run.sh"'

touch "$FAKE/windows-red"
rm -f "$FAKE/calls"
run_status "$MAIN/scripts/integrate.sh" integration/w3 feat/one --no-gate
expect "a red Windows lint holds back --full" "full=skipped windows=1 prune=skipped" "$(wait_status)"
expect_true "which never started" '! grep -q "check.sh --full" "$FAKE/calls"'
rm -f "$FAKE/windows-red"
unset RD_INTEGRATE_LOGS

integrate integration/w1 feat/nope
expect_status "a branch that does not exist" 2
run_status "$MAIN/scripts/integrate.sh" integration/w1
expect_status "no branch to merge is a usage error" 2

finish_tests integrate
