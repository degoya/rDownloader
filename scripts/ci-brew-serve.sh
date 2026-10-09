#!/usr/bin/env bash
#
# channels.yml's `homebrew` job starts the `rdownloader` service with `brew services`, waits up to
# 60 s for /api/v1/health and stops it again (RD-180-06). 0: it answered. A service that did not
# answer gets scripts/ci-brew-service-diagnostics.sh and 1 -- with one exception (RD-1230-01).
#
# On macOS, after `brew upgrade` from an older release (`--upgraded`), the keychain item the old
# ad-hoc signed build created refuses the new one, and the service ends with
# `keychain_interaction_refused` and the way out instead of waiting on a prompt nobody sees
# (RD-1200-02). The job does not allow the new build beforehand (that needs the login keychain's
# password and tests a path no user takes), so until a Developer ID signature lets the item trust
# the next build (RD-200-01) that end is the expected one: a `::notice::`, the service stopped,
# and 3, on which the job skips what needs the upgraded service running. Every other end -- a
# timeout, another error, the refusal on Linux or without an upgrade -- stays 1.
#
#   scripts/ci-brew-serve.sh <service log> [--upgraded]
#
# CI only: it starts the service on the machine it runs on.
set -uo pipefail

log="${1:?usage: ci-brew-serve.sh <service log> [--upgraded]}"
upgraded=false
[[ "${2:-}" == --upgraded ]] && upgraded=true

brew services start rdownloader
refused=false
for _ in $(seq 1 60); do
    if curl --fail --silent http://127.0.0.1:8710/api/v1/health; then
        echo
        brew services stop rdownloader
        exit 0
    fi
    if grep -qF keychain_interaction_refused "${log}" 2> /dev/null; then
        refused=true
        break
    fi
    sleep 1
done

if [[ "${refused}" == true && "${upgraded}" == true && "$(uname -s)" == Darwin ]]; then
    echo "::notice::the upgraded service stopped at the keychain (keychain_interaction_refused), as expected for an ad-hoc signed upgrade until RD-200-01"
    grep -F keychain_interaction_refused "${log}" | tail -n 1
    brew services stop rdownloader
    exit 3
fi
if [[ "${refused}" == true ]]; then
    echo "::error::the service stopped at the keychain (keychain_interaction_refused); see its log below"
else
    echo "::error::the service did not answer /api/v1/health within 60 s; see its log below"
fi
"$(dirname "${BASH_SOURCE[0]}")/ci-brew-service-diagnostics.sh"
exit 1
