#!/usr/bin/env bash
#
# Builds and runs the container image the way this machine needs it.
#
# Three things this encodes:
#
#  * Docker Desktop writes `credsStore: desktop.exe` into ~/.docker/config.json. With WSL
#    interop off that helper cannot be executed and every image pull dies with
#    "exec format error", so the build runs against a credential-free config.
#  * `cargo build` inside the image would use every core. Unbounded cargo parallelism has taken
#    this machine down before, so JOBS is passed in as a build argument.
#  * The build context has no .git, so the commit and build time the About page shows are
#    worked out here, by rd_build_stamp, and passed in as build arguments (RD-130-12).
#  * dist/plugins is gitignored. Without scripts/build-plugins.sh first, the image ships no
#    bundled plugins at all -- silently, until the service logs it at startup.
#
# Usage:
#   scripts/docker.sh build [--tag rdownloader:local]
#   scripts/docker.sh run   [--port 8710] [--tag rdownloader:local] [--name rdownloader]
#   scripts/docker.sh stop  [--name rdownloader]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
TAG="rdownloader:local"
PORT="8710"
NAME="rdownloader"

command="${1:-}"
shift || true
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag) TAG="$2"; shift 2 ;;
        --port) PORT="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

# A config directory with no credsStore, so the daemon never reaches for the Windows helper.
docker_config="$(mktemp -d)"
trap 'rm -rf "$docker_config"' EXIT
echo '{"auths":{}}' > "$docker_config/config.json"
export DOCKER_CONFIG="$docker_config"

case "$command" in
build)
    shopt -s nullglob
    packages=(dist/plugins/*.rdplug)
    shopt -u nullglob
    if [[ ${#packages[@]} -eq 0 ]]; then
        echo "!! dist/plugins is empty -- the image will ship no bundled plugins." >&2
        echo "   Run scripts/build-plugins.sh first if you want them." >&2
    else
        echo "==> bundling ${#packages[@]} signed plugin packages"
    fi
    # shellcheck source=lib/version-file.sh
    source "$ROOT/scripts/lib/version-file.sh"
    version="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
    rd_build_stamp "${version:?version not found in Cargo.toml}"
    echo "==> building $TAG (cargo -j $JOBS, commit $RD_BUILD_COMMIT)"
    docker build \
        --file docker/Dockerfile \
        --build-arg "RD_BUILD_JOBS=$JOBS" \
        --build-arg "RD_BUILD_COMMIT=$RD_BUILD_COMMIT" \
        --build-arg "RD_BUILD_TIME=$RD_BUILD_TIME" \
        --tag "$TAG" \
        .
    ;;

run)
    docker rm --force "$NAME" > /dev/null 2>&1 || true
    echo "==> starting $NAME on http://127.0.0.1:$PORT"
    docker run --detach \
        --name "$NAME" \
        --publish "127.0.0.1:$PORT:8710" \
        --volume "$NAME-config:/config" \
        --volume "$NAME-downloads:/downloads" \
        --env "PUID=$(id -u)" \
        --env "PGID=$(id -g)" \
        --env "TZ=$(cat /etc/timezone 2>/dev/null || echo UTC)" \
        --env "RUST_LOG=rdownloader=info,rd_=info" \
        --restart unless-stopped \
        "$TAG"
    echo "    docker logs -f $NAME"
    ;;

stop)
    docker rm --force "$NAME"
    ;;

*)
    echo "usage: scripts/docker.sh {build|run|stop} [--tag T] [--port P] [--name N]" >&2
    exit 2
    ;;
esac
