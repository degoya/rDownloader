#!/usr/bin/env bash
#
# scripts/release-start.sh refuses a release whose migration-forward baselines are not at its
# version (RD-1280-23), before it detaches anything: 1.28.0 shipped with the baseline guard red on
# main because the chain's test step reused the green before the version bump. Runs the real
# release-start.sh in a scratch tree whose release-pipeline.sh, release-branches.sh and
# migration-baselines.sh are stand-ins, so no chain can ever start from here.
#
#   scripts/tests/release-start-baselines.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$(mktemp -d)"
trap 'rm -rf "$TREE"' EXIT
mkdir -p "$TREE/scripts/lib" "$TREE/run"
cp "$ROOT/scripts/release-start.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/release-tag.sh" "$TREE/scripts/lib/"
printf '#!/usr/bin/env bash\nexit 0\n' > "$TREE/scripts/release-branches.sh"
printf '#!/usr/bin/env bash\ntouch "%s/run/pipeline-started"\n' "$TREE" > "$TREE/scripts/release-pipeline.sh"
# The stand-in answers --check with the exit code the test sets in $TREE/run/baselines.
cat > "$TREE/scripts/migration-baselines.sh" << EOF
#!/usr/bin/env bash
code="\$(cat "$TREE/run/baselines")"
[[ "\$code" -eq 0 ]] || echo "the baselines are not at \$2 yet"
exit "\$code"
EOF
chmod +x "$TREE"/scripts/*.sh

start() { run_status env RD_RELEASE_RUN_DIR="$TREE/run" "$TREE/scripts/release-start.sh" "$@"; }

echo 1 > "$TREE/run/baselines"
start 9.9.9
expect_status "baselines behind the version: refused" 1
expect_output "naming the step to run" "run scripts/migration-baselines.sh 9.9.9 and commit it"
expect_output "with the check's own finding" "the baselines are not at 9.9.9 yet"
expect "nothing detached" "absent" "$([[ -e "$TREE/run/pid" ]] && echo present || echo absent)"

echo 0 > "$TREE/run/baselines"
start 9.9.9-beta.1 --help
expect_status "the check does not stand in the way of --help" 0

finish_tests release-start-baselines
