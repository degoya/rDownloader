#!/usr/bin/env bash
#
# The screenshot demo's news server, scripts/screenshots/nntp_mock.py (RD-1270-24), on a free
# port, spoken to the way rd-usenet speaks: directly and through its SOCKS5 greeting, AUTHINFO,
# GROUP, STAT, pipelined BODY commands. Every article decodes — yEnc read the way
# crates/rd-usenet/src/yenc.rs reads it — to its part of the file with the CRC its =yend names,
# an unknown one is 430, the NZB names every article, and a throttled connection takes as long
# as its rate says.
#
# python3 only, no service and no build: a few seconds.
#
#   scripts/tests/screenshots-nntp.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOL="$ROOT/scripts/screenshots"
SCRATCH="$(mktemp -d)"
server=""
trap '[[ -n "$server" ]] && kill "$server" 2> /dev/null; rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# The client: one check per call, its answer on stdout.
cat > "$SCRATCH/client.py" <<'PY'
import re, socket, sys, time, zlib
sys.path.insert(0, sys.argv[1])
import nntp_mock

port, check = int(sys.argv[2]), sys.argv[3]

def connect(socks=False):
    s = socket.create_connection(('127.0.0.1', port), timeout=20)
    if socks:
        host = b'news.example.com'
        s.sendall(b'\x05\x01\x00')
        assert s.recv(2) == b'\x05\x00', 'socks method'
        s.sendall(b'\x05\x01\x00\x03' + bytes([len(host)]) + host + (119).to_bytes(2, 'big'))
        reply = b''
        while len(reply) < 10:
            reply += s.recv(10 - len(reply))
        assert reply[:2] == b'\x05\x00', f'socks connect {reply!r}'
    return s, s.makefile('rb')

def status(f):
    return f.readline().decode().rstrip('\r\n')

def body(f):
    lines = []
    while True:
        line = f.readline()
        if line in (b'.\r\n', b''):
            return b''.join(lines)
        lines.append(line[1:] if line.startswith(b'..') else line)

def decode(article):
    """yEnc as crates/rd-usenet/src/yenc.rs reads it."""
    fields = lambda line: dict(f.split('=', 1) for f in line.decode().split()[1:] if '=' in f)
    lines = [l.rstrip(b'\r') for l in article.split(b'\n')]
    begin = fields(next(l for l in lines if l.startswith(b'=ybegin ')))
    out, part, end = bytearray(), None, None
    for line in lines[lines.index(next(l for l in lines if l.startswith(b'=ybegin '))) + 1:]:
        if line.startswith(b'=ypart '):
            part = fields(line)
            continue
        if line.startswith(b'=yend '):
            end = fields(line)
            break
        i = 0
        while i < len(line):
            v = line[i]
            if v == 0x3D:
                i += 1
                v = (line[i] - 64) & 0xFF
            out.append((v - 42) & 0xFF)
            i += 1
    assert end and int(end['size']) == len(out), 'part size'
    return begin, part, end, bytes(out)

if check == 'session':
    for socks in (False, True):
        s, f = connect(socks)
        got = [status(f).split(' ')[0]]
        for cmd in ('AUTHINFO USER demo-user', 'AUTHINFO PASS not-a-real-password', 'GROUP alt.binaries.example',
                    'STAT <demo-bpf-part2-7@example.invalid>', 'STAT <nope@example.invalid>', 'BODY <nope@example.invalid>',
                    'MODE READER', 'QUIT'):
            s.sendall(cmd.encode() + b'\r\n')
            got.append(status(f).split(' ')[0])
        print(('socks ' if socks else 'direct ') + ' '.join(got))
elif check == 'articles':
    s, f = connect(True)
    status(f)
    wanted = [(1, 1), (2, 300), (3, nntp_mock.PARTS)]
    # Pipelined, as rd-usenet sends them: every command first, then the answers in order.
    s.sendall(b''.join(f'BODY <{nntp_mock.message_id(n, k)}>\r\n'.encode() for n, k in wanted))
    for number, part in wanted:
        line = status(f)
        assert line == f'222 0 <{nntp_mock.message_id(number, part)}>', line
        begin, ypart, end, data = decode(body(f))
        start = (part - 1) * nntp_mock.PART_SIZE
        # The file is bytes(range(256)) over and over; this part is its slice.
        assert data == bytes((start + i) % 256 for i in range(nntp_mock.PART_SIZE)), 'payload'
        assert int(end['pcrc32'], 16) == zlib.crc32(data), 'crc'
        assert begin['name'] == nntp_mock.file_name(number) and int(begin['part']) == part, 'ybegin'
        assert int(begin['size']) == nntp_mock.PARTS * nntp_mock.PART_SIZE, 'file size'
        assert (int(ypart['begin']), int(ypart['end'])) == (start + 1, start + nntp_mock.PART_SIZE), 'ypart'
        print(f'part{number}-{part} ok')
elif check == 'escapes':
    lines = nntp_mock.yenc_lines(bytes(range(256)) * 3)
    bad = [l for l in lines if l[:1] in (b'.', b'\t', b' ') or l[-1:] in (b'\t', b' ')
           or any(c in l for c in (b'\x00', b'\r', b'\n'))]
    print('edges clean' if not bad else f'bad lines: {bad[:2]!r}')
elif check == 'nzb':
    doc = nntp_mock.nzb_document()
    ids = re.findall(r'>(demo-bpf-[^<]+)</segment>', doc)
    files = re.findall(r'subject="&quot;([^&]+)&quot;', doc)
    sizes = set(re.findall(r'bytes="(\d+)"', doc))
    print(len(files), len(ids), all(nntp_mock.article(i) for i in ids), sizes == {str(len(nntp_mock.article(ids[0])))})
elif check == 'throttle':
    s, f = connect()
    status(f)
    started = time.monotonic()
    s.sendall(f'BODY <{nntp_mock.message_id(1, 2)}>\r\n'.encode())
    status(f)
    size = len(body(f))
    print(f'{time.monotonic() - started:.2f} {size}')
PY

free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
start() {  # start <rate>: the server alone, ready once it accepts
    [[ -n "$server" ]] && { kill "$server" 2> /dev/null || true; wait "$server" 2> /dev/null || true; }
    port="$(free_port)"
    RD_SHOTS_NNTP_RATE="$1" python3 "$TOOL/nntp_mock.py" "$port" > "$SCRATCH/server.log" 2>&1 &
    server=$!
    for _ in $(seq 1 50); do
        python3 -c 'import socket, sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1).close()' "$port" 2> /dev/null && return 0
        sleep 0.1
    done
    fail "the news server did not start" "$(cat "$SCRATCH/server.log")"
    finish_tests screenshots-nntp
}
client() { run_status python3 -B "$SCRATCH/client.py" "$TOOL" "$port" "$@"; }

start 0
client session
expect_status "a session" 0
expect "direct: greeting, AUTHINFO, GROUP, STAT, 430 for an unknown article, 500 for the rest, QUIT" \
    "direct 200 381 281 211 223 430 430 500 205
socks 200 381 281 211 223 430 430 500 205" "$output"

client articles
expect_status "pipelined BODY through SOCKS5" 0
expect "each article decodes to its part of the file, with the CRC its =yend names" \
    "part1-1 ok
part2-300 ok
part3-600 ok" "$output"

client escapes
expect "no encoded line starts with a dot or blank, ends blank or holds NUL, CR or LF" "edges clean" "$output"

client nzb
expect "the NZB names three files of six hundred articles the server has, each its real size" "3 1800 True True" "$output"

# 256 KiB/s against an article of about 730 KB: close to three seconds, never under 2.5.
start $((256 * 1024))
client throttle
expect_status "a throttled article" 0
expect_true "takes as long as its rate says ($output)" \
    'python3 -c "import sys; t, n = sys.argv[1].split(); sys.exit(0 if float(t) >= 0.9 * int(n) / (256 * 1024) else 1)" "$output"'

finish_tests screenshots-nntp
