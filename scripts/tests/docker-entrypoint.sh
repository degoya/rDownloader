#!/usr/bin/env bash
#
# docker/entrypoint.sh with stub binaries (audit K6): started as root it adopts PUID/PGID and
# drops to them with gosu; a PUID or PGID of 0 stops it unless RDOWNLOADER_ALLOW_ROOT=1 says so,
# and one that is not a number stops it either way, before an id is changed. Started as another
# user it runs the service directly and reads neither.
#
# Pure shell: it runs in well under a second.
#
#   scripts/tests/docker-entrypoint.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENTRYPOINT="$ROOT/docker/entrypoint.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

STUBS="$SCRATCH/bin"
CALLS="$SCRATCH/calls"
mkdir -p "$STUBS"
# `id -u` is the caller's uid (STUB_UID, root by default); the image's user is 10001:10001.
cat > "$STUBS/id" <<'EOF'
#!/bin/sh
case "$*" in
    -u) echo "${STUB_UID:-0}" ;;
    *) echo 10001 ;;
esac
EOF
for tool in groupmod usermod chown gosu rdownloader; do
    printf '#!/bin/sh\necho "%s $*" >> "%s"\n' "$tool" "$CALLS" > "$STUBS/$tool"
done
chmod +x "$STUBS"/*

# Runs the entrypoint with the environment assignments given, the stubs first on PATH.
start() {
    : > "$CALLS"
    run_status env -i PATH="$STUBS:/usr/bin:/bin" "$@" sh "$ENTRYPOINT" serve
}
calls() { cat "$CALLS"; }

start
expect_status "the image's defaults start" 0
expect "and drop to 10001:10001" "chown -R 10001:10001 /config
chown 10001:10001 /downloads
gosu 10001:10001 rdownloader serve" "$(calls)"

start PUID=1000 PGID=100
expect_status "the host's ids start" 0
expect "and are adopted" "groupmod -o -g 100 rdownloader
usermod -o -u 1000 rdownloader
chown -R 1000:100 /config
chown 1000:100 /downloads
gosu 1000:100 rdownloader serve" "$(calls)"

start PUID=0
expect_status "PUID=0 without the opt-in: refused" 1
expect_output "and said why" "PUID=0 would run the service as root; set RDOWNLOADER_ALLOW_ROOT=1"
expect "nothing changed, nothing started" "" "$(calls)"

start PGID=0
expect_status "PGID=0 without the opt-in: refused" 1
expect "nothing changed, nothing started" "" "$(calls)"

start PUID=0 PGID=0 RDOWNLOADER_ALLOW_ROOT=1
expect_status "root, said out loud: starts" 0
expect_true "as root" 'grep -qx "gosu 0:0 rdownloader serve" "$CALLS"'

for value in abc -5 1000x " 1000"; do
    start PUID="$value" RDOWNLOADER_ALLOW_ROOT=1
    expect_status "PUID='$value': refused, opt-in or not" 1
    expect_output "and named" "PUID=$value is not a numeric id"
    expect "nothing changed, nothing started" "" "$(calls)"
done

start STUB_UID=1000 PUID=0
expect_status "already unprivileged: starts" 0
expect "the service directly, PUID unread" "rdownloader serve" "$(calls)"

finish_tests docker-entrypoint
