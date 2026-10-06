#!/usr/bin/env bash
#
# scripts/docker-smoke.sh against a stub `docker` and `curl`: the image starts on two fresh named
# volumes, answers its health check, and every bundled Python tool — yt-dlp, streamlink,
# gallery-dl, apprise — answers `--version` as the service user (RD-1120-07); a tool that does not
# fails the smoke test, and the volumes and the container are removed either way.
#
#   scripts/tests/docker-smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
# docker: logs every call; `run` prints a container id; `exec` of the tool in $FAKE/broken fails.
cat > "$SCRATCH/bin/docker" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/docker.calls"
case "$1" in
    run) echo c0ffee ;;
    exec) [[ -f "$FAKE/broken" && "$*" == *" $(cat "$FAKE/broken") --version" ]] && exit 127 ;;
esac
exit 0
STUB
printf '#!/usr/bin/env bash\necho "{\\"status\\":\\"ok\\"}"\n' > "$SCRATCH/bin/curl"
chmod +x "$SCRATCH/bin/docker" "$SCRATCH/bin/curl"
export PATH="$SCRATCH/bin:$PATH"
smoke() { rm -f "$FAKE/docker.calls"; run_status env GITHUB_RUN_ID=7 GITHUB_RUN_ATTEMPT=1 "$ROOT/scripts/docker-smoke.sh" rdownloader:smoke; }

smoke
expect_status "a working image" 0
expect_output "says so" "rdownloader:smoke passed the smoke test"
for tool in yt-dlp streamlink gallery-dl apprise; do
    expect_true "$tool answers as the service user" \
        "grep -qx 'exec --user rdownloader c0ffee $tool --version' \"\$FAKE/docker.calls\""
done
expect_true "on two fresh volumes" \
    'grep -q -- "--volume rdownloader-config-7-1:/config --volume rdownloader-downloads-7-1:/downloads rdownloader:smoke" "$FAKE/docker.calls"'
expect_true "removed afterwards" 'grep -qx "rm --force c0ffee" "$FAKE/docker.calls" && grep -qx "volume rm rdownloader-config-7-1 rdownloader-downloads-7-1" "$FAKE/docker.calls"'

echo streamlink > "$FAKE/broken"
smoke
expect_status "a tool that does not start fails the smoke test" 127
expect_true "the tools after it are not asked" '! grep -q "apprise --version" "$FAKE/docker.calls"'
expect_true "and the container is still removed" 'grep -qx "rm --force c0ffee" "$FAKE/docker.calls"'

run_status "$ROOT/scripts/docker-smoke.sh"
expect_status "without an image" 1

finish_tests "docker-smoke"
