#!/usr/bin/env bash
#
# scripts/tools-manifest-check.sh against a loopback HTTP server (RD-1101-13): a download that
# answers with its declared size passes, one that answers 404 and one whose size changed fail,
# each named; two entries sharing an archive are asked once. Last, `--list` over this
# checkout's embedded manifest, which reads the file only — the live check is the weekly
# workflow's, never this test's.
#
# Python's http.server on 127.0.0.1: well under a second.
#
#   scripts/tests/tools-manifest-check.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/tools-manifest-check.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
server=""
cleanup() {
    [[ -n "$server" ]] && kill "$server" 2>/dev/null
    rm -rf "$SCRATCH"
}
trap cleanup EXIT

mkdir -p "$SCRATCH/www"
printf 'twelve bytes' > "$SCRATCH/www/archive.tar.xz"
printf 'raw' > "$SCRATCH/www/tool"

python3 - "$SCRATCH/www" "$SCRATCH/port" <<'EOF' > /dev/null 2>&1 &
import functools, http.server, sys
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
with open(sys.argv[2], "w") as port:
    port.write(str(server.server_address[1]))
server.serve_forever()
EOF
server=$!
for _ in $(seq 50); do
    [[ -s "$SCRATCH/port" ]] && break
    sleep 0.1
done
[[ -s "$SCRATCH/port" ]] || { echo "the loopback server did not start" >&2; exit 1; }
BASE="http://127.0.0.1:$(cat "$SCRATCH/port")"

# A signed document's shape: ffmpeg and ffprobe share one archive.
cat > "$SCRATCH/live.json" <<EOF
{"payload": {"tools": [
  {"name": "ffmpeg", "url": "$BASE/archive.tar.xz", "size": 12},
  {"name": "ffprobe", "url": "$BASE/archive.tar.xz", "size": 12},
  {"name": "yt-dlp", "url": "$BASE/tool", "size": 3}
]}, "signatures": []}
EOF

# A bare payload: one asset gone, one re-uploaded with other bytes.
cat > "$SCRATCH/dead.json" <<EOF
{"tools": [
  {"name": "ffmpeg", "url": "$BASE/gone.zip", "size": 12},
  {"name": "yt-dlp", "url": "$BASE/tool", "size": 4}
]}
EOF

run_status "$SCRIPT" "$SCRATCH/live.json"
expect_status "every download answering with its size passes" 0
expect_output "a shared archive is asked once" "2 download(s) in $SCRATCH/live.json answer"

run_status "$SCRIPT" "$SCRATCH/dead.json"
expect_status "a dead or changed download fails" 1
expect_output "a 404 is named" "DEAD $BASE/gone.zip: HTTP 404"
expect_output "a changed size is named" "SIZE $BASE/tool: 3, the manifest says 4"
expect_output "both are counted" "2 of 2 download(s)"

cat > "$SCRATCH/conflict.json" <<EOF
{"tools": [
  {"name": "ffmpeg", "url": "$BASE/archive.tar.xz", "size": 12},
  {"name": "ffprobe", "url": "$BASE/archive.tar.xz", "size": 13}
]}
EOF
run_status "$SCRIPT" --list "$SCRATCH/conflict.json"
expect_status "one URL with two sizes is refused" 1
expect_output "the URL is named" "$BASE/archive.tar.xz is named with two different sizes"

run_status "$SCRIPT" --list
expect_status "the embedded manifest is listed without the network" 0
expect "every embedded download is https" "" "$(grep -v '^https://' <<< "$output" || true)"

finish_tests "tools-manifest-check"
