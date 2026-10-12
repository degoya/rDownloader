#!/usr/bin/env python3
"""NNTP news server for the screenshot demo (RD-1270-24), started by mock.py on a port of its own.

The `usenet` group needs a Usenet download that holds connections while the views are taken, so
this answers what rd-usenet sends (`crates/rd-usenet/src/nntp.rs`): the greeting, `AUTHINFO
USER`/`PASS`, `BODY`, and besides `GROUP`, `STAT` and `QUIT`; anything else is `500`. Commands
are answered one at a time in their order, which covers pipelining too.

One port speaks both ways: a client that opens with a SOCKS5 greeting (no authentication) gets
the SOCKS5 answers first and then NNTP on the same socket, whatever target it named; one that
waits gets the NNTP greeting. The demo goes through a SOCKS5 proxy profile, so the server card
keeps showing news.example.com instead of 127.0.0.1.

The articles are the yEnc parts of the files of one NZB (`nzb_document`, served by mock.py at
NZB_PATH): every part decodes to the same bytes, so one encoding and one CRC serve all of them.
Each connection sends at RD_SHOTS_NNTP_RATE bytes per second (default 48 KiB/s, about fifteen
seconds an article), so a connection is nearly always inside an answer and counts as busy.

    nntp_mock.py <port>        this server alone
"""
import functools
import os
import re
import select
import socketserver
import sys
import time
import zlib

GROUP = 'alt.binaries.example'
STEM = 'blender-production-files'
NZB_PATH = f'/nzb/{STEM}.nzb'
FILES = 3
PARTS = 600
PART_SIZE = 716800  # a multiple of 256: every part holds the same bytes
LINE = 128
ARTICLE = re.compile(r'<?demo-bpf-part(\d+)-(\d+)@example\.invalid>?$')
PAYLOAD = bytes(range(256)) * (PART_SIZE // 256)


def rate():
    """Bytes per second each connection sends; 0 is unthrottled."""
    return int(os.environ.get('RD_SHOTS_NNTP_RATE', str(48 * 1024)))


def file_name(number):
    return f'{STEM}.part{number}.rar'


def message_id(number, part):
    return f'demo-bpf-part{number}-{part}@example.invalid'


def yenc_lines(data, line=LINE):
    """yEnc lines of `data`: byte + 42, `=`-escaped NUL, LF, CR and `=`, a dot at a line's start,
    and a tab or space at its start or end."""
    lines, current = [], bytearray()
    last = len(data) - 1
    for index, byte in enumerate(data):
        out = (byte + 42) & 0xFF
        at_edge = not current or len(current) >= line - 1 or index == last
        if out in (0x00, 0x0A, 0x0D, 0x3D) or (out in (0x09, 0x20) and at_edge) or (out == 0x2E and not current):
            current += bytes((0x3D, (out + 64) & 0xFF))
        else:
            current.append(out)
        if len(current) >= line:
            lines.append(bytes(current))
            current = bytearray()
    if current:
        lines.append(bytes(current))
    return lines


@functools.lru_cache(maxsize=1)
def encoded_part():
    """The encoded lines every part shares, with their CRC32."""
    return b'\r\n'.join(yenc_lines(PAYLOAD)) + b'\r\n', zlib.crc32(PAYLOAD)


def article(mid):
    """The body of a message id as `BODY` sends it, dot-terminated; None for an unknown one."""
    match = ARTICLE.match(mid.strip())
    if not match:
        return None
    number, part = int(match.group(1)), int(match.group(2))
    if not (1 <= number <= FILES and 1 <= part <= PARTS):
        return None
    lines, crc = encoded_part()
    begin = (part - 1) * PART_SIZE + 1
    head = (f'=ybegin part={part} total={PARTS} line={LINE} size={PARTS * PART_SIZE} name={file_name(number)}\r\n'
            f'=ypart begin={begin} end={begin + PART_SIZE - 1}\r\n').encode()
    tail = f'=yend size={PART_SIZE} part={part} pcrc32={crc:08x}\r\n.\r\n'.encode()
    return head + lines + tail


def nzb_document():
    """The NZB of the demo's Usenet package: FILES files of PARTS segments each."""
    size = len(article(message_id(1, 1)))
    files = []
    for number in range(1, FILES + 1):
        segments = ''.join(f'<segment bytes="{size}" number="{part}">{message_id(number, part)}</segment>'
                           for part in range(1, PARTS + 1))
        files.append(f'<file poster="demo@example.invalid" date="{int(time.time())}" '
                     f'subject="&quot;{file_name(number)}&quot; yEnc (1/{PARTS})">'
                     f'<groups><group>{GROUP}</group></groups><segments>{segments}</segments></file>')
    return ('<?xml version="1.0" encoding="UTF-8"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">'
            + ''.join(files) + '</nzb>')


class Handler(socketserver.StreamRequestHandler):
    def send(self, data, throttled=False):
        speed = rate() if throttled else 0
        if speed <= 0:
            self.wfile.write(data)
            return
        start, step = time.monotonic(), max(1024, speed // 20)
        for at in range(0, len(data), step):
            self.wfile.write(data[at:at + step])
            ahead = (at + step) / speed - (time.monotonic() - start)
            if ahead > 0:
                time.sleep(ahead)

    def line(self, text):
        self.send(f'{text}\r\n'.encode())

    def socks5(self):
        """Answers a SOCKS5 greeting and CONNECT; whatever target it names, NNTP follows."""
        version, count = self.rfile.read(2)
        methods = self.rfile.read(count)
        if version != 5 or 0 not in methods:
            self.send(b'\x05\xff')
            return False
        self.send(b'\x05\x00')
        _, command, _, kind = self.rfile.read(4)
        length = {1: 4, 4: 16}.get(kind) or self.rfile.read(1)[0]
        self.rfile.read(length + 2)
        if command != 1:
            self.send(b'\x05\x07\x00\x01' + bytes(6))
            return False
        self.send(b'\x05\x00\x00\x01' + bytes(6))
        return True

    def handle(self):
        try:
            # An NNTP client waits for the greeting; a SOCKS5 client speaks first.
            if select.select([self.connection], [], [], 0.3)[0] and self.connection.recv(1, 0x02) == b'\x05':
                if not self.socks5():
                    return
            self.line('200 rDownloader demo news server')
            while True:
                raw = self.rfile.readline(1024)
                if not raw:
                    return
                if not self.command(raw.decode('latin-1').strip()):
                    return
        except (BrokenPipeError, ConnectionResetError, ValueError, IndexError):
            pass

    def command(self, text):
        verb, _, rest = text.partition(' ')
        verb = verb.upper()
        if verb == 'AUTHINFO':
            kind = rest.split(' ', 1)[0].upper()
            self.line({'USER': '381 password required', 'PASS': '281 authentication accepted'}.get(kind, '501 syntax error'))
        elif verb == 'GROUP':
            count = FILES * PARTS
            self.line(f'211 {count} 1 {count} {GROUP}' if rest.strip() == GROUP else '411 no such group')
        elif verb in ('STAT', 'BODY'):
            mid = rest.strip().strip('<>')
            body = article(mid)
            if body is None:
                self.line('430 no such article')
            elif verb == 'STAT':
                self.line(f'223 0 <{mid}>')
            else:
                self.line(f'222 0 <{mid}>')
                self.send(body, throttled=True)
        elif verb == 'QUIT':
            self.line('205 bye')
            return False
        else:
            self.line('500 command not recognized')
        return True


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def serve(port):
    encoded_part()  # once, before the first client waits for it
    with Server(('127.0.0.1', port), Handler) as server:
        server.serve_forever()


if __name__ == '__main__':
    serve(int(sys.argv[1]) if len(sys.argv) > 1 else int(os.environ.get('RD_SHOTS_NNTP_PORT', '18792')))
