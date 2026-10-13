"""The service side of the soak run (RD-180-12), split from soak.py (RD-1280-08), which imports
and re-exports every name here: the REST client, the sampled process, the samples file, and the
small helpers the run and the self-test share.

Standard library only; `psutil` where /proc is missing.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import http.cookiejar
import json
import os
import re
import socket
import urllib.error
import urllib.request
from pathlib import Path

import soak_budgets

MIB = 1024 * 1024


class RunError(Exception):
    """The run could not be carried out; no verdict on the budgets."""


def parse_duration(value: str) -> float:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)([smh]?)", value.strip())
    if not match:
        raise argparse.ArgumentTypeError(f"not a duration: {value!r} (e.g. 90s, 5m, 2h)")
    return float(match[1]) * {"": 1, "s": 1, "m": 60, "h": 3600}[match[2]]


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Api:
    """JSON over the service's REST API with a session cookie; signs in again on a 401."""

    def __init__(self, base: str, password: str):
        self.base = base
        self.password = password
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def call(self, method: str, path: str, body: object = None,
             relogin: bool = True) -> tuple[int, object]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method=method)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with self.opener.open(request, timeout=30) as response:
                status, raw = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read()
        if status == 401 and relogin and not path.startswith("/api/v1/auth/"):
            self.login()
            return self.call(method, path, body, relogin=False)
        try:
            return status, json.loads(raw) if raw else None
        except ValueError:
            return status, raw.decode(errors="replace")

    def expect(self, want: int, method: str, path: str, body: object = None) -> object:
        status, answer = self.call(method, path, body)
        if status != want:
            raise RunError(f"{method} {path} -> {status} (expected {want}): {str(answer)[:300]}")
        return answer

    def login(self) -> None:
        self.expect(200, "POST", "/api/v1/auth/login", {"password": self.password})


class Process:
    """Resident memory, open files and threads of one process."""

    def __init__(self, pid: int):
        self.pid = pid
        self.proc = Path(f"/proc/{pid}")
        self.psutil = None
        if not self.proc.is_dir():
            try:
                import psutil  # noqa: PLC0415 - only where /proc is missing
            except ImportError as error:
                raise RunError("sampling needs /proc or the psutil module") from error
            self.psutil = psutil.Process(pid)

    def sample(self) -> tuple[float, int, int]:
        if self.psutil is not None:
            files = (self.psutil.num_handles() if hasattr(self.psutil, "num_handles")
                     else self.psutil.num_fds())
            return self.psutil.memory_info().rss / MIB, files, self.psutil.num_threads()
        status = (self.proc / "status").read_text()
        rss = int(re.search(r"^VmRSS:\s+(\d+) kB", status, re.M)[1]) / 1024
        threads = int(re.search(r"^Threads:\s+(\d+)", status, re.M)[1])
        return rss, len(os.listdir(self.proc / "fd")), threads


class Samples(list):
    """The samples of a run, each written to the CSV as it is taken: a run cut off by a job
    timeout still leaves what it measured."""

    def __init__(self, path: Path):
        super().__init__()
        self.handle = open(path, "w", newline="")  # noqa: SIM115 - held for the run
        self.writer = csv.DictWriter(self.handle, fieldnames=soak_budgets.COLUMNS)
        self.writer.writeheader()

    def append(self, sample: dict[str, float]) -> None:
        super().append(sample)
        self.writer.writerow(sample)
        self.handle.flush()

    def close(self) -> None:
        self.handle.close()


def database_mib(database: Path) -> float:
    total = 0
    for suffix in ("", "-wal", "-shm"):
        path = Path(f"{database}{suffix}")
        if path.exists():
            total += path.stat().st_size
    return total / MIB


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(MIB):
            digest.update(chunk)
    return digest.hexdigest()
