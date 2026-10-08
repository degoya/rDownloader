#!/usr/bin/env bash
#
# scripts/docker-tools.sh's check (RD-191-09 T17), which reads files only: requirements.txt has to
# be the compile of requirements.in — the header --lock writes, every tool at its version, every
# package with its hashes. On scratch files; docker/ itself is check.sh's file check (PIPE-05,
# scripts/lib/preflight.sh), which runs whatever the change touched. --lock and --bump need uv and
# the network and are not run here.
#
# Pure bash. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/docker-tools.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRIPT="$ROOT/scripts/docker-tools.sh"

printf '# the tools\nyt-dlp==2026.8.19\n\nstreamlink==8.5.0\n' > "$SCRATCH/requirements.in"
write_txt() {
    {
        head -n 2 "$ROOT/docker/requirements.txt"
        printf 'isodate==0.7.2 \\\n    --hash=sha256:aa \\\n    --hash=sha256:bb\n    # via streamlink\n'
        printf 'streamlink==8.5.0 \\\n    --hash=sha256:cc\n    # via -r docker/requirements.in\n'
        printf 'yt-dlp==2026.8.19 \\\n    --hash=sha256:dd\n    # via -r docker/requirements.in\n'
    } > "$SCRATCH/requirements.txt"
}
check() { run_status env DOCKER_TOOLS_DIR="$SCRATCH" "$SCRIPT"; }

write_txt
check
expect_status "a compile that matches passes" 0
expect_output "and counts" "2 tools, 3 packages, every one hashed"

sed -i 's/^yt-dlp==2026.8.19/yt-dlp==2026.9.1/' "$SCRATCH/requirements.txt"
check
expect_status "a tool at another version fails" 1
expect_output "naming it" "yt-dlp==2026.8.19 is not in requirements.txt"

write_txt
sed -i '/--hash=sha256:cc/d' "$SCRATCH/requirements.txt"
check
expect_status "a package without a hash fails" 1
expect_output "naming it" "no hash for streamlink==8.5.0"

write_txt
sed -i '1d' "$SCRATCH/requirements.txt"
check
expect_status "a hand-written file without the header fails" 1

write_txt
printf 'gallery-dl\n' >> "$SCRATCH/requirements.in"
check
expect_status "an unpinned tool fails" 1
expect_output "naming it" "not name==version: gallery-dl"

rm "$SCRATCH/requirements.txt"
check
expect_status "no compile at all fails" 1

finish_tests "docker-tools"
