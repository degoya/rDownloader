#!/usr/bin/env bash
#
# scripts/host-liveness.sh against scratch manifests and a stand-in `dig` and `curl` (RD-1280-20):
# the domains of resolver and crawler manifests are asked, wildcards and other plugin types are
# not; NXDOMAIN and an answer without an address are dead, an answering server is alive, a
# resolver failure, a timeout, a TLS error and a 5xx are unsure; the table, the JSON and the
# Markdown file say the same. Nothing here touches the network.
#
#   scripts/tests/host-liveness.sh
# shellcheck disable=SC2016  # expect_true evaluates its condition later
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts" "$TREE/docs"
cp "$ROOT/scripts/host-liveness.sh" "$TREE/scripts/"
plugin() {
    mkdir -p "$TREE/plugins/$1"
    printf 'manifest_version = 3\nplugin_type = "%s"\nname = "%s"\nversion = "0.1.0"\nmatch_domains = [%s]\n' \
        "$2" "$1" "$3" > "$TREE/plugins/$1/manifest.toml"
}
plugin alpha resolver '"alive.test", "www.alive.test", "moved.test", "gone.test", "*.cdn.alive.test"'
plugin bravo resolver '"empty.test", "slow.test", "broken.test", "busy.test", "tls.test", "alive.test"'
plugin charlie crawler '"crawl.test"'
plugin debrid resolver '"*"'
plugin delta notifier '"never.test"'

# The stand-ins answer from one table: `<domain> <dns> <https>`. dns: NOERROR (an A record),
# V6 (only AAAA), NOADDR (NOERROR, no record), NXDOMAIN, SERVFAIL, TIMEOUT (dig exit 9).
# https: a status, `<status>:<redirect host>`, or curl's exit code as `exit<n>`.
FAKE="$SCRATCH/bin"
mkdir -p "$FAKE"
cat > "$SCRATCH/hosts" <<'EOF'
alive.test NOERROR 200
www.alive.test V6 405
moved.test NOERROR 301:parking.example
gone.test NXDOMAIN -
empty.test NOADDR -
slow.test NOERROR exit28
broken.test SERVFAIL -
busy.test NOERROR 503
tls.test NOERROR exit60
crawl.test TIMEOUT -
EOF
cat > "$FAKE/dig" <<STUB
#!/usr/bin/env bash
domain="\${*: -2:1}" kind="\${*: -1}"
echo "\$domain \$kind" >> "$SCRATCH/asked"
read -r _ dns _ < <(grep "^\$domain " "$SCRATCH/hosts")
if [[ "\$dns" == TIMEOUT ]]; then echo ";; connection timed out; no servers could be reached"; exit 9; fi
status="\$dns"; [[ "\$dns" == NOERROR || "\$dns" == V6 || "\$dns" == NOADDR ]] && status=NOERROR
echo ";; ->>HEADER<<- opcode: QUERY, status: \$status, id: 4242"
if [[ "\$dns" == NOERROR && "\$kind" == A ]] || [[ "\$dns" == V6 && "\$kind" == AAAA ]]; then
    printf '%s.\t300\tIN\t%s\t192.0.2.1\n' "\$domain" "\$kind"
fi
STUB
cat > "$FAKE/curl" <<STUB
#!/usr/bin/env bash
url="\${*: -1}" domain="\${url#https://}" domain="\${domain%/}"
echo "\$domain" >> "$SCRATCH/fetched"
read -r _ _ https < <(grep "^\$domain " "$SCRATCH/hosts")
case "\$https" in
    exit*) echo "curl: (\${https#exit}) failed" >&2; printf '000 '; exit "\${https#exit}" ;;
    *:*) printf '%s https://%s/' "\${https%%:*}" "\${https#*:}" ;;
    *) printf '%s ' "\$https" ;;
esac
STUB
chmod +x "$FAKE/dig" "$FAKE/curl"
export PATH="$FAKE:$PATH"
liveness() { "$TREE/scripts/host-liveness.sh" "$@"; }

run_status liveness --bogus
expect_status "an unknown argument is a usage error" 2

run_status liveness --json
expect_status "a full run passes whatever the hosts answer" 0
printf '%s\n' "$output" | sed '/^host-liveness: /d' > "$SCRATCH/result.json"
# `status_of <domain>`: "<status> <detail>" of its row, or "absent"; `plugins_of`, `first_three`.
cat > "$SCRATCH/rows.py" <<'PY'
import json
import sys

rows = json.load(open(sys.argv[1]))["domains"]
by_domain = {row["domain"]: row for row in rows}
if sys.argv[2] == "status":
    row = by_domain.get(sys.argv[3])
    print(f"{row['status']} {row['detail']}" if row else "absent")
elif sys.argv[2] == "plugins":
    print(" ".join(by_domain[sys.argv[3]]["plugins"]))
else:
    print(" ".join(row["status"] for row in rows[:3]))
PY
status_of() { python3 "$SCRATCH/rows.py" "$SCRATCH/result.json" status "$1"; }
expect "an answering server is alive" "alive https: 200" "$(status_of alive.test)"
expect "an AAAA record alone resolves; 405 still answers" "alive https: 405" "$(status_of www.alive.test)"
expect "a redirect elsewhere stays alive and names its target" "alive https: 301 to parking.example" "$(status_of moved.test)"
expect "NXDOMAIN is dead" "dead dns: NXDOMAIN" "$(status_of gone.test)"
expect "no address is dead" "dead dns: no address" "$(status_of empty.test)"
expect "a timeout is unsure" "unsure https: timeout" "$(status_of slow.test)"
expect "a resolver failure is unsure" "unsure dns: SERVFAIL" "$(status_of broken.test)"
expect "a 5xx is unsure" "unsure https: 503" "$(status_of busy.test)"
expect "a certificate error is unsure" "unsure https: certificate" "$(status_of tls.test)"
expect "a crawler's domain is asked; an unreachable resolver is unsure" \
    "unsure dns: no answer from the resolver" "$(status_of crawl.test)"
expect "a notifier's domain is not asked" "absent" "$(status_of never.test)"
expect_true "wildcards are never asked" '! grep -q "\*" "$SCRATCH/asked"'
expect "a dead domain is never fetched" "0" "$(grep -c '^gone.test$' "$SCRATCH/fetched" || true)"
expect "a domain two plugins claim is one row naming both" "alpha bravo" \
    "$(python3 "$SCRATCH/rows.py" "$SCRATCH/result.json" plugins alive.test)"
expect "dead first, then unsure, then alive" "dead dead unsure" \
    "$(python3 "$SCRATCH/rows.py" "$SCRATCH/result.json" order)"
expect_output "the summary counts" "10 domains: 3 alive, 2 dead, 5 unsure"

DOC="$TREE/docs/plugins-liveness.md"
expect_true "a full run writes the Markdown result" '[[ -f "$DOC" ]]'
expect_true "with the date on every row" \
    'grep -qE "^\| [0-9]{4}-[0-9]{2}-[0-9]{2} \| gone\.test \| alpha \| dead \| dns: NXDOMAIN \|$" "$DOC"'
expect "one row per domain" "10" "$(grep -cE '^\| [0-9]{4}-' "$DOC")"

# Plugins named: only theirs, the table, and no file unless --out names one.
rm "$DOC"
run_status liveness charlie
expect_status "a named plugin passes" 0
expect_output "its domain is in the table" "crawl.test"
expect_true "and alpha's is not" '[[ "$output" != *gone.test* ]]'
expect_true "no file without --out" '[[ ! -e "$DOC" ]]'
run_status liveness charlie --out "$SCRATCH/charlie.md"
expect_true "--out writes there" 'grep -q "crawl.test" "$SCRATCH/charlie.md"'
run_status liveness charlie --out "$SCRATCH/new/dir/charlie.md"
expect_true "--out makes its directory, as a tree without docs/ needs" '[[ -f "$SCRATCH/new/dir/charlie.md" ]]'
run_status liveness delta
expect_status "a plugin without hoster domains is an error" 1
run_status liveness echo
expect_status "an unknown plugin is an error" 1

# Without dig, nothing is asked.
mkdir -p "$SCRATCH/nodig"
for tool in bash python3 curl sed dirname; do ln -s "$(command -v "$tool")" "$SCRATCH/nodig/$tool"; done
ln -sf "$FAKE/curl" "$SCRATCH/nodig/curl"
run_status env PATH="$SCRATCH/nodig" "$TREE/scripts/host-liveness.sh"
expect_status "a missing dig is an error" 1
expect_output "and named" "dig is required"

finish_tests host-liveness
