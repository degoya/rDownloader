#!/usr/bin/env bash
#
# The live services of ci.yml's `s3-live` and `clamav-live` jobs (RD-150-04, RD-190-14): MinIO
# started as a step, MinIO and clamd waited for (RD-1101-07 moved the steps out of the workflow as
# they were). CI only; the test build in between gives both their time.
#
# MinIO is started as a step, not as a `services:` container: the official image needs
# `server /data` as its command, which `services:` cannot pass, and MinIO no longer publishes it
# on Docker Hub (the registry answers UNAUTHORIZED for minio/minio, 2026-09-28). The last Bitnami
# build, frozen as bitnamilegacy/minio and starting the server on its own, is pinned by digest in
# the job's MINIO_IMAGE; the credentials are the throw-away container's own. The clamd image
# starts clamd on 3310 by itself; freshclam updating its database at start is not waited for.
#
#   scripts/ci-services.sh minio-start          # docker run MINIO_IMAGE with the RD_S3_LIVE_* keys
#   scripts/ci-services.sh minio-ready          # /minio/health/live, then the bucket
#   scripts/ci-services.sh clamd-ready <id>     # clamd's own PING; <id> is the service container
#
# Reads MINIO_IMAGE and RD_S3_LIVE_ENDPOINT, _REGION, _BUCKET, _ACCESS_KEY and _SECRET from the
# job's environment.
set -euo pipefail

minio_start() {
    docker run --detach --name minio --publish 127.0.0.1:9000:9000 \
        --env MINIO_ROOT_USER="${RD_S3_LIVE_ACCESS_KEY}" \
        --env MINIO_ROOT_PASSWORD="${RD_S3_LIVE_SECRET}" \
        "${MINIO_IMAGE}"
}

# The build gave MinIO its time; this waits for the rest, polling /minio/health/live from the
# runner, then makes the bucket with a signed CreateBucket (curl's own SigV4), since object_store
# creates none.
minio_ready() {
    local attempt
    for attempt in $(seq 1 60); do
        curl --silent --fail --output /dev/null "${RD_S3_LIVE_ENDPOINT}/minio/health/live" && break
        if [[ ${attempt} -eq 60 ]]; then
            echo "::error::MinIO did not answer /minio/health/live within 60 s"
            docker logs minio
            exit 1
        fi
        sleep 1
    done
    curl --silent --show-error --fail -X PUT --data-binary '' \
        --aws-sigv4 "aws:amz:${RD_S3_LIVE_REGION}:s3" \
        --user "${RD_S3_LIVE_ACCESS_KEY}:${RD_S3_LIVE_SECRET}" \
        "${RD_S3_LIVE_ENDPOINT}/${RD_S3_LIVE_BUCKET}"
}

# clamd's own PING, polled from the runner once the build has given it its time.
clamd_ready() {
    local answer
    for _ in $(seq 1 180); do
        answer="$( (exec 3<>/dev/tcp/127.0.0.1/3310 && printf 'zPING\0' >&3 \
            && timeout 5 head -c 4 <&3) 2>/dev/null || true)"
        [[ "${answer}" == "PONG" ]] && exit 0
        sleep 1
    done
    echo "::error::clamd did not answer PING within 180 s"
    docker logs "$1" || true
    exit 1
}

case "${1:-}" in
    minio-start) minio_start ;;
    minio-ready) minio_ready ;;
    clamd-ready) clamd_ready "${2:?usage: scripts/ci-services.sh clamd-ready <container id>}" ;;
    *)
        echo "usage: scripts/ci-services.sh minio-start|minio-ready|clamd-ready <container id>" >&2
        exit 2
        ;;
esac
