#!/usr/bin/env bash
#
# The steps of release.yml's `packages` job (RD-1101-07 moved them out of the workflow as they
# were): the signed plugins into every archive the binary jobs made, the layout check, and the deb
# and rpm packages of both Linux architectures. CI only, on Linux, from the checkout root.
#
#   scripts/release-packages.sh add-plugins    # unpacked/*.unpacked.* + layer/plugins → packages/
#   scripts/release-packages.sh check-layout   # every archive in packages/ against the one layout
#   scripts/release-packages.sh install-nfpm   # nfpm 2.47.0, checksum-pinned, on GITHUB_PATH
#   scripts/release-packages.sh deb-rpm        # installers/ from the two Linux tarballs
set -euo pipefail

# The tar gets the plugins appended and is compressed, the zip gets them added. Nothing is
# extracted and repacked, so every other entry is byte for byte the one the platform's runner
# wrote.
add_plugins() {
    local count=0 archive name
    mkdir -p packages
    for archive in unpacked/*.unpacked.tar unpacked/*.unpacked.zip; do
        [[ -f "${archive}" ]] || continue
        name="$(basename "${archive}")"
        case "${name}" in
            *.tar)
                tar --directory layer --append --file "${archive}" ./plugins
                gzip --no-name --stdout "${archive}" > "packages/${name%.unpacked.tar}.tar.gz"
                ;;
            *.zip)
                cp "${archive}" "packages/${name%.unpacked.zip}.zip"
                (cd layer && zip -q -r "../packages/${name%.unpacked.zip}.zip" plugins)
                ;;
        esac
        count=$((count + 1))
    done
    # One archive per binary target; fewer means a platform is missing from the release.
    [[ ${count} -eq 5 ]] || { echo "::error::expected 5 archives, found ${count}"; exit 1; }
    ls -l packages
}

# The same check scripts/release-pipeline.sh runs on the local packages (RD-180-05): flat, the
# listed files, VERSION.txt, the signed plugins, nothing else.
check_layout() {
    local archive name platform
    # shellcheck source=lib/archive-layout.sh
    source scripts/lib/archive-layout.sh
    for archive in packages/*.tar.gz packages/*.zip; do
        name="$(basename "${archive}")"
        platform="${name#rdownloader-}"
        rd_check_archive_layout "${archive}" "${platform%%-*}"
        echo "ok   ${name}"
    done
}

# nfpm writes both formats for both architectures: it runs nothing it packs. Pinned with its
# published checksum.
install_nfpm() {
    curl --fail --silent --show-error --location --output "${RUNNER_TEMP}/nfpm.tar.gz" \
        https://github.com/goreleaser/nfpm/releases/download/v2.47.0/nfpm_2.47.0_Linux_x86_64.tar.gz
    echo "0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783  ${RUNNER_TEMP}/nfpm.tar.gz" | sha256sum --check
    tar --extract --gzip --file "${RUNNER_TEMP}/nfpm.tar.gz" --directory "${RUNNER_TEMP}" nfpm
    echo "${RUNNER_TEMP}" >> "${GITHUB_PATH}"
}

# RD-180-05: the deb and rpm of each architecture, from the finished tarball.
deb_rpm() {
    local arch
    for arch in x86_64 aarch64; do
        scripts/package-deb-rpm.sh "packages/rdownloader-linux-${arch}.tar.gz" installers
    done
    ls -l installers
}

case "${1:-}" in
    add-plugins) add_plugins ;;
    check-layout) check_layout ;;
    install-nfpm) install_nfpm ;;
    deb-rpm) deb_rpm ;;
    *) echo "usage: scripts/release-packages.sh add-plugins|check-layout|install-nfpm|deb-rpm" >&2; exit 2 ;;
esac
