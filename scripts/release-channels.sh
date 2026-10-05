#!/usr/bin/env bash
#
# The steps of release.yml that follow a published release out of the repository (RD-1101-07
# moved them out of the workflow as they were): the `package-managers` job (Homebrew tap, Scoop
# bucket, winget, AUR; RD-180-06/-07/-08), the gate of `extension-stores` (RD-170-10) and the
# image tags of `container`. scripts/release-repositories.sh is `package-repository`'s. CI only,
# from the checkout root of the tag; every channel moves for the newest plain vX.Y.Z tag only, so
# a re-run of an old tag or a pre-release cannot move one back.
#
#   scripts/release-channels.sh newest           # the newest plain vX.Y.Z tag of origin
#   scripts/release-channels.sh gate             # outputs tap, bucket, winget, aur: true|false
#   scripts/release-channels.sh push             # CHECKOUT, RENDERED, TARGET, README: one commit
#   scripts/release-channels.sh winget           # komac's pull request to microsoft/winget-pkgs
#   scripts/release-channels.sh aur              # rdownloader-bin to the AUR
#   scripts/release-channels.sh stores-gate      # output submit: true|false
#   scripts/release-channels.sh image-tags       # outputs name and tags of the image
#
# Every command reads REF_NAME (the tag); `gate` the four channels' secrets and AUR_ENABLED,
# `winget` KOMAC_VERSION, KOMAC_SHA256 and GH_TOKEN, `aur` AUR_SSH_KEY and AUR_HOST_FINGERPRINT —
# all from the step's environment, never from the command line.
set -euo pipefail

# Only plain vX.Y.Z tags count, and sort -V orders v1.10.0 after v1.9.0.
newest() {
    git ls-remote --tags --refs origin 'refs/tags/v*' \
        | sed 's#.*refs/tags/##' | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true; } \
        | sort -V | tail -n 1
}

# A secret cannot be tested in `if:`, so this says which channels have their key.
gate() {
    local newest channel name secret
    newest="$(newest)"
    if [[ "${REF_NAME}" != "${newest}" ]]; then
        echo "::notice::${REF_NAME} is not the newest release tag (${newest:-none}); no package channel moves"
        printf '%s=false\n' tap bucket winget aur >> "${GITHUB_OUTPUT}"
        exit 0
    fi
    for channel in tap:HOMEBREW_TAP_DEPLOY_KEY bucket:SCOOP_BUCKET_DEPLOY_KEY winget:WINGET_TOKEN aur:AUR_SSH_KEY; do
        name="${channel%%:*}"
        secret="${channel#*:}"
        if [[ -z "${!secret:-}" ]]; then
            echo "::warning::${secret} is not set; the ${name} stays on its last release"
            echo "${name}=false" >> "${GITHUB_OUTPUT}"
        elif [[ "${name}" == aur && "${AUR_ENABLED:-}" != true ]]; then
            # Until the AUR account exists the key alone does not move the AUR package.
            echo "::notice::the repository variable AUR_ENABLED is not true; the AUR package is not pushed"
            echo "aur=false" >> "${GITHUB_OUTPUT}"
        else
            echo "${name}=true" >> "${GITHUB_OUTPUT}"
        fi
    done
}

# The key stays where actions/checkout put it, in that checkout's SSH configuration; nothing here
# reads or prints it.
push() {
    mkdir -p "$(dirname "${CHECKOUT}/${TARGET}")"
    cp "${RENDERED}" "${CHECKOUT}/${TARGET}"
    cp "${README}" "${CHECKOUT}/README.md"
    git -C "${CHECKOUT}" config user.name "github-actions[bot]"
    git -C "${CHECKOUT}" config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    git -C "${CHECKOUT}" add --all
    if git -C "${CHECKOUT}" diff --cached --quiet; then
        echo "${CHECKOUT}: already at ${REF_NAME}"
        exit 0
    fi
    git -C "${CHECKOUT}" commit --quiet --message "rdownloader ${REF_NAME#v}"
    git -C "${CHECKOUT}" push --quiet origin HEAD
    echo "${CHECKOUT}: rdownloader ${REF_NAME#v}"
}

# komac opens the pull request from the fork <account>/winget-pkgs of the token's account, which
# must exist (komac 2.16 creates none); the token reaches komac as GITHUB_TOKEN and gh as
# GH_TOKEN, never on a command line. komac starts its branch in the fork at microsoft/winget-pkgs'
# newest commit, and GitHub refused that ref in 1.8.1 ("Ref cannot be created", the fork ~1500
# commits behind; RD-190-10). So the fork is synced first with GitHub's own merge-upstream, and a
# refused branch — nothing is created then — is tried again after a pause, synced again; any other
# failure is not, so no pull request doubles.
winget() {
    local archive fork attempt
    archive="komac-${KOMAC_VERSION}-x86_64-unknown-linux-gnu.tar.gz"
    curl --fail --silent --show-error --location --retry 3 --output "${RUNNER_TEMP}/${archive}" \
        "https://github.com/russellbanks/Komac/releases/download/v${KOMAC_VERSION}/${archive}"
    echo "${KOMAC_SHA256}  ${RUNNER_TEMP}/${archive}" | sha256sum --check --quiet
    tar --extract --gzip --file "${RUNNER_TEMP}/${archive}" --directory "${RUNNER_TEMP}" komac
    fork="$(gh api user --jq .login)/winget-pkgs"
    for attempt in 1 2 3; do
        if ! gh api --method POST "repos/${fork}/merge-upstream" -f branch=master > /dev/null; then
            echo "::warning::${fork} could not be synced with microsoft/winget-pkgs"
        fi
        if "${RUNNER_TEMP}/komac" submit rendered/winget --yes 2>&1 | tee "${RUNNER_TEMP}/komac.log"; then
            exit 0
        fi
        if [[ "${attempt}" -eq 3 ]] || ! grep -q 'failed to create branch' "${RUNNER_TEMP}/komac.log"; then
            exit 1
        fi
        echo "::warning::GitHub refused komac's branch in ${fork} (attempt ${attempt}); trying again in 60 s"
        sleep 60
    done
}

# The AUR accepts only its own host key; the fingerprint is the one aur.archlinux.org publishes.
# The key file lives in the runner's temporary folder and is never printed.
aur() {
    local ssh_dir
    if ! scripts/package-managers.sh --check-aur rendered; then
        echo "::error::AUR_ENABLED is true, but the PKGBUILD names no AUR maintainer; put the account into packaging/aur/PKGBUILD.in — nothing is pushed to the AUR"
        exit 1
    fi
    ssh_dir="${RUNNER_TEMP}/aur-ssh"
    install -d -m 700 "${ssh_dir}"
    ( umask 077; printf '%s\n' "${AUR_SSH_KEY}" > "${ssh_dir}/key" )
    ssh-keyscan -t ed25519 aur.archlinux.org > "${ssh_dir}/known_hosts" 2> /dev/null
    if [[ "$(ssh-keygen -l -f "${ssh_dir}/known_hosts" | awk '{ print $2 }')" != "${AUR_HOST_FINGERPRINT}" ]]; then
        echo "::error::aur.archlinux.org did not present its published host key"
        exit 1
    fi
    export GIT_SSH_COMMAND="ssh -i ${ssh_dir}/key -o IdentitiesOnly=yes -o UserKnownHostsFile=${ssh_dir}/known_hosts -o StrictHostKeyChecking=yes"
    # A package name that does not exist yet clones as an empty repository; the first push
    # creates it.
    git clone --quiet ssh://aur@aur.archlinux.org/rdownloader-bin.git aur
    cp rendered/aur/PKGBUILD rendered/aur/.SRCINFO rendered/aur/rdownloader.service \
        rendered/aur/rdownloader-capture.service aur/
    git -C aur config user.name "github-actions[bot]"
    git -C aur config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    git -C aur add --all
    if git -C aur diff --cached --quiet; then
        echo "aur: already at ${REF_NAME}"
        exit 0
    fi
    git -C aur commit --quiet --message "rdownloader-bin ${REF_NAME#v}"
    git -C aur push --quiet origin HEAD:master
    echo "aur: rdownloader-bin ${REF_NAME#v}"
}

stores_gate() {
    local newest
    newest="$(newest)"
    if [[ "${REF_NAME}" != "${newest}" ]]; then
        echo "::notice::${REF_NAME} is not the newest release tag (${newest:-none}); the stores keep their version"
        echo "submit=false" >> "${GITHUB_OUTPUT}"
    else
        echo "submit=true" >> "${GITHUB_OUTPUT}"
    fi
}

# `:latest` only from the newest release tag, so a re-run of an old one cannot move it back.
image_tags() {
    local name newest tags
    name="ghcr.io/${GITHUB_REPOSITORY,,}"
    newest="$(newest)"
    tags="${name}:${REF_NAME}"
    if [[ "${REF_NAME}" == "${newest}" ]]; then
        tags+=$'\n'"${name}:latest"
    else
        echo "::notice::${REF_NAME} is not the newest release tag (${newest:-none}); :latest stays where it is"
    fi
    echo "name=${name}" >> "${GITHUB_OUTPUT}"
    printf 'tags<<EOF\n%s\nEOF\n' "${tags}" >> "${GITHUB_OUTPUT}"
    printf 'pushing:\n%s\n' "${tags}"
}

case "${1:-}" in
    newest) newest ;;
    gate) gate ;;
    push) push ;;
    winget) winget ;;
    aur) aur ;;
    stores-gate) stores_gate ;;
    image-tags) image_tags ;;
    *) echo "usage: scripts/release-channels.sh newest|gate|push|winget|aur|stores-gate|image-tags" >&2; exit 2 ;;
esac
