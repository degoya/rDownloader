#!/usr/bin/env bash
#
# The screenshot demo's provider API over TLS, scripts/screenshots/remote_mock.py behind mock.py
# (RD-1490-13), on a free port: the mock makes its throwaway CA and a server certificate into
# <RD_SHOTS_WORK>/tls/ — a CA certificate, a server certificate for the API host it signed, the
# keys readable by their owner alone — answers a proxy CONNECT to that host over TLS with it, and
# refuses one to any other host. Through the tunnel, verified against that CA alone, Real-Debrid's
# API answers the way plugins/realdebrid-torrents reads it: a token refused without one, a magnet
# of the demo taken as the torrent the job list then shows in its state; the magnets seed.mjs
# submits come over plain HTTP.
#
# python3 and openssl, no service and no build: a few seconds.
#
#   scripts/tests/screenshots-tls.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOL="$ROOT/scripts/screenshots"
SCRATCH="$(mktemp -d)"
server=""
trap '[[ -n "$server" ]] && kill "$server" 2> /dev/null; rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

if ! command -v openssl > /dev/null; then
    fail "openssl is not installed: the demo's TLS mock makes its certificates with it"
    finish_tests screenshots-tls
fi

# The client: one check per call, its answer on stdout.
cat > "$SCRATCH/client.py" <<'PY'
import http.client, json, socket, ssl, sys
from urllib.parse import quote

port, ca, check = int(sys.argv[1]), sys.argv[2], sys.argv[3]
HOST = 'api.real-debrid.com'

def tunnel(host=HOST):
    """A CONNECT through the mock, as reqwest asks an HTTP proxy for an https address."""
    s = socket.create_connection(('127.0.0.1', port), timeout=10)
    s.sendall(f'CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n'.encode())
    head = b''
    while b'\r\n\r\n' not in head:
        chunk = s.recv(1024)
        if not chunk:
            break
        head += chunk
    return s, head.split(b'\r\n', 1)[0].decode()

def api(method, path, body=None, token='not-a-real-token'):
    s, status = tunnel()
    assert status.startswith('HTTP/1.1 200'), status
    context = ssl.create_default_context(cafile=ca)  # that CA alone, with the host name checked
    connection = http.client.HTTPConnection(HOST)
    connection.sock = context.wrap_socket(s, server_hostname=HOST)
    headers = {'Authorization': f'Bearer {token}'} if token else {}
    if body is not None:
        headers['Content-Type'] = 'application/x-www-form-urlencoded'
    connection.request(method, '/rest/1.0' + path, body=body, headers=headers)
    response = connection.getresponse()
    data = response.read()
    return response.status, (json.loads(data) if data else None)

if check == 'refused':
    s, status = tunnel('www.example.org')
    print(status.split(' ', 2)[1])
elif check == 'untrusted':
    s, _ = tunnel()
    try:
        ssl.create_default_context().wrap_socket(s, server_hostname=HOST)
        print('trusted by the system roots')
    except ssl.SSLCertVerificationError:
        print('refused without the demo CA')
elif check == 'token':
    status, body = api('GET', '/user', token=None)
    print(status, body['error_code'])
elif check == 'user':
    status, body = api('GET', '/user')
    print(status, body['type'], body['premium'] > 0)
elif check == 'jobs':
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    connection.request('GET', '/remote-jobs/magnets.json')
    magnets = json.loads(connection.getresponse().read())
    states = []
    for entry in magnets:
        status, added = api('POST', '/torrents/addMagnet', body='magnet=' + quote(entry['magnet'], safe=''))
        assert status == 201 and len(added['id']) == 13, (status, added)
        _, info = api('GET', f"/torrents/info/{added['id']}")
        assert info['filename'] == entry['name'], info
        states.append(info['status'] + (' links' if info['links'] else ''))
    status, listed = api('GET', '/torrents')
    print(len(magnets), len(listed), ' '.join(sorted(states)))
elif check == 'unknown':
    status, body = api('POST', '/torrents/addMagnet', body='magnet=' + quote('magnet:?xt=urn:btih:' + '0' * 40, safe=''))
    gone, missing = api('GET', '/torrents/info/NOPE')
    print(status, gone, missing['error_code'])
PY

free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
port="$(free_port)"
RD_SHOTS_WORK="$SCRATCH/work" python3 "$TOOL/mock.py" "$port" "$(free_port)" > "$SCRATCH/mock.log" 2>&1 &
server=$!
for _ in $(seq 1 100); do
    python3 -c 'import socket, sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1).close()' "$port" 2> /dev/null && break
    sleep 0.1
done
client() { run_status python3 -B "$SCRATCH/client.py" "$port" "$SCRATCH/work/tls/ca.pem" "$@"; }

tls="$SCRATCH/work/tls"
expect_true "the mock made its CA and server certificate in the work directory" \
    "[[ -s '$tls/ca.pem' && -s '$tls/server.pem' && -s '$tls/server.key' && -s '$tls/ca.key' ]]"
run_status openssl x509 -in "$tls/ca.pem" -noout -ext basicConstraints
expect_output "the CA is a CA" "CA:TRUE"
run_status openssl verify -CAfile "$tls/ca.pem" -purpose sslserver -verify_hostname api.real-debrid.com "$tls/server.pem"
expect_status "the CA signed a server certificate for the API host" 0
expect "the keys are their owner's alone" "600 600" "$(stat -c %a "$tls/ca.key") $(stat -c %a "$tls/server.key")"
expect "no request file is left beside them" "ca.key ca.pem server.key server.pem" "$(cd "$tls" && echo *)"

client refused
expect "a CONNECT to another host gets no tunnel" "502" "$output"
client untrusted
expect "the system roots do not trust the demo CA" "refused without the demo CA" "$output"
client token
expect "without a token Real-Debrid's bad_token" "401 8" "$output"
client user
expect "the account is premium" "200 premium True" "$output"
client jobs
expect_status "every demo magnet through the tunnel" 0
expect "five torrents: two running, one queued, one finished with its link, one dead" \
    "5 5 dead downloaded links downloading downloading queued" "$output"
client unknown
expect "an unknown magnet and an unknown id are refused in Real-Debrid's words" "400 404 7" "$output"

finish_tests screenshots-tls
