"""What a soak run's samples come to, and whether that stays inside the budgets (RD-180-12).

A budget is `<metric>_max` or `<metric>_min` in the `[budgets]` table of
`scripts/soak-budgets.toml`; a `[budgets.<platform>]` table (`linux`, `windows`, `darwin`, as
`platform.system()` names them, lower-cased) overrides budgets for that platform alone. Every
budget, in every table, must name a metric this module knows and every override table a known
platform, so a typo is a refusal rather than a budget that never fails. A metric that should
have been measured and was not (too few samples for a window) fails its budget; a metric that
does not apply on this platform is left out of `metrics` and reported as skipped.

Growth is the mean of the last window minus the mean of the first window after the warm-up: a
leak shows as a level that keeps rising, not as the peak a busy moment reaches.
"""

from __future__ import annotations

import csv
import platform
from dataclasses import dataclass

# The columns `soak.py` writes, in order.
COLUMNS = [
    "elapsed_s", "rss_mib", "open_files", "threads", "db_mib",
    "completed", "failed", "corrupt", "bytes_mib", "in_flight",
]

METRICS = {
    "rss_mib_peak": "resident memory, highest sample (MiB)",
    "rss_mib_growth": "resident memory, last window over first window after warm-up (MiB)",
    "open_files_peak": "open file descriptors/handles, highest sample",
    "open_files_growth": "open file descriptors/handles, last window over first window",
    "threads_peak": "threads, highest sample",
    "threads_growth": "threads, last window over first window",
    "db_mib_peak": "database with its WAL and shared memory, largest sample (MiB)",
    "db_mib_per_hour": "database growth from the first to the last window, per hour (MiB/h)",
    "throughput_mib_s": "verified bytes completed per second over the whole run (MiB/s)",
    "throughput_decline_percent": "second half after warm-up slower than the first half (%)",
    "stall_seconds": "longest time without a completed download (s)",
    "failed": "downloads that ended failed, cancelled or blocked",
    "corrupt": "completed downloads whose bytes differ from what the fixture served",
    "shutdown_seconds": "time the service took to stop after SIGTERM (s)",
}

# The platforms a `[budgets.<platform>]` table may name: `platform.system()`, lower-cased.
PLATFORMS = ("linux", "windows", "darwin")

MIN_WINDOW_SAMPLES = 3


def current_platform() -> str:
    return platform.system().lower()


@dataclass
class Verdict:
    budget: str
    metric: str
    limit: float
    value: float | None
    passed: bool
    skipped: bool = False
    # The platform whose `[budgets.<platform>]` table set this limit; None for `[budgets]`.
    override: str | None = None

    def line(self) -> str:
        if self.skipped:
            return f"skip {self.metric}: not measured on this platform"
        sign = "<=" if self.budget.endswith("_max") else ">="
        shown = "not measured" if self.value is None else f"{self.value:.2f}"
        word = "ok  " if self.passed else "FAIL"
        source = f" for {self.override}" if self.override else ""
        return (f"{word} {self.metric} = {shown} (budget {sign} {self.limit:g}{source}; "
                f"{METRICS[self.metric]})")


def read_samples(path: str) -> list[dict[str, float]]:
    with open(path, newline="") as handle:
        return [{key: float(value) for key, value in row.items()} for row in csv.DictReader(handle)]


def _mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if len(values) >= MIN_WINDOW_SAMPLES else None


def derive(samples: list[dict[str, float]], warmup_percent: float,
           window_percent: float) -> dict[str, float | None]:
    """The metrics of one run. `failed`, `corrupt` and the peaks need no window."""
    if not samples:
        return {name: None for name in METRICS if name != "shutdown_seconds"}
    end = samples[-1]["elapsed_s"]
    warm = end * warmup_percent / 100
    window = end * window_percent / 100
    settled = [s for s in samples if s["elapsed_s"] >= warm]
    first = [s for s in settled if s["elapsed_s"] < warm + window]
    last = [s for s in settled if s["elapsed_s"] >= end - window]

    def growth(column: str) -> float | None:
        before = _mean([s[column] for s in first])
        after = _mean([s[column] for s in last])
        return None if before is None or after is None else after - before

    def per_hour(column: str) -> float | None:
        change = growth(column)
        if change is None:
            return None
        span = (sum(s["elapsed_s"] for s in last) / len(last)
                - sum(s["elapsed_s"] for s in first) / len(first))
        return change / span * 3600 if span > 0 else None

    metrics: dict[str, float | None] = {
        "rss_mib_peak": max(s["rss_mib"] for s in samples),
        "rss_mib_growth": growth("rss_mib"),
        "open_files_peak": max(s["open_files"] for s in samples),
        "open_files_growth": growth("open_files"),
        "threads_peak": max(s["threads"] for s in samples),
        "threads_growth": growth("threads"),
        "db_mib_peak": max(s["db_mib"] for s in samples),
        "db_mib_per_hour": per_hour("db_mib"),
        "throughput_mib_s": samples[-1]["bytes_mib"] / end if end > 0 else None,
        "failed": samples[-1]["failed"],
        "corrupt": samples[-1]["corrupt"],
    }

    # Throughput of the two halves after the warm-up, from the running byte count.
    decline = None
    if len(settled) >= 2 * MIN_WINDOW_SAMPLES:
        middle = settled[0]["elapsed_s"] + (end - settled[0]["elapsed_s"]) / 2
        mid = min(settled, key=lambda s: abs(s["elapsed_s"] - middle))

        def rate(a: dict[str, float], b: dict[str, float]) -> float:
            span = b["elapsed_s"] - a["elapsed_s"]
            return (b["bytes_mib"] - a["bytes_mib"]) / span if span > 0 else 0.0

        early, late = rate(settled[0], mid), rate(mid, settled[-1])
        decline = max(0.0, (early - late) / early * 100) if early > 0 else None
    metrics["throughput_decline_percent"] = decline

    # The longest stretch in which the completed count did not move, run start and end included.
    stall, since = 0.0, 0.0
    previous = 0.0
    for sample in samples:
        if sample["completed"] > previous:
            stall = max(stall, sample["elapsed_s"] - since)
            since, previous = sample["elapsed_s"], sample["completed"]
    metrics["stall_seconds"] = max(stall, end - since)
    return metrics


def _metric(budget: str) -> str:
    if not budget.endswith(("_max", "_min")):
        raise ValueError(f"budget {budget!r} ends in neither _max nor _min")
    metric = budget[:-4]
    if metric not in METRICS:
        raise ValueError(f"budget {budget!r} names no known metric ({', '.join(METRICS)})")
    return metric


def resolve(table: dict, platform_name: str) -> dict[str, tuple[float, str | None]]:
    """The budgets that apply on `platform_name`: `[budgets]` with the platform's own table
    merged over it, each limit with the platform that set it (None for `[budgets]`). Every key of
    every table is checked, so a typo in another platform's table is refused here too."""
    if platform_name not in PLATFORMS:
        raise ValueError(f"platform {platform_name!r} is none of {', '.join(PLATFORMS)}")
    budgets: dict[str, tuple[float, str | None]] = {}
    for key, value in table.items():
        if isinstance(value, dict):
            if key not in PLATFORMS:
                raise ValueError(
                    f"[budgets.{key}] names no known platform ({', '.join(PLATFORMS)})")
            for budget in value:
                _metric(budget)
        else:
            _metric(key)
            budgets[key] = (float(value), None)
    for budget, limit in table.get(platform_name, {}).items():
        budgets[budget] = (float(limit), platform_name)
    return budgets


def evaluate(metrics: dict[str, float | None], table: dict, platform_name: str) -> list[Verdict]:
    verdicts = []
    for budget, (limit, override) in resolve(table, platform_name).items():
        metric = _metric(budget)
        if metric not in metrics:
            verdicts.append(Verdict(budget, metric, limit, None, True, skipped=True,
                                    override=override))
            continue
        value = metrics[metric]
        if value is None:
            passed = False
        elif budget.endswith("_max"):
            passed = value <= limit
        else:
            passed = value >= limit
        verdicts.append(Verdict(budget, metric, limit, value, passed, override=override))
    return verdicts
