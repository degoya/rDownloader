#!/usr/bin/env bash
#
# The install half of .github/workflows/packages-repo.yml (RD-180-10), one package manager per
# call, inside that client's container, as root, with the two fixture repository states of
# `scripts/tests/package-repo.sh --out` in ./states (RD-1280-15: three jobs carried the same
# step pair before, one matrix runs this). CI only; it changes the system it runs on.
#
#   scripts/ci-package-repo.sh apt|dnf|zypper install    # install 1.0.0, upgrade, downgrade
#   scripts/ci-package-repo.sh apt|dnf|zypper refused    # an altered signed index is refused
#
# `install` serves state 1 at file:///repo, adds the key and the .sources or .repo file exactly
# as the README says, installs, upgrades to state 2 and downgrades to the kept older version.
# `refused` shows that a repository whose signed index was altered fails the update with a
# non-zero exit, installs nothing and offers nothing — the signature is checked, nothing says
# trusted=yes, and dnf does not skip the repository with a warning — and that the same state
# unaltered installs again, so the failure is the signature's.
set -euxo pipefail

installed() { cat /usr/lib/rdownloader-fixture/version; }

apt_install() {
    ln -sfn "$PWD/states/1" /repo
    install -d -m 0755 /etc/apt/keyrings
    cp /repo/rdownloader.asc /etc/apt/keyrings/rdownloader.asc
    cp /repo/rdownloader.sources /etc/apt/sources.list.d/rdownloader.sources
    # `!` would not stop the script under set -e, hence the if.
    if grep -i 'trusted=' /etc/apt/sources.list.d/rdownloader.sources; then exit 1; fi
    apt-get update
    apt-get install -y rdownloader
    [[ "$(installed)" == 1.0.0 ]]
    ln -sfn "$PWD/states/2" /repo
    apt-get update
    apt-get upgrade -y
    [[ "$(installed)" == 1.2.0 ]]
    apt-get install -y --allow-downgrades rdownloader=1.1.0-1
    [[ "$(installed)" == 1.1.0 ]]
}

apt_refused() {
    cp -a states/2 tampered
    sed -i 's/^Label: rDownloader$/Label: someone else/' \
        tampered/deb/dists/stable/InRelease tampered/deb/dists/stable/Release
    ln -sfn "$PWD/tampered" /repo
    apt-get purge -y rdownloader
    rm -rf /var/lib/apt/lists/*
    if apt-get update; then
        echo "::error::apt-get update succeeds on a repository whose signed index was altered"
        exit 1
    fi
    if apt-cache show rdownloader; then
        echo "::error::apt offers rdownloader from a repository whose signed index was altered"
        exit 1
    fi
    if apt-get install -y rdownloader; then
        echo "::error::apt installs rdownloader from a repository whose signed index was altered"
        exit 1
    fi
    ln -sfn "$PWD/states/2" /repo
    apt-get update
    apt-get install -y rdownloader
    [[ "$(installed)" == 1.2.0 ]]
}

dnf_install() {
    ln -sfn "$PWD/states/1" /repo
    cp /repo/rdownloader.repo /etc/yum.repos.d/rdownloader.repo
    # gpgcheck=1 makes dnf refuse an unsigned rpm, so the install proves the package signature.
    grep -x 'gpgcheck=1' /etc/yum.repos.d/rdownloader.repo
    grep -x 'repo_gpgcheck=1' /etc/yum.repos.d/rdownloader.repo
    # Fedora's dnf5 sets skip_if_unavailable=True in /usr/share/dnf5/libdnf.conf.d/20-fedora-defaults.conf;
    # the repository's own skip_if_unavailable=0 turns a failed signature check into a hard error.
    grep -x 'skip_if_unavailable=0' /etc/yum.repos.d/rdownloader.repo
    dnf install -y rdownloader
    [[ "$(installed)" == 1.0.0 ]]
    ln -sfn "$PWD/states/2" /repo
    dnf upgrade -y --refresh rdownloader
    [[ "$(installed)" == 1.2.0 ]]
    dnf downgrade -y rdownloader
    [[ "$(installed)" == 1.1.0 ]]
}

dnf_refused() {
    cp -a states/2 tampered
    sed -i 's#<revision>#<revision>1#' tampered/rpm/repodata/repomd.xml
    ln -sfn "$PWD/tampered" /repo
    dnf remove -y rdownloader
    dnf clean all
    if dnf makecache --refresh --repo rdownloader; then
        echo "::error::dnf accepts a repomd.xml that no longer matches its signature"
        exit 1
    fi
    if dnf install -y --refresh rdownloader; then
        echo "::error::dnf installs rdownloader from a repository whose repomd.xml was altered"
        exit 1
    fi
    # With skipping switched back on dnf ends with 0 (run 36781008221), but the altered
    # metadata are still never used: the repository is dropped and nothing is offered.
    if dnf install -y --refresh --setopt=rdownloader.skip_if_unavailable=1 rdownloader; then
        echo "::error::dnf installs rdownloader from an altered repomd.xml once skipping is allowed"
        exit 1
    fi
    if rpm -q rdownloader; then exit 1; fi
    ln -sfn "$PWD/states/2" /repo
    dnf install -y --refresh rdownloader
    [[ "$(installed)" == 1.2.0 ]]
}

# zypper reads the same rdownloader.repo as dnf, keys and all: gpgcheck=1 and repo_gpgcheck=1
# are libzypp's too, skip_if_unavailable is not (zypper never skips a failed repository
# silently). The key is imported with rpm, as `zypper addrepo` would otherwise ask to trust it
# interactively; a failed signature check aborts in --non-interactive mode, it is not accepted.
zypper_install() {
    # The minimal image need not carry them; asked for before the fixture repository exists.
    zypper --non-interactive install --no-recommends grep sed
    ln -sfn "$PWD/states/1" /repo
    grep -x 'gpgcheck=1' /repo/rdownloader.repo
    grep -x 'repo_gpgcheck=1' /repo/rdownloader.repo
    rpm --import /repo/rdownloader.asc
    # As the README says: the .repo file by its URL.
    zypper --non-interactive addrepo file:///repo/rdownloader.repo
    zypper --non-interactive refresh rdownloader
    zypper --non-interactive install rdownloader
    [[ "$(installed)" == 1.0.0 ]]
    ln -sfn "$PWD/states/2" /repo
    zypper --non-interactive refresh rdownloader
    zypper --non-interactive update rdownloader
    [[ "$(installed)" == 1.2.0 ]]
    zypper --non-interactive install --oldpackage rdownloader=1.1.0-1
    [[ "$(installed)" == 1.1.0 ]]
}

zypper_refused() {
    cp -a states/2 tampered
    sed -i 's#<revision>#<revision>1#' tampered/rpm/repodata/repomd.xml
    ln -sfn "$PWD/tampered" /repo
    zypper --non-interactive remove rdownloader
    zypper --non-interactive clean --all
    if zypper --non-interactive refresh --force rdownloader; then
        echo "::error::zypper accepts a repomd.xml that no longer matches its signature"
        exit 1
    fi
    if zypper --non-interactive install rdownloader; then
        echo "::error::zypper installs rdownloader from a repository whose repomd.xml was altered"
        exit 1
    fi
    if rpm -q rdownloader; then exit 1; fi
    ln -sfn "$PWD/states/2" /repo
    zypper --non-interactive refresh --force rdownloader
    zypper --non-interactive install rdownloader
    [[ "$(installed)" == 1.2.0 ]]
}

case "${1:-} ${2:-}" in
    "apt install" | "apt refused" | "dnf install" | "dnf refused" | "zypper install" | "zypper refused")
        "${1}_${2}" ;;
    *)
        set +x
        echo "usage: scripts/ci-package-repo.sh apt|dnf|zypper install|refused" >&2
        exit 2
        ;;
esac
