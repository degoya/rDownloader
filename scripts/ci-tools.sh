#!/usr/bin/env bash
#
# The pinned downloads of ci.yml's `scripts`, `supply-chain` and `clamav-live` jobs, each checked
# against its SHA-256 before it runs (RD-1101-07 moved the steps out of the workflow as they were).
# The linters' and 7-Zip's versions and checksums are scripts/lib/tool-versions.sh's (RD-1280-12),
# gitleaks' stay in the workflow's step environment. CI only, Linux x86_64.
#
#   scripts/ci-tools.sh linters    # shellcheck and actionlint into $RUNNER_TEMP/lint-bin, on PATH
#   scripts/ci-tools.sh gitleaks   # gitleaks over the checkout, findings fail
#   scripts/ci-tools.sh seven-zip  # 7-Zip as $RUNNER_TEMP/seven-zip/7zz, for RD_TEST_7Z
#
# `linters` appends to GITHUB_PATH; `gitleaks` reads GITLEAKS_VERSION and GITLEAKS_SHA256.
set -euo pipefail

# shellcheck source=lib/tool-versions.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/tool-versions.sh"

linters() {
    local bin="${RUNNER_TEMP}/lint-bin" archive
    mkdir -p "${bin}"
    archive="${RUNNER_TEMP}/shellcheck.tar.xz"
    curl -fsSL -o "${archive}" \
        "https://github.com/koalaman/shellcheck/releases/download/v${RD_SHELLCHECK_VERSION}/shellcheck-v${RD_SHELLCHECK_VERSION}.linux.x86_64.tar.xz"
    echo "${RD_SHELLCHECK_SHA256}  ${archive}" | sha256sum --check --strict
    tar -xJf "${archive}" -C "${bin}" --strip-components=1 "shellcheck-v${RD_SHELLCHECK_VERSION}/shellcheck"
    archive="${RUNNER_TEMP}/actionlint.tar.gz"
    curl -fsSL -o "${archive}" \
        "https://github.com/rhysd/actionlint/releases/download/v${RD_ACTIONLINT_VERSION}/actionlint_${RD_ACTIONLINT_VERSION}_linux_amd64.tar.gz"
    echo "${RD_ACTIONLINT_SHA256}  ${archive}" | sha256sum --check --strict
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

# For the archive test of rd-postprocess (RD-1480-14): Ubuntu 24.04's own 7zip is 23.01, below the
# service's floor of 25.00. The same release as the container image's (Debian 13).
seven_zip() {
    local directory="${RUNNER_TEMP}/seven-zip" archive="${RUNNER_TEMP}/seven-zip.tar.xz"
    mkdir -p "${directory}"
    curl -fsSL --retry 3 -o "${archive}" \
        "https://www.7-zip.org/a/7z${RD_SEVEN_ZIP_VERSION//./}-linux-x64.tar.xz"
    echo "${RD_SEVEN_ZIP_SHA256}  ${archive}" | sha256sum --check --strict
    tar -xJf "${archive}" -C "${directory}" 7zz
}

case "${1:-}" in
    linters) linters ;;
    gitleaks) gitleaks ;;
    seven-zip) seven_zip ;;
    *) echo "usage: scripts/ci-tools.sh linters|gitleaks|seven-zip" >&2; exit 2 ;;
esac
