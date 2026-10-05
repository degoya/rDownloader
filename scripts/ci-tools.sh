#!/usr/bin/env bash
#
# The pinned downloads of ci.yml's `scripts` and `supply-chain` jobs, each checked against its
# published SHA-256 before it runs (RD-1101-07 moved the steps out of the workflow as they were).
# The versions and checksums stay in the workflow's step environment. CI only, Linux x86_64.
#
#   scripts/ci-tools.sh linters    # shellcheck and actionlint into $RUNNER_TEMP/lint-bin, on PATH
#   scripts/ci-tools.sh gitleaks   # gitleaks over the checkout, findings fail
#
# `linters` reads SHELLCHECK_VERSION, SHELLCHECK_SHA256, ACTIONLINT_VERSION, ACTIONLINT_SHA256 and
# appends to GITHUB_PATH; `gitleaks` reads GITLEAKS_VERSION and GITLEAKS_SHA256.
set -euo pipefail

linters() {
    local bin="${RUNNER_TEMP}/lint-bin" archive
    mkdir -p "${bin}"
    archive="${RUNNER_TEMP}/shellcheck.tar.xz"
    curl -fsSL -o "${archive}" \
        "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VERSION}/shellcheck-v${SHELLCHECK_VERSION}.linux.x86_64.tar.xz"
    echo "${SHELLCHECK_SHA256}  ${archive}" | sha256sum --check --strict
    tar -xJf "${archive}" -C "${bin}" --strip-components=1 "shellcheck-v${SHELLCHECK_VERSION}/shellcheck"
    archive="${RUNNER_TEMP}/actionlint.tar.gz"
    curl -fsSL -o "${archive}" \
        "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"
    echo "${ACTIONLINT_SHA256}  ${archive}" | sha256sum --check --strict
    tar -xzf "${archive}" -C "${bin}" actionlint
    echo "${bin}" >> "${GITHUB_PATH}"
}

# The tree is public, so a secret in it is published. The known fixture findings are allowlisted
# in .gitleaks.toml (RD-130-23), which gitleaks reads from the scanned root.
gitleaks() {
    local archive="gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz"
    curl -fsSL -o "$RUNNER_TEMP/$archive" \
        "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/$archive"
    echo "$GITLEAKS_SHA256  $RUNNER_TEMP/$archive" | sha256sum --check --strict
    tar -xzf "$RUNNER_TEMP/$archive" -C "$RUNNER_TEMP" gitleaks
    "$RUNNER_TEMP/gitleaks" dir . --no-banner --redact --exit-code 1
}

case "${1:-}" in
    linters) linters ;;
    gitleaks) gitleaks ;;
    *) echo "usage: scripts/ci-tools.sh linters|gitleaks" >&2; exit 2 ;;
esac
