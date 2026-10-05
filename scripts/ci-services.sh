#!/usr/bin/env bash
#
# The live services of ci.yml's `s3-live` and `clamav-live` jobs (RD-150-04, RD-190-14): the S3
# service started as a step, it and clamd waited for (RD-1101-07 moved the steps out of the workflow
# as they were). CI only; the test build in between gives both their time.
#
# The S3 service is RustFS (RD-1110-08): MinIO is archived and no longer publishes images, and the
# frozen bitnamilegacy/minio build it replaced received no fixes. RustFS starts its server on its own
# on 9000, takes the root credentials from RUSTFS_ACCESS_KEY/RUSTFS_SECRET_KEY and answers /health;
# it is pinned by its index digest in the job's S3_IMAGE (1.0.1, 2026-10-03), the credentials are
# the throw-away container's own.
# It stays a step, as MinIO was one (it needed `server /data`, which `services:` cannot pass); a
# `services:` container would do now, the step keeps the container's name, `s3`, for its log.
# The clamd image starts clamd on 3310 by itself; freshclam updating its database at start is not
# waited for.
#
#   scripts/ci-services.sh s3-start             # docker run S3_IMAGE with the RD_S3_LIVE_* keys
#   scripts/ci-services.sh s3-ready             # /health, then the bucket
#   scripts/ci-services.sh clamd-ready <id>     # clamd's own PING; <id> is the service container
#
# Reads S3_IMAGE and RD_S3_LIVE_ENDPOINT, _REGION, _BUCKET, _ACCESS_KEY and _SECRET from the job's
# environment.
set -euo pipefail

s3_start() {
    docker run --detach --name s3 --publish 127.0.0.1:9000:9000 \
        --env RUSTFS_ACCESS_KEY="${RD_S3_LIVE_ACCESS_KEY}" \
        --env RUSTFS_SECRET_KEY="${RD_S3_LIVE_SECRET}" \
        "${S3_IMAGE}"
}

# The build gave the service its time; this waits for the rest, polling /health from the runner,
# then makes the bucket with a signed CreateBucket (curl's own SigV4), since object_store creates
# none.
s3_ready() {
    local attempt
    for attempt in $(seq 1 60); do
        curl --silent --fail --output /dev/null "${RD_S3_LIVE_ENDPOINT}/health" && break
        if [[ ${attempt} -eq 60 ]]; then
            echo "::error::the S3 service did not answer /health within 60 s"
            docker logs s3
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
    s3-start) s3_start ;;
    s3-ready) s3_ready ;;
    clamd-ready) clamd_ready "${2:?usage: scripts/ci-services.sh clamd-ready <container id>}" ;;
    *)
        echo "usage: scripts/ci-services.sh s3-start|s3-ready|clamd-ready <container id>" >&2
        exit 2
        ;;
esac
