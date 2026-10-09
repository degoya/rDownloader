#!/usr/bin/env bash
#
# scripts/ci-brew-service-diagnostics.sh against stub `brew`, `curl`, `uname` and service binary
# (RD-1200-02): the service is stopped before its binary starts in the foreground — one still
# running holds the data folder's lock, and the foreground start then only reported that — and
# the script never fails.
#
#   scripts/tests/ci-brew-service-diagnostics.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export CALLS="$SCRATCH/calls"
export PREFIX="$SCRATCH/prefix"
mkdir -p "$SCRATCH/bin" "$PREFIX/opt/rdownloader/libexec" "$PREFIX/var/rdownloader"
stub() {
    printf '#!/usr/bin/env bash\n%s\n' "$2" > "$SCRATCH/bin/$1"
    chmod +x "$SCRATCH/bin/$1"
}
stub brew 'echo "brew $*" >> "$CALLS"; if [[ "$1" == --prefix ]]; then echo "$PREFIX"; fi'
stub curl 'exit 7'
stub uname 'echo Linux'
stub systemctl 'exit 0'
stub journalctl 'exit 0'
stub lsof 'exit 1'
cat > "$PREFIX/opt/rdownloader/libexec/rdownloader" <<'STUB'
#!/usr/bin/env bash
echo "binary $* in $PWD" >> "$CALLS"
echo "keychain_interaction_refused: stub"
exit 1
STUB
chmod +x "$PREFIX/opt/rdownloader/libexec/rdownloader"
export PATH="$SCRATCH/bin:$PATH"

run_status "$ROOT/scripts/ci-brew-service-diagnostics.sh"
expect_status "never fails" 0
expect_true "stops the service" 'grep -qx "brew services stop rdownloader" "$CALLS"'
stop_line="$(grep -nx "brew services stop rdownloader" "$CALLS" | cut -d: -f1 || true)"
serve_line="$(grep -n "^binary serve in " "$CALLS" | cut -d: -f1 || true)"
if [[ -n "$stop_line" && -n "$serve_line" && "$stop_line" -lt "$serve_line" ]]; then
    ok "before the foreground start"
else
    fail "before the foreground start" "$(cat "$CALLS")"
fi
expect_true "which runs in the service's working folder" 'grep -qx "binary serve in $PREFIX/var/rdownloader" "$CALLS"'
expect_output "and shows what the binary said" "keychain_interaction_refused: stub"
expect_output "with its exit status" "foreground exit: 1"

finish_tests ci-brew-service-diagnostics
