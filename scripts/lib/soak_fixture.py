"""The HTTP server a soak run downloads from (RD-180-12).

`/f/<seed>/<size>/<name>` answers `size` bytes that depend on `seed` and the offset only, so a
file that was resumed at the wrong offset, or had bytes invented for it, hashes differently from
`expected_sha256(seed, size)`. Ranges (single, open-ended and suffix), `If-Range` and a stable
`ETag` are served the way a resumable host serves them.

Every `outage_every` seconds the server goes away for `outage_for` seconds: the listening socket
is closed, so new connections are refused, and every body being written is cut off mid-stream.
It then listens on the same port again. That is the reconnect the service has to survive: its
transfers fail, wait, and resume with a range.

Standard library only; `scripts/tests/soak.sh` tests it without the service.
"""

from __future__ import annotations

import hashlib
import http.server
import random
import re
import socket
import threading
import time

# A pool of pseudo-random bytes whose length is prime-ish, so the content only repeats after
# ~4 MiB and a resume off by any amount below that changes the hash.
_POOL_SIZE = 4 * 1024 * 1024 + 7
_POOL = random.Random(180_12).randbytes(_POOL_SIZE)
_CHUNK = 64 * 1024
_PATH = re.compile(r"^/f/(\d+)/(\d+)/[A-Za-z0-9._-]+$")
_RANGE = re.compile(r"^bytes=(\d*)-(\d*)$")


def content(seed: int, offset: int, length: int) -> bytes:
    """`length` bytes of file `seed` from `offset` on."""
    start = (offset + seed * 104_729) % _POOL_SIZE
    out = bytearray()
    while len(out) < length:
        take = min(length - len(out), _POOL_SIZE - start)
        out += _POOL[start : start + take]
        start = 0
    return bytes(out)


def expected_sha256(seed: int, size: int) -> str:
    digest = hashlib.sha256()
    for offset in range(0, size, 1024 * 1024):
        digest.update(content(seed, offset, min(1024 * 1024, size - offset)))
    return digest.hexdigest()


class RateLimiter:
    """One token bucket for the whole fixture, so the line is shared the way a real one is."""

    def __init__(self, bytes_per_second: float):
        self.rate = bytes_per_second
        self.lock = threading.Lock()
        self.available = 0.0
        self.last = time.monotonic()

    def take(self, amount: int) -> None:
        if self.rate <= 0:
            return
        while True:
            with self.lock:
                now = time.monotonic()
                self.available = min(self.rate, self.available + (now - self.last) * self.rate)
                self.last = now
                if self.available >= amount:
                    self.available -= amount
                    return
                wait = (amount - self.available) / self.rate
            time.sleep(min(wait, 0.25))


class Fixture:
    def __init__(self, port: int = 0, rate_mib: float = 0, outage_every: float = 0,
                 outage_for: float = 0):
        self.limiter = RateLimiter(rate_mib * 1024 * 1024)
        self.outage_every = outage_every
        self.outage_for = outage_for
        self.down = threading.Event()
        self.stopping = threading.Event()
        self.lock = threading.Lock()
        self.sockets: set[socket.socket] = set()
        self.stats = {"requests": 0, "ranged": 0, "bytes": 0, "outages": 0, "cut": 0}
        self.server = self._listen(port)
        self.port = self.server.server_address[1]
        self.threads: list[threading.Thread] = []

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def _listen(self, port: int) -> http.server.ThreadingHTTPServer:
        fixture = self

        class Handler(_Handler):
            owner = fixture

        server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
        server.daemon_threads = True
        return server

    def start(self) -> "Fixture":
        self._serve()
        if self.outage_every > 0 and self.outage_for > 0:
            controller = threading.Thread(target=self._outages, daemon=True)
            controller.start()
            self.threads.append(controller)
        return self

    def _serve(self) -> None:
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.threads.append(thread)

    def _outages(self) -> None:
        while not self.stopping.wait(self.outage_every):
            self.go_down()
            if self.stopping.wait(self.outage_for):
                return
            self.come_back()

    def go_down(self) -> None:
        self.down.set()
        # Every connection, idle keep-alive ones included: a client that kept its socket open
        # must not be served through the outage.
        with self.lock:
            self.stats["outages"] += 1
            for sock in list(self.sockets):
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
        self.server.shutdown()
        self.server.server_close()

    def come_back(self) -> None:
        # The port was ours a moment ago; a few tries cover a socket still in its close.
        for attempt in range(50):
            try:
                self.server = self._listen(self.port)
                break
            except OSError:
                if attempt == 49:
                    raise
                time.sleep(0.1)
        self.down.clear()
        self._serve()

    def stop(self) -> None:
        self.stopping.set()
        if not self.down.is_set():
            self.server.shutdown()
            self.server.server_close()

    def count(self, key: str, amount: int = 1) -> None:
        with self.lock:
            self.stats[key] += amount


class _Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    owner: Fixture

    def log_message(self, format, *args):  # noqa: A002 - the base class's name
        pass

    def setup(self):
        super().setup()
        with self.owner.lock:
            self.owner.sockets.add(self.connection)

    def finish(self):
        with self.owner.lock:
            self.owner.sockets.discard(self.connection)
        try:
            super().finish()
        except OSError:
            pass

    def do_HEAD(self):  # noqa: N802
        self._answer(body=False)

    def do_GET(self):  # noqa: N802
        self._answer(body=True)

    def _answer(self, body: bool) -> None:
        fixture = self.owner
        if fixture.down.is_set():
            self.close_connection = True
            return
        match = _PATH.match(self.path)
        if not match:
            self.send_error(404)
            return
        seed, size = int(match[1]), int(match[2])
        etag = f'"{seed}-{size}"'
        start, end = 0, size - 1
        status = 200
        wanted = self.headers.get("Range")
        if_range = self.headers.get("If-Range")
        if wanted and (if_range is None or if_range == etag):
            parsed = _parse_range(wanted, size)
            if parsed is None:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{size}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            start, end = parsed
            status = 206
        fixture.count("requests")
        if status == 206 and start > 0:
            fixture.count("ranged")
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", etag)
        self.send_header("Last-Modified", "Tue, 29 Sep 2026 00:00:00 GMT")
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if not body:
            return
        try:
            offset = start
            while offset <= end:
                if fixture.down.is_set():
                    fixture.count("cut")
                    self.close_connection = True
                    return
                length = min(_CHUNK, end - offset + 1)
                fixture.limiter.take(length)
                self.wfile.write(content(seed, offset, length))
                fixture.count("bytes", length)
                offset += length
        except OSError:
            fixture.count("cut")
            self.close_connection = True


def _parse_range(value: str, size: int) -> tuple[int, int] | None:
    match = _RANGE.match(value.strip())
    if not match or (not match[1] and not match[2]):
        return None
    if not match[1]:
        suffix = int(match[2])
        if suffix == 0:
            return None
        return max(0, size - suffix), size - 1
    start = int(match[1])
    end = int(match[2]) if match[2] else size - 1
    if start >= size or end < start:
        return None
    return start, min(end, size - 1)
