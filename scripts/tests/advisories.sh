#!/usr/bin/env bash
#
# scripts/advisories.sh (RD-1480-14, SEC-5) against a server of this test that answers as OSV's
# `querybatch` and `vulns` do, pages included: a finding fails, a withdrawn one does not, a waiver
# counts by alias and has to match something, and a server that is gone or answers with fewer
# results than questions is no all-clear. And against this checkout's lock files, asking nobody.
#
#   scripts/tests/advisories.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
server_pid=""
# shellcheck disable=SC2329  # the EXIT trap calls it
cleanup() {
    [[ -z "$server_pid" ]] || kill "$server_pid" 2> /dev/null || true
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
export PYTHONDONTWRITEBYTECODE=1

run_status "$ROOT/scripts/advisories.sh" --no-such-option
expect_status "an unknown option" 2
run_status "$ROOT/scripts/advisories.sh" --list
expect_status "the list of this checkout" 0
expect_output "holds the container's tools" "PyPI yt-dlp "
expect_output "and their dependencies, without the marker" "PyPI urllib3 "
expect_output "and the interface's packages" "npm @nuxt/ui "
expect "one line per hashed Python package" "$(grep -cE '^[A-Za-z0-9].*==' "$ROOT/docker/requirements.txt")" \
    "$(grep -c '^PyPI ' <<< "$output")"

tree="$SCRATCH/tree"
mkdir -p "$tree/docker" "$tree/web" "$tree/scripts"
printf '%s\n' '# header' 'lxml==1.0 \' '    --hash=sha256:aa' "cffi==2.0 ; os_name == 'nt' \\" \
    '    --hash=sha256:bb' > "$tree/docker/requirements.txt"
printf '%s\n' 'packages:' '' "  '@scope/name@1.2.3':" '    resolution: {integrity: sha512-AA}' '' \
    '  vue@3.5.0:' '    resolution: {integrity: sha512-AA}' '' 'snapshots:' '' '  other@9.9.9: {}' \
    > "$tree/web/pnpm-lock.yaml"
allow() { printf '%s\n' '# waivers' "$@" > "$tree/scripts/advisories-allow.txt"; }
allow

# What the server knows: `<name>=<id>[,<id>]...` in $SCRATCH/vulns, read at every request. The
# second id of a package comes on a second page.
cat > "$SCRATCH/server.py" <<'EOF'
import http.server, json, pathlib, sys
scratch = pathlib.Path(sys.argv[1])
class Handler(http.server.BaseHTTPRequestHandler):
    def answer(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_POST(self):
        queries = json.loads(self.rfile.read(int(self.headers["Content-Length"])))["queries"]
        known = dict(line.split("=") for line in (scratch / "vulns").read_text().split())
        results = []
        for query in queries:
            ids = known.get(query["package"]["name"], "").split(",")
            page = int(query.get("page_token", "0"))
            result = {"vulns": [{"id": ids[page]}]} if ids[page] else {}
            if page + 1 < len(ids):
                result["next_page_token"] = str(page + 1)
            results.append(result)
        self.answer({"results": results[1:] if (scratch / "short").exists() else results})
    def do_GET(self):
        identity = self.path.rsplit("/", 1)[-1]
        body = {"id": identity, "summary": "a  flaw\nin it", "aliases": ["CVE-" + identity]}
        if identity.startswith("OLD"):
            body["withdrawn"] = "2026-01-01T00:00:00Z"
        self.answer(body)
    def log_message(self, *_):
        pass
server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
(scratch / "port.tmp").write_text(str(server.server_address[1]))
(scratch / "port.tmp").rename(scratch / "port")
server.serve_forever()
EOF
: > "$SCRATCH/vulns"
python3 -I "$SCRATCH/server.py" "$SCRATCH" &
server_pid=$!
for _ in $(seq 100); do [[ -e "$SCRATCH/port" ]] && break; sleep 0.1; done
port="$(cat "$SCRATCH/port")"
ask() { env -u http_proxy -u HTTP_PROXY RD_ADVISORIES_ROOT="$tree" RD_OSV_URL="http://127.0.0.1:$port" "$ROOT/scripts/advisories.sh"; }

run_status ask
expect_status "nothing known" 0
expect_output "says what was asked" "2 Python and 2 npm packages, 0 finding(s), 0 waived"
echo 'lxml=PYSEC-1,PYSEC-2 @scope/name=OLD-3 other=NEVER-4' > "$SCRATCH/vulns"
run_status ask
expect_status "a known flaw" 1
expect_output "is named with its package and address" "!! PyPI lxml 1.0: PYSEC-1 a flaw in it https://osv.dev/PYSEC-1"
expect_output "the one on the second page as well" "PYSEC-2"
expect "a withdrawn one and a package that is not locked are not" "2" "$(grep -c '^!! ' <<< "$output")"
allow 'PYSEC-1 the tool never reads this format' 'CVE-PYSEC-2 the same, by its alias'
run_status ask
expect_status "waived, by id and by alias" 0
expect_output "and counted" "0 finding(s), 2 waived"
allow 'PYSEC-1 reason' 'CVE-PYSEC-2 reason' 'PYSEC-9 long gone'
run_status ask
expect_status "a waiver nothing matches" 1
expect_output "is named" "PYSEC-9 matches nothing any more"
allow 'PYSEC-1'
run_status ask
expect_true "a waiver without its reason is refused" '[[ $status -ne 0 ]]'
expect_output "and named" "PYSEC-1 has no reason"
allow
: > "$SCRATCH/short"
run_status ask
expect_status "an answer that leaves a package out is no all-clear" 2
expect_output "and says so" "nothing checked"
kill "$server_pid"
wait "$server_pid" 2> /dev/null || true
server_pid=""
run_status ask
expect_status "nor is a server that is gone" 2
printf '# empty\n' > "$tree/docker/requirements.txt"
run_status ask
expect_status "nor a lock file nothing was read from" 2
expect_output "which is named" "nothing to ask for"

finish_tests "advisories"
