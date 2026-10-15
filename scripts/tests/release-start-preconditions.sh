#!/usr/bin/env bash
#
# scripts/release-start.sh refuses, before it detaches anything and compiling nothing, a tree the
# chain would stop on later (RD-1300-04): migration-forward baselines behind the version
# (RD-1280-23; 1.28.0 shipped with their guard red on main), a CHANGELOG.md without the version's
# dated section, RELEASE-NOTES.md without a finished section, docs/roadmap.md not naming the
# version (the 1.29.0 docs gate stopped the chain after about 40 minutes on it), a dirty tree. One
# tree per missing precondition, each refusal naming what fixes it; a complete tree starts the chain
# as before. The documents are checked by the real release-docs.sh and release-notes.sh; the
# pipeline, check.sh, release-branches.sh, migration-baselines.sh and pnpm are stand-ins, so no real
# chain can ever start from here.
#
#   scripts/tests/release-start-preconditions.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# The complete tree every case starts from.
BASE="$SCRATCH/base"
mkdir -p "$BASE/scripts/lib" "$BASE/docs" "$SCRATCH/bin"
cp "$ROOT/scripts/release-start.sh" "$ROOT/scripts/release-docs.sh" "$ROOT/scripts/release-notes.sh" \
    "$BASE/scripts/"
cp "$ROOT/scripts/lib/release-tag.sh" "$ROOT/scripts/lib/release-docs.py" "$BASE/scripts/lib/"
printf '#!/usr/bin/env bash\nexit 0\n' > "$BASE/scripts/release-branches.sh"
printf '#!/usr/bin/env bash\necho "check.sh $*"\n' > "$BASE/scripts/check.sh"
printf '#!/usr/bin/env bash\ntouch "$RD_RELEASE_RUN_DIR/pipeline-started"\n' > "$BASE/scripts/release-pipeline.sh"
# The stand-in answers --check red while FAKE_BASELINES_BEHIND is set.
cat > "$BASE/scripts/migration-baselines.sh" << 'EOF'
#!/usr/bin/env bash
[[ -z "${FAKE_BASELINES_BEHIND:-}" ]] || { echo "the baselines are not at $2 yet"; exit 1; }
echo "the baselines are at $2"
EOF
printf '#!/usr/bin/env bash\necho "pnpm $*"\n' > "$SCRATCH/bin/pnpm"
chmod +x "$BASE"/scripts/*.sh "$SCRATCH/bin/pnpm"
cat > "$BASE/CHANGELOG.md" << 'EOF'
# Changelog

## [Unreleased]

## [9.9.9] - 2026-10-15

### Fixed

- Something.
EOF
printf '# Release notes\n\n## 9.9.9\n\n- Downloads resume where they stopped.\n' > "$BASE/RELEASE-NOTES.md"
printf '# Roadmap\n\n- 9.9.9: released.\n' > "$BASE/docs/roadmap.md"
git init -q "$BASE"
git -C "$BASE" add -A
git -C "$BASE" commit -qm base

# case <name> [shell run in the copy before its commit]: a copy of the complete tree, changed and
# committed, whose release-start.sh then runs; $tree and $run name it and its run directory.
case_tree() {
    tree="$SCRATCH/$1"
    run="$SCRATCH/run-$1"
    cp -a "$BASE" "$tree"
    if [[ -n "${2:-}" ]]; then
        (cd "$tree" && eval "$2")
        git -C "$tree" commit -qam "$1"
    fi
}
start() { run_status env PATH="$SCRATCH/bin:$PATH" RD_RELEASE_RUN_DIR="$run" "$tree/scripts/release-start.sh" "$@"; }
nothing_detached() {
    expect "$1: nothing detached" "absent" "$([[ -e "$run/pid" || -e "$run/chain.log" ]] && echo present || echo absent)"
}

case_tree complete
start 9.9.9
expect_status "a complete tree: the chain starts" 0
expect_output "saying so" "release chain for 9.9.9 started, detached"
for _ in $(seq 1 100); do
    [[ -s "$run/exit" ]] && break
    sleep 0.1
done
expect "and runs to its end" "0" "$(cat "$run/exit" 2> /dev/null)"
expect_true "through the web build, check.sh --full and the pipeline" \
    'grep -qx "pnpm --dir web run build" "$run/chain.log" && grep -qx "check.sh --full" "$run/chain.log" && [[ -e "$run/pipeline-started" ]]'

case_tree baselines
FAKE_BASELINES_BEHIND=1 start 9.9.9
expect_status "baselines behind the version: refused" 1
expect_output "with the check's own finding" "the baselines are not at 9.9.9 yet"
expect_output "naming the step to run" "run scripts/migration-baselines.sh 9.9.9 and commit it"
expect_output "and how to start again" "release 9.9.9 not started; fix the above, then run scripts/release-start.sh 9.9.9 again"
nothing_detached "baselines"

case_tree changelog "sed -i '/^## \[9.9.9\]/d' CHANGELOG.md"
start 9.9.9
expect_status "CHANGELOG.md without the version's section: refused" 1
expect_output "naming the cut" "CHANGELOG.md has no '## [9.9.9] - YYYY-MM-DD' section: run scripts/release-docs.sh 9.9.9"
nothing_detached "changelog"

case_tree undated "sed -i 's/^## \[9.9.9\] - .*/## [9.9.9]/' CHANGELOG.md"
start 9.9.9
expect_status "a section without its date: refused" 1
expect_output "the same finding" "CHANGELOG.md has no '## [9.9.9] - YYYY-MM-DD' section"

case_tree draft "sed -i 's/^## 9.9.9$/&\n\n<!-- draft: the notes of the version in the making -->/' RELEASE-NOTES.md"
start 9.9.9
expect_status "RELEASE-NOTES.md with the version still a draft: refused" 1
expect_output "with release-notes.sh's finding" "the section 9.9.9 is still a draft"
expect_output "naming what fixes it" "RELEASE-NOTES.md: finish the section of 9.9.9"
nothing_detached "draft"

case_tree notes "printf '# Release notes\n' > RELEASE-NOTES.md"
start 9.9.9
expect_status "RELEASE-NOTES.md without the version's section: refused" 1
expect_output "with release-notes.sh's finding" 'no section "## 9.9.9" for the release'
nothing_detached "notes"

case_tree roadmap "printf '# Roadmap\n\n- 9.9.8: released.\n' > docs/roadmap.md"
start 9.9.9
expect_status "docs/roadmap.md not naming the version: refused" 1
expect_output "naming what fixes it" "docs/roadmap.md never names 9.9.9: write it into the milestone's status line"
nothing_detached "roadmap"

case_tree dirty
echo "work in progress" >> "$tree/CHANGELOG.md"
start 9.9.9
expect_status "a dirty tree: refused" 1
expect_output "saying so" "the working tree is not clean; commit what belongs to the release"
expect_output "naming the file" " M CHANGELOG.md"
nothing_detached "dirty"

case_tree everything "sed -i '/^## \[9.9.9\]/d' CHANGELOG.md; printf '# Roadmap\n' > docs/roadmap.md"
FAKE_BASELINES_BEHIND=1 start 9.9.9
expect_status "several preconditions missing: refused" 1
expect_true "each one named in the same run" \
    '[[ "$output" == *"migration-baselines.sh 9.9.9"* && "$output" == *"release-docs.sh 9.9.9"* && "$output" == *"never names 9.9.9"* ]]'

case_tree help
start 9.9.9-beta.1 --help
expect_status "the checks do not stand in the way of --help" 0

finish_tests release-start-preconditions
