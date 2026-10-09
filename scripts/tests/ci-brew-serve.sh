#!/usr/bin/env bash
#
# scripts/ci-brew-serve.sh against stub `brew`, `curl`, `uname`, `sleep` and service binary
# (RD-1230-01): a service that answers ends 0; `keychain_interaction_refused` ends 3 with a
# notice only on macOS after an upgrade; every other end -- the refusal on Linux or without an
# upgrade, a timeout, another error -- ends 1 after the diagnostics.
#
#   scripts/tests/ci-brew-serve.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export CALLS="$SCRATCH/calls" FAKE="$SCRATCH/fake"
export PREFIX="$SCRATCH/prefix"
mkdir -p "$SCRATCH/bin" "$FAKE" "$PREFIX/opt/rdownloader/libexec" "$PREFIX/var/rdownloader"
stub() {
    printf '#!/usr/bin/env bash\n%s\n' "$2" > "$SCRATCH/bin/$1"
    chmod +x "$SCRATCH/bin/$1"
}
stub brew 'echo "brew $*" >> "$CALLS"; if [[ "$1" == --prefix ]]; then echo "$PREFIX"; fi'
# curl answers once $FAKE/healthy exists; uname says $FAKE/os.
stub curl 'echo "curl $*" >> "$CALLS"; [[ -f "$FAKE/healthy" ]] || exit 7; echo "{\"status\":\"ok\"}"'
stub uname 'cat "$FAKE/os"'
stub sleep 'exit 0'
for probe in systemctl journalctl launchctl plutil codesign xattr log; do stub "$probe" 'exit 0'; done
stub lsof 'exit 1'
cat > "$PREFIX/opt/rdownloader/libexec/rdownloader" <<'STUB'
#!/usr/bin/env bash
echo "binary $*" >> "$CALLS"
exit 1
STUB
chmod +x "$PREFIX/opt/rdownloader/libexec/rdownloader"
export PATH="$SCRATCH/bin:$PATH"
LOG="$SCRATCH/rdownloader.log"

# [HEALTHY=yes] serve <os> <the log's line, or "-" for no log> [--upgraded]
serve() {
    echo "$1" > "$FAKE/os"
    rm -f "$FAKE/healthy" "$CALLS" "$LOG"
    if [[ "$2" != - ]]; then printf '%s\n' "$2" > "$LOG"; fi
    if [[ "${HEALTHY:-}" == yes ]]; then touch "$FAKE/healthy"; fi
    shift 2
    run_status "$ROOT/scripts/ci-brew-serve.sh" "$LOG" "$@"
}
# The service's own health checks, not the diagnostics' foreground one.
health_checks() { grep -c "^curl --fail --silent http" "$CALLS" || true; }
diagnosed() { grep -q "^binary serve" "$CALLS"; }
stopped() { grep -qx "brew services stop rdownloader" "$CALLS"; }
refusal="keychain_interaction_refused: the macOS keychain holds the vault master key"

# --- it answers ---------------------------------------------------------------------------------
HEALTHY=yes serve Darwin "" --upgraded
expect_status "a service that answers" 0
expect_true "was started" 'grep -qx "brew services start rdownloader" "$CALLS"'
expect_true "and is stopped again" stopped
expect_true "without diagnostics" '! diagnosed'

# --- the expected refusal (RD-1230-01) ----------------------------------------------------------
serve Darwin "$refusal" --upgraded
expect_status "macOS after an upgrade, stopped at the keychain" 3
expect_output "says so in a notice" "::notice::the upgraded service stopped at the keychain"
expect_output "naming RD-200-01" "RD-200-01"
expect_output "with the service's own line" "$refusal"
expect_true "stops the service launchd keeps restarting" stopped
expect_true "without diagnostics" '! diagnosed'
expect_true "at the first look at the log" '[[ "$(health_checks)" == 1 ]]'

# --- every other end stays red ------------------------------------------------------------------
serve Darwin "$refusal"
expect_status "the refusal without an upgrade" 1
expect_output "is an error" "::error::the service stopped at the keychain"
expect_true "with the diagnostics" diagnosed
serve Linux "$refusal" --upgraded
expect_status "the refusal on Linux" 1
expect_true "with the diagnostics" diagnosed
serve Darwin "Platform failure: The user name or passphrase you entered is not correct." --upgraded
expect_status "another error after an upgrade" 1
expect_output "is a service that did not answer" "::error::the service did not answer /api/v1/health within 60 s"
expect_true "after 60 tries" '[[ "$(health_checks)" == 60 ]]'
expect_true "with the diagnostics" diagnosed
serve Darwin - --upgraded
expect_status "no log at all" 1
run_status "$ROOT/scripts/ci-brew-serve.sh"
expect_status "no log named" 1
expect_output "is a usage error" "usage: ci-brew-serve.sh"

finish_tests ci-brew-serve
