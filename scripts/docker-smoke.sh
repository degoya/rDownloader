#!/usr/bin/env bash
#
# Starts a built container image on fresh volumes and checks that it serves (RD-140-25).
#
# One definition for the two places that must agree on what "the image works" means: the CI
# docker job, and the release, which runs it on the amd64 image before anything is pushed.
#
#  * Fresh named volumes, as a first start on a user's machine has: the entrypoint must be able
#    to take ownership of both, and the bundled plugins must install into /config.
#  * The bundled Python tools run as the service user: yt-dlp, streamlink, gallery-dl and apprise
#    (RD-130-14, RD-1120-07), each answering `--version`.
#
# Usage:
#   scripts/docker-smoke.sh <image> [port]
set -euo pipefail

image="${1:?usage: scripts/docker-smoke.sh <image> [port]}"
port="${2:-8710}"
suffix="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-$$}"
config_volume="rdownloader-config-${suffix}"
downloads_volume="rdownloader-downloads-${suffix}"

docker volume create "${config_volume}" > /dev/null
docker volume create "${downloads_volume}" > /dev/null
container_id=""
cleanup() {
    if [[ -n "${container_id}" ]]; then
        # What the service said is the only evidence a failed run leaves behind.
        docker logs "${container_id}" 2>&1 | tail -n 80 || true
        docker rm --force "${container_id}" > /dev/null || true
    fi
    docker volume rm "${config_volume}" "${downloads_volume}" > /dev/null || true
}
trap cleanup EXIT

container_id="$(docker run --detach --publish "127.0.0.1:${port}:8710" \
    --volume "${config_volume}:/config" \
    --volume "${downloads_volume}:/downloads" \
    "${image}")"
docker exec "${container_id}" rdownloader --version
# The service loads its plugins before it listens, and until then Docker's port proxy answers
# with a reset — an error curl's plain --retry does not repeat. Retry all errors for up to two
# minutes; the cleanup shows the container's log when it never answers.
if ! curl --fail --silent --show-error --retry 60 --retry-delay 2 --retry-all-errors \
    "http://127.0.0.1:${port}/api/v1/health"; then
    docker ps --all --filter "id=${container_id}"
    exit 1
fi
echo
for tool in yt-dlp streamlink gallery-dl apprise; do
    echo "==> ${tool} as the service user"
    docker exec --user rdownloader "${container_id}" "${tool}" --version
done
echo "==> ${image} passed the smoke test"
