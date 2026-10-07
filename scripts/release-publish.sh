#!/usr/bin/env bash
#
# Three steps of release.yml's `publish` job (RD-1101-07 moved them out of the workflow as they
# were). CI only, from the checkout root, after release-assets/SHA256SUMS exists.
#
#   scripts/release-publish.sh update-manifest   # signed rdownloader-update-<channel>.json
#   scripts/release-publish.sh plugin-release    # name output and body of the plugin release
#   scripts/release-publish.sh app-release       # body of the application release
#
# All read REF_NAME (the tag) and GITHUB_REPOSITORY; `update-manifest` reads
# RDOWNLOADER_UPDATE_SIGNING_KEY and the packager at $RUNNER_TEMP/rd-pack, `plugin-release`
# writes $RUNNER_TEMP/plugin-release.md and the output `name`, `app-release`
# $RUNNER_TEMP/app-release.md.
set -euo pipefail

# RD-180-01: the signed update manifest of this release, built from the SHA256SUMS, the files
# beside it, the version's points for users in RELEASE-NOTES.md and the anchor of its CHANGELOG
# heading (RD-1150-02): rdownloader-update-stable.json on a plain vX.Y.Z
# tag, rdownloader-update-beta.json on a vX.Y.Z-beta.N pre-release. `verify` checks it against
# the root this build embeds, which also catches a secret that is not the update key. It is signed
# itself and therefore not in SHA256SUMS. Without the secret the release carries no manifest, and
# installations report `update.not_published`.
update_manifest() {
    local schema_change manifest
    if [[ -z "${RDOWNLOADER_UPDATE_SIGNING_KEY}" ]]; then
        echo "::warning::RDOWNLOADER_UPDATE_SIGNING_KEY is not set; this release publishes no update manifest"
        exit 0
    fi
    # RD-180-02: whether crates/rd-db/migrations/ changed since the release before this one on
    # its channel; `true` whenever that cannot be told.
    schema_change="$(scripts/update-schema-change.sh "${REF_NAME}")"
    "${RUNNER_TEMP}/rd-pack" update manifest build \
        --version "${REF_NAME}" \
        --schema-change "${schema_change}" \
        --checksums release-assets/SHA256SUMS \
        --assets release-assets \
        --base-url "https://github.com/${GITHUB_REPOSITORY}/releases/download/${REF_NAME}/" \
        --changelog CHANGELOG.md \
        --release-notes RELEASE-NOTES.md \
        --out release-assets
    for manifest in release-assets/rdownloader-update-*.json; do
        "${RUNNER_TEMP}/rd-pack" update manifest verify "${manifest}" --assets release-assets
    done
}

plugin_release() {
    echo "name=rDownloader plugins ${REF_NAME#v}" >> "${GITHUB_OUTPUT}"
    printf '%s\n' "The signed plugin packages of rDownloader ${REF_NAME#v}. The application, its installers and the plugin index are in the release [${REF_NAME}](https://github.com/${GITHUB_REPOSITORY}/releases/tag/${REF_NAME}); a running rDownloader installs and updates these plugins through that index (Settings > Plugins)." \
        > "${RUNNER_TEMP}/plugin-release.md"
}

# RD-1150-02: the application release's description starts with the version's points for users
# (scripts/release-notes.sh), the same the update dialog shows; under them the developers' link,
# the version's CHANGELOG section at the tag. GitHub appends its generated notes below
# (`generate_release_notes`).
app_release() {
    local version="${REF_NAME#v}" anchor changelog notes
    anchor="$(scripts/release-notes.sh --anchor "${version}")"
    changelog="https://github.com/${GITHUB_REPOSITORY}/blob/${REF_NAME}/CHANGELOG.md${anchor:+#${anchor}}"
    notes="$(scripts/release-notes.sh "${version}")"
    {
        if [[ -n "${notes}" ]]; then
            printf "## What's new\n\n%s\n\n" "${notes}"
        fi
        echo "**For developers:** every change of ${version} is in [CHANGELOG.md](${changelog})."
    } > "${RUNNER_TEMP}/app-release.md"
}

case "${1:-}" in
    update-manifest) update_manifest ;;
    plugin-release) plugin_release ;;
    app-release) app_release ;;
    *) echo "usage: scripts/release-publish.sh update-manifest|plugin-release|app-release" >&2; exit 2 ;;
esac
