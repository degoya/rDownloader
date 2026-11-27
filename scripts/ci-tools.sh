#!/usr/bin/env bash
#
# The pinned downloads of ci.yml's `scripts`, `supply-chain` and `clamav-live` jobs, each checked
# against its SHA-256 before it runs (RD-1101-07 moved the steps out of the workflow as they were).
# The linters', 7-Zip's and unrar's versions and checksums are scripts/lib/tool-versions.sh's (RD-1280-12),
# gitleaks' stay in the workflow's step environment. CI only, Linux x86_64.
#
#   scripts/ci-tools.sh linters    # shellcheck and actionlint into $RUNNER_TEMP/lint-bin, on PATH
#   scripts/ci-tools.sh gitleaks   # gitleaks over the checkout, findings fail
#   scripts/ci-tools.sh seven-zip  # 7-Zip as $RUNNER_TEMP/seven-zip/7zz, for RD_TEST_7Z
#   scripts/ci-tools.sh unrar      # unrar as $RUNNER_TEMP/unrar/unrar, for RD_TEST_UNRAR
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

# For the RAR test of rd-postprocess (RD-1490-05): Debian 13's `unrar` (non-free), the program a
# Debian user installs. Ubuntu's runner carries none, and the package is not installed: its one
# binary is unpacked into the runner's temporary directory, so nothing reaches the image, a package
# or the SBOM. The pool drops a version a point release replaces; the snapshot archive keeps it.
unrar() {
    local directory="${RUNNER_TEMP}/unrar" archive="${RUNNER_TEMP}/unrar.deb"
    local file="pool/non-free/u/unrar-nonfree/unrar_${RD_UNRAR_DEBIAN_VERSION}_amd64.deb"
    mkdir -p "${directory}"
    curl -fsSL --retry 3 -o "${archive}" "https://deb.debian.org/debian/${file}" ||
        curl -fsSL --retry 3 -o "${archive}" \
            "https://snapshot.debian.org/archive/debian/${RD_UNRAR_DEBIAN_SNAPSHOT}/${file}"
    echo "${RD_UNRAR_SHA256}  ${archive}" | sha256sum --check --strict
    dpkg-deb --fsys-tarfile "${archive}" | tar -xO ./usr/bin/unrar-nonfree > "${directory}/unrar"
    chmod +x "${directory}/unrar"
    "${directory}/unrar" | sed -n 2p
}

case "${1:-}" in
    linters) linters ;;
    gitleaks) gitleaks ;;
    seven-zip) seven_zip ;;
    unrar) unrar ;;
    *) echo "usage: scripts/ci-tools.sh linters|gitleaks|seven-zip|unrar" >&2; exit 2 ;;
esac
