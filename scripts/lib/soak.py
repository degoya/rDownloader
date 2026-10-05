#!/usr/bin/env python3
"""The soak run (RD-180-12): a real `rdownloader serve` downloading for hours from a local fixture
that keeps dropping out, sampled all along and judged against `scripts/soak-budgets.toml`.

    soak.py run --binary PATH [--duration 5m] [--budgets FILE] [--out DIR]
    soak.py evaluate --samples samples.csv [--budgets FILE] [--shutdown-seconds S] [--platform P]
    soak.py serve-fixture [--port N] [--rate-mib R] [--outage-every S --outage-for S]

`run` starts the service with a throwaway data directory, signs in, keeps `queue_depth` downloads
queued, verifies every completed file against the bytes the fixture served, removes it again, and
samples the process every `sample_seconds`: resident memory, open files, threads, database size
with WAL, completed bytes. It writes `samples.csv`, `summary.json` and `server.log` to `--out` and
exits 1 naming each exceeded budget, 2 when the run itself could not be carried out. `evaluate`
judges a samples file again (the self-test, and a budget changed after a run); `--platform` picks
the `[budgets.<platform>]` table to apply, by default this machine's (a Windows run's samples are
judged on Linux with `--platform windows`).

Standard library only (Python 3.11 for tomllib). On Linux the process is read from /proc; on
other systems `psutil` is required.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import http.cookiejar
import json
import os
import platform
import random
import re
import secrets
import shutil
import socket
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

# The two modules beside this one would otherwise leave a __pycache__ in the checkout.
sys.dont_write_bytecode = True
import soak_budgets  # noqa: E402
import soak_fixture  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BUDGETS = ROOT / "scripts" / "soak-budgets.toml"
MIB = 1024 * 1024
TERMINAL_BAD = {"failed", "cancelled", "blocked"}


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


def run(args: argparse.Namespace) -> int:
    config = tomllib.loads(Path(args.budgets).read_text())
    settings, fixture_config = config["run"], config["fixture"]
    duration = args.duration or parse_duration(str(settings["duration"]))
    sample_every = float(settings["sample_seconds"])
    queue_depth = int(settings["queue_depth"])
    sizes = [int(size * MIB) for size in fixture_config["sizes_mib"]]
    # A budget typo is refused before the run, not after two hours of it.
    try:
        soak_budgets.resolve(config["budgets"], soak_budgets.current_platform())
    except ValueError as error:
        print(f"!! {error}", file=sys.stderr)
        return 2
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    # The throwaway data directory sits next to the results, so a run writes nowhere else.
    work = out / "data"
    if work.exists():
        raise SystemExit(f"{work} exists already; give each run its own --out")
    downloads = work / "downloads"
    database = work / "rdownloader.sqlite3"
    work.mkdir()
    port = free_port()
    password = secrets.token_urlsafe(18)
    fixture = soak_fixture.Fixture(
        rate_mib=float(fixture_config["rate_mib"]),
        outage_every=float(fixture_config["outage_every_seconds"]),
        outage_for=float(fixture_config["outage_for_seconds"])).start()
    env = {key: value for key, value in os.environ.items() if not key.startswith("RDOWNLOADER_")}
    log = open(out / "server.log", "wb")  # noqa: SIM115 - held for the service's lifetime
    service = subprocess.Popen(
        [str(Path(args.binary).resolve()), "serve", "--database", str(database),
         "--downloads", str(downloads), "--listen", f"127.0.0.1:{port}",
         "--plugin-root", str(work / "plugins")],
        cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT)
    print(f"==> soak: {duration:.0f} s against {fixture.base}, service pid {service.pid}, "
          f"out {out}", flush=True)
    api = Api(f"http://127.0.0.1:{port}", password)
    samples = Samples(out / "samples.csv")
    failures: list[str] = []
    broken: Exception | None = None
    try:
        drive(api, service, fixture, downloads, database, sizes, duration, sample_every,
              queue_depth, samples, failures)
    except (RunError, OSError) as error:
        # OSError: the API stopped answering (refused, reset, timed out) mid-run.
        broken = error
    finally:
        shutdown_seconds = stop_service(service)
        fixture.stop()
        log.close()
        samples.close()
    if broken is not None:
        print(f"!! the soak run could not be carried out: {broken}", file=sys.stderr)
        tail(out / "server.log")
        print(f"    data directory kept for inspection: {work}", file=sys.stderr)
        return 2
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)
    return judge(samples, config, shutdown_seconds, fixture.stats, failures, out)


def drive(api, service, fixture, downloads, database, sizes, duration, sample_every,
          queue_depth, samples, failures) -> None:
    deadline = time.monotonic() + 120
    while True:
        if service.poll() is not None:
            raise RunError(f"the service exited during startup with {service.returncode}")
        try:
            if api.call("GET", "/api/v1/health")[0] == 200:
                break
        except OSError:
            pass
        if time.monotonic() > deadline:
            raise RunError("the service never answered /api/v1/health")
        time.sleep(0.5)
    api.expect(200, "POST", "/api/v1/auth/setup", {"password": api.password})
    api.login()
    process = Process(service.pid)

    ours: dict[str, tuple[int, int]] = {}  # download id -> (seed, size)
    counts = {"completed": 0, "failed": 0, "corrupt": 0, "bytes": 0, "next": 0}
    started = time.monotonic()
    next_sample = started
    choose = random.Random(180_12)
    while (now := time.monotonic()) - started < duration:
        if service.poll() is not None:
            raise RunError(f"the service exited during the run with {service.returncode}")
        listing = api.expect(200, "GET", "/api/v1/downloads")
        for file in listing:
            if file["id"] not in ours:
                continue
            state = file["state"]
            if state == "completed":
                seed, size = ours.pop(file["id"])
                verify(file, seed, size, downloads, counts, failures)
                remove(api, file, downloads)
            elif state in TERMINAL_BAD:
                ours.pop(file["id"])
                counts["failed"] += 1
                failures.append(f"{file['file_name']} ended {state}: {file.get('last_error')}")
                remove(api, file, downloads)
        while len(ours) < queue_depth:
            counts["next"] += 1
            seed, size = counts["next"], choose.choice(sizes)
            name = f"soak-{seed}.bin"
            created = api.expect(201, "POST", "/api/v1/downloads", {
                "url": f"{fixture.base}/f/{seed}/{size}/{name}", "package_name": f"soak-{seed}"})
            ours[created["id"]] = (seed, size)
        if now >= next_sample:
            rss, files, threads = process.sample()
            samples.append({
                "elapsed_s": round(now - started, 1), "rss_mib": round(rss, 2),
                "open_files": files, "threads": threads,
                "db_mib": round(database_mib(database), 3),
                "completed": counts["completed"], "failed": counts["failed"],
                "corrupt": counts["corrupt"], "bytes_mib": round(counts["bytes"] / MIB, 3),
                "in_flight": len(ours)})
            next_sample += sample_every
            if len(samples) % max(1, int(60 / sample_every)) == 0:
                print(f"    {samples[-1]['elapsed_s']:>7.0f} s  rss {rss:7.1f} MiB  files {files:4d}"
                      f"  threads {threads:3d}  db {samples[-1]['db_mib']:6.2f} MiB"
                      f"  done {counts['completed']:5d}  failed {counts['failed']}", flush=True)
        time.sleep(1)


def verify(file, seed, size, downloads, counts, failures) -> None:
    found = [path for path in downloads.rglob(file["file_name"]) if path.is_file()]
    if len(found) != 1:
        counts["corrupt"] += 1
        failures.append(f"{file['file_name']} completed, but {len(found)} files carry its name")
        return
    actual = found[0].stat().st_size
    if actual != size or file_sha256(found[0]) != soak_fixture.expected_sha256(seed, size):
        counts["corrupt"] += 1
        failures.append(f"{file['file_name']} completed with bytes the fixture never served"
                        f" ({actual} of {size} bytes)")
        return
    counts["completed"] += 1
    counts["bytes"] += size


def remove(api, file, downloads) -> None:
    status, answer = api.call("DELETE", f"/api/v1/packages/{file['package_id']}?force=true")
    if status != 200:
        raise RunError(f"removing package {file['package_id']} -> {status}: {answer}")
    for path in downloads.rglob(file["file_name"]):
        folder = path.parent
        path.unlink(missing_ok=True)
        if folder != downloads and not any(folder.iterdir()):
            folder.rmdir()


def stop_service(service: subprocess.Popen) -> float | None:
    """Seconds the service took to stop after SIGTERM; None where there is no SIGTERM."""
    if service.poll() is not None:
        return None
    started = time.monotonic()
    service.terminate()
    try:
        service.wait(timeout=120)
    except subprocess.TimeoutExpired:
        service.kill()
        service.wait()
        return 120.0
    # Only POSIX has a SIGTERM to measure; elsewhere terminate() is a hard kill.
    return time.monotonic() - started if os.name == "posix" else None


def tail(path: Path, lines: int = 40) -> None:
    text = path.read_text(errors="replace").splitlines()[-lines:]
    print("--- server log (last lines) ---", *text, sep="\n", file=sys.stderr)


def judge(samples, config, shutdown_seconds, fixture_stats, failures, out: Path) -> int:
    evaluation = config["evaluation"]
    metrics = soak_budgets.derive(samples, float(evaluation["warmup_percent"]),
                                  float(evaluation["window_percent"]))
    if shutdown_seconds is not None:
        metrics["shutdown_seconds"] = shutdown_seconds
    budgets_for = soak_budgets.current_platform()
    verdicts = soak_budgets.evaluate(metrics, config["budgets"], budgets_for)
    summary = {
        "platform": f"{platform.system()} {platform.machine()}",
        "budgets_for": budgets_for,
        "samples": len(samples),
        "duration_s": samples[-1]["elapsed_s"] if samples else 0,
        "fixture": fixture_stats,
        "metrics": metrics,
        "verdicts": [vars(verdict) for verdict in verdicts],
        "failures": failures[:50],
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return report(verdicts, failures, budgets_for, fixture_stats)


def report(verdicts, failures, budgets_for: str, fixture_stats=None) -> int:
    print()
    if fixture_stats:
        print(f"    fixture: {fixture_stats['requests']} requests, {fixture_stats['ranged']} resumed"
              f" with a range, {fixture_stats['outages']} outages, {fixture_stats['cut']} cut off")
    for verdict in verdicts:
        print(f"    {verdict.line()}")
    for failure in failures[:10]:
        print(f"    - {failure}")
    broken = [verdict.metric for verdict in verdicts if not verdict.passed]
    if broken:
        print(f"==> soak failed: over budget: {', '.join(broken)} ({budgets_for} budgets)",
              file=sys.stderr)
        return 1
    print(f"==> soak passed: every budget held ({budgets_for} budgets)")
    return 0


def evaluate(args: argparse.Namespace) -> int:
    config = tomllib.loads(Path(args.budgets).read_text())
    samples = soak_budgets.read_samples(args.samples)
    evaluation = config["evaluation"]
    metrics = soak_budgets.derive(samples, float(evaluation["warmup_percent"]),
                                  float(evaluation["window_percent"]))
    if args.shutdown_seconds is not None:
        metrics["shutdown_seconds"] = args.shutdown_seconds
    try:
        verdicts = soak_budgets.evaluate(metrics, config["budgets"], args.platform)
    except ValueError as error:
        print(f"!! {error}", file=sys.stderr)
        return 2
    return report(verdicts, [], args.platform)


def serve_fixture(args: argparse.Namespace) -> int:
    fixture = soak_fixture.Fixture(args.port, args.rate_mib, args.outage_every,
                                   args.outage_for).start()
    print(fixture.base, flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        fixture.stop()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    run_parser = commands.add_parser("run")
    run_parser.add_argument("--binary", required=True)
    run_parser.add_argument("--duration", type=parse_duration)
    run_parser.add_argument("--budgets", default=str(DEFAULT_BUDGETS))
    run_parser.add_argument("--out", required=True)
    run_parser.add_argument("--keep", action="store_true", help="keep the data dir under --out")
    evaluate_parser = commands.add_parser("evaluate")
    evaluate_parser.add_argument("--samples", required=True)
    evaluate_parser.add_argument("--budgets", default=str(DEFAULT_BUDGETS))
    evaluate_parser.add_argument("--shutdown-seconds", type=float)
    evaluate_parser.add_argument("--platform", default=soak_budgets.current_platform(),
                                 help="whose [budgets.<platform>] table applies")
    fixture_parser = commands.add_parser("serve-fixture")
    fixture_parser.add_argument("--port", type=int, default=0)
    fixture_parser.add_argument("--rate-mib", type=float, default=0)
    fixture_parser.add_argument("--outage-every", type=float, default=0)
    fixture_parser.add_argument("--outage-for", type=float, default=0)
    args = parser.parse_args()
    return {"run": run, "evaluate": evaluate, "serve-fixture": serve_fixture}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
