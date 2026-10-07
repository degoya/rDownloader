#!/usr/bin/env bash
#
# The published apt and dnf repositories (RD-180-10, RD-1120-07) installed from as their README
# says, as root in a fresh container of channels.yml's `repository` job: the key and the
# `.sources` file (apt) or the `.repo` file (dnf) from <base-url>, never `trusted=yes`, then
# `apt install` / `dnf install`. The newest version the repository carries must be <expect> when
# given, within ten minutes of Pages publishing it. With <from> (a release tag) that version is installed first — the repository keeps the
# last two — started as a user until /api/v1/health answers, given marker files, and upgraded by
# the package manager; then the newest is started again, the markers must still be there, and
# removing the package must leave the database. packages-repo.yml tests the same files against a
# fixture repository; this is the one run against what users actually install from.
#
#   scripts/ci-published-repo.sh apt|dnf <base-url> [<expect X.Y.Z>] [<from vX.Y.Z>]
#
# CI only: it changes the system it runs on. RD_PUBLISHED_REPO_FS prefixes every system path, for
# scripts/tests/ci-published-repo.sh.
set -euo pipefail

usage() { echo "usage: scripts/ci-published-repo.sh apt|dnf <base-url> [<expect X.Y.Z>] [<from vX.Y.Z>]" >&2; exit 2; }
[[ $# -ge 2 && $# -le 4 ]] || usage
manager="$1" base="$2" expect="${3:-}" from="${4:-}"
from="${from#v}"
fs="${RD_PUBLISHED_REPO_FS:-}"
home="${fs}/home/tester/.local/share/rdownloader"
db="${home}/data/rdownloader.sqlite3"

case "$manager" in
    apt)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq curl ca-certificates > /dev/null
        install -d -m 0755 "${fs}/etc/apt/keyrings"
        curl -fsSL "${base}/rdownloader.asc" -o "${fs}/etc/apt/keyrings/rdownloader.asc"
        curl -fsSL "${base}/rdownloader.sources" -o "${fs}/etc/apt/sources.list.d/rdownloader.sources"
        if grep -qi 'trusted=' "${fs}/etc/apt/sources.list.d/rdownloader.sources"; then
            echo "::error::the published rdownloader.sources trusts the repository without its signature"
            exit 1
        fi
        offered() { apt-get update -qq && apt-cache madison rdownloader | awk -F '|' '{ gsub(/ /, "", $2); print $2 }'; }
        install_version() { apt-get install -y --allow-downgrades "rdownloader${1:+=$1}"; }
        upgrade() { apt-get install -y --only-upgrade rdownloader; }
        remove() { apt-get purge -y rdownloader; }
        ;;
    dnf)
        # runuser is util-linux's, which the container image lacks.
        dnf install -y -q util-linux
        curl -fsSL "${base}/rdownloader.repo" -o "${fs}/etc/yum.repos.d/rdownloader.repo"
        # -y: the first look imports the repository's key from the .repo file's gpgkey= for
        # repo_gpgcheck; without it dnf asks, CI answers no and the metadata fails its signature
        # check ("Signing key not found", RD-1130-05). A user answers the question once.
        offered() { dnf -y repoquery --refresh --quiet --queryformat '%{version}\n' rdownloader; }
        install_version() { dnf install -y "rdownloader${1:+-$1}"; }
        upgrade() { dnf upgrade -y rdownloader; }
        remove() { dnf remove -y rdownloader; }
        ;;
    *) usage ;;
esac

# GitHub Pages publishes the release's push to the repository a few minutes after it, and this
# job starts when the Release run ends: the expected version is waited for up to
# RD_PUBLISHED_REPO_WAIT seconds (600), asked every RD_PUBLISHED_REPO_POLL (30).
deadline=$(( SECONDS + ${RD_PUBLISHED_REPO_WAIT:-600} ))
while :; do
    versions="$(offered | sed '/^$/d' | sort -V -u)"
    current="$(tail -n 1 <<< "${versions}")"
    echo "the repository carries rdownloader $(paste -sd' ' - <<< "${versions}")"
    [[ -z "${expect}" || "${current}" == "${expect}" ]] && break
    if (( SECONDS >= deadline )); then
        echo "::error::the repository carries ${current:-nothing}, the release was ${expect}: the release did not move it"
        exit 1
    fi
    sleep "${RD_PUBLISHED_REPO_POLL:-30}"
done
[[ -n "${current}" ]] || { echo "::error::the repository at ${base} offers no rdownloader"; exit 1; }

useradd --create-home tester
# The service's log and pid file, made by the tester: one root made with mktemp -d is 0700 and
# refuses the redirections below (RD-1130-05).
run="$(runuser -u tester -- mktemp -d)"
trap 'rm -rf "${run}"' EXIT
# As the user, in the background, until it answers; then stopped. `;` and not `&&` before the
# `&`, so that $! is the service itself (installers.yml says why).
serve() {
    runuser -u tester -- bash -c "cd ~ || exit 1; rdownloader serve > '${run}/serve.log' 2>&1 & echo \$! > '${run}/serve.pid'"
    # --max-time per try: a service that accepts and never answers held curl until the job's
    # timeout, 20 minutes without its log (RD-1130-05, run 37528846470); --retry-max-time caps
    # the whole wait at five minutes.
    curl --fail --silent --show-error --max-time 10 --retry 300 --retry-delay 1 --retry-max-time 300 \
        --retry-connrefused --retry-all-errors \
        http://127.0.0.1:8710/api/v1/health > /dev/null || { cat "${run}/serve.log"; exit 1; }
    stop_service "$(cat "${run}/serve.pid")"
    test -s "${db}"
}

# Stopped means gone or a zombie: PID 1 of GitHub's job containers reaps nobody, so a service that
# had already ended stayed a zombie, `kill -0` kept answering for it, and the loop waited until the
# job was cancelled after 20 minutes (RD-1130-05, release 1.14.0's channels run). A minute, then
# the service's log and a failure.
stop_service() {
    local pid="$1" waited=0 state
    kill "${pid}" 2> /dev/null || true
    while state="$(ps -o stat= -p "${pid}" 2> /dev/null)" && [[ -n "${state}" && "${state}" != Z* ]]; do
        if (( waited >= ${RD_PUBLISHED_REPO_STOP_WAIT:-60} )); then
            echo "::error::the service (pid ${pid}) did not stop within 60 s of SIGTERM"
            cat "${run}/serve.log"
            exit 1
        fi
        sleep 1
        waited=$(( waited + 1 ))
    done
}

upgraded=0
if [[ -n "${from}" ]] && grep -qx -F -- "${from}" <<< "${versions}"; then
    install_version "${from}"
    rdownloader --version | grep -F "${from}"
    serve
    echo "repository ${from}" > "${home}/data/channels-marker"
    upgrade
    upgraded=1
else
    [[ -z "${from}" ]] \
        || echo "::warning::the repository keeps no ${from} any more (only the last two versions); installing ${current} alone"
    install_version ""
fi
rdownloader --version | grep -F "${current}"
serve
if [[ "${upgraded}" -eq 1 ]]; then
    grep -qx "repository ${from}" "${home}/data/channels-marker" \
        || { echo "::error::the upgrade from ${from} lost the data folder's marker"; exit 1; }
fi
remove
test ! -e "${fs}/usr/lib/rdownloader" || { echo "::error::removing the package left /usr/lib/rdownloader"; exit 1; }
test -s "${db}" || { echo "::error::removing the package took the database with it"; exit 1; }
echo "==> rdownloader ${current} from ${base} ($(
    [[ "${upgraded}" -eq 1 ]] && echo "upgraded from ${from}" || echo "fresh install"
)), data kept"
