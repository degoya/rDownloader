#!/usr/bin/env bash
#
# Prices a MEGA account sign-in inside the WebAssembly sandbox (RD-120-11).
#
# `docs/roadmap/jobs/120-11-mega.md` moved MEGA out of milestone 1.1 on exactly one unmeasured
# number: what RSA costs a guest against the plugin host's fuel budget. Section 7 of that job
# reasoned its way to "roughly 200 Wasm instructions per AES block" and said plainly that the
# figure was arithmetic. This script replaces it with a measurement.
#
# It builds `plugins/mega-login-probe` — a core module, not a plugin: no manifest, never
# packaged, never signed — and runs it under the same Wasmtime configuration `SandboxEngine`
# gives a real plugin, reading the fuel counter around each stage of the sign-in.
#
# Not run by CI and not a gate. A measurement is re-taken when somebody wants to know, and the
# numbers it produced last are written down in the job file beside the date they were taken.
#
# Usage:
#   scripts/measure-mega-login-fuel.sh [JOBS]
#
set -euo pipefail

cd "$(dirname "$0")/.."

JOBS="${1:-${JOBS:-}}"
# shellcheck source=lib/jobs.sh
source scripts/lib/jobs.sh
TARGET="wasm32-unknown-unknown"

echo "==> building the probe for $TARGET (jobs: $JOBS)"
cargo build --release -j "$JOBS" --target "$TARGET" -p mega-login-probe

echo "==> measuring"
cargo run -j "$JOBS" -p rd-plugin-host --example mega_login_fuel
