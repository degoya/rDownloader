#!/usr/bin/env bash
#
# The soak harness without the service (RD-180-12): the budget evaluation of scripts/lib/soak.py
# on a samples file written here, and the fixture it downloads from.
#
# A steady run passes the checked-in budgets; the same samples fail once a budget is lowered below
# them, a leak fails the growth budget, a stall and a corrupt file fail theirs, and each failure
# names its metric. A budget key naming no metric is refused. A `[budgets.windows]` override
# applies on Windows alone and is named in the verdict; a typo in it, or a table naming no
# platform, is refused on every platform, and the growth budget still catches a handle leak on
# Windows. The
# fixture serves ranges that match its own hash and refuses connections during an outage.
#
# Pure python3 and bash: it runs in seconds. check.sh runs it when scripts/ changes, and under
# --full.
#
#   scripts/tests/soak.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOAK="$ROOT/scripts/lib/soak.py"
BUDGETS="$ROOT/scripts/soak-budgets.toml"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# The budgets of `$platform` (linux unless the call sets it), whatever machine runs this.
# shellcheck disable=SC2034  # `status` is read inside the eval of expect()
judge() { python3 "$SOAK" evaluate --samples "$1" --budgets "${2:-$BUDGETS}" --shutdown-seconds 2 \
    --platform "${platform:-linux}" > "$SCRATCH/out" 2>&1 && status=0 || status=$?; }
has() { grep -qF -- "$1" "$SCRATCH/out"; }
# The checked-in budgets with one key replaced.
lowered() { sed -E "s/^$1 = .*/$1 = $2/" "$BUDGETS" > "$SCRATCH/lowered.toml"; }

# An hour sampled every 10 s: flat memory, files and threads, 12 MiB/s, one completion every 5 s.
# `leak` adds 1 MiB of resident memory per minute, `stall` stops completions for 10 minutes and
# `corrupt` counts one bad file at the end. `handles` is the Windows run of 2026-10-05: a handle
# count peaking at 522 and falling; `handle-leak` adds one handle a minute.
samples() {
    python3 - "$1" > "$SCRATCH/$1.csv" <<'PY'
import sys
shape = sys.argv[1]
print("elapsed_s,rss_mib,open_files,threads,db_mib,completed,failed,corrupt,bytes_mib,in_flight")
completed = 0
for step in range(361):
    t = step * 10
    rss = 120 + (t / 60 if shape == "leak" else 0) + (step % 3)
    if not (shape == "stall" and 1800 <= t < 2400):
        completed = step * 2
    corrupt = 1 if shape == "corrupt" and step == 360 else 0
    files = 40 + step % 2
    if shape == "handles":
        files = 522 - step / 4
    elif shape == "handle-leak":
        files = 400 + t / 60
    print(f"{t},{rss},{files},{30},{4 + t / 3600},{completed},0,{corrupt},{completed * 60},12")
PY
}

for shape in steady leak stall corrupt handles handle-leak; do samples "$shape"; done

judge "$SCRATCH/steady.csv"
expect_true "a steady run passes the checked-in budgets" '[[ $status -eq 0 ]] && has "soak passed"'

lowered rss_mib_peak_max 100
judge "$SCRATCH/steady.csv" "$SCRATCH/lowered.toml"
expect_true "a lowered memory budget fails the same run and names it" \
    '[[ $status -eq 1 ]] && has "FAIL rss_mib_peak" && has "over budget: rss_mib_peak"'

lowered throughput_mib_s_min 20
judge "$SCRATCH/steady.csv" "$SCRATCH/lowered.toml"
expect_true "a raised throughput floor fails the same run and names it" \
    '[[ $status -eq 1 ]] && has "over budget: throughput_mib_s"'

lowered shutdown_seconds_max 1
judge "$SCRATCH/steady.csv" "$SCRATCH/lowered.toml"
expect_true "a slow shutdown fails its budget" '[[ $status -eq 1 ]] && has "over budget: shutdown_seconds"'

judge "$SCRATCH/leak.csv"
expect_true "a leak of 1 MiB a minute fails the growth budget, not the peak" \
    '[[ $status -eq 1 ]] && has "FAIL rss_mib_growth" && has "ok   rss_mib_peak"'

judge "$SCRATCH/stall.csv"
expect_true "ten minutes without a completion fail the stall budget" \
    '[[ $status -eq 1 ]] && has "over budget: stall_seconds"'

judge "$SCRATCH/corrupt.csv"
expect_true "one corrupt file fails the run" '[[ $status -eq 1 ]] && has "over budget: corrupt"'

head -4 "$SCRATCH/steady.csv" > "$SCRATCH/short.csv"
judge "$SCRATCH/short.csv"
expect_true "a run too short for its windows fails as not measured" \
    '[[ $status -eq 1 ]] && has "rss_mib_growth = not measured"'

sed 's/^\[budgets\]$/&\nrss_mib_peek_max = 1/' "$BUDGETS" > "$SCRATCH/typo.toml"
judge "$SCRATCH/steady.csv" "$SCRATCH/typo.toml"
expect_true "a budget naming no metric is refused" '[[ $status -eq 2 ]] && has "rss_mib_peek_max"'

platform=windows judge "$SCRATCH/handles.csv"
expect_true "522 handles on Windows pass the Windows peak, and the verdict names it" \
    '[[ $status -eq 0 ]] && has "ok   open_files_peak = 522.00 (budget <= 1024 for windows;" \
        && has "ok   rss_mib_peak = " && has "(budget <= 256;" && has "(windows budgets)"'

judge "$SCRATCH/handles.csv"
expect_true "the same samples fail the Linux peak, which has no override" \
    '[[ $status -eq 1 ]] && has "FAIL open_files_peak = 522.00 (budget <= 256;" \
        && has "over budget: open_files_peak (linux budgets)"'

platform=windows judge "$SCRATCH/handle-leak.csv"
expect_true "a handle leak on Windows still fails the shared growth budget" \
    '[[ $status -eq 1 ]] && has "FAIL open_files_growth" && has "ok   open_files_peak"'

printf '%s\nopen_files_peek_max = 1\n' "$(cat "$BUDGETS")" > "$SCRATCH/override-typo.toml"
judge "$SCRATCH/steady.csv" "$SCRATCH/override-typo.toml"
expect_true "a typo in the Windows table is refused on Linux too" \
    '[[ $status -eq 2 ]] && has "open_files_peek_max"'

printf '%s\n[budgets.windwos]\nopen_files_peak_max = 1\n' "$(cat "$BUDGETS")" \
    > "$SCRATCH/platform-typo.toml"
platform=windows judge "$SCRATCH/steady.csv" "$SCRATCH/platform-typo.toml"
expect_true "an override table naming no platform is refused" \
    '[[ $status -eq 2 ]] && has "[budgets.windwos] names no known platform"'

# The fixture: a range matches its own hash, and an outage refuses connections.
if python3 -B - "$ROOT/scripts/lib" > "$SCRATCH/out" 2>&1 <<'PY'
import hashlib, sys, urllib.error, urllib.request
sys.path.insert(0, sys.argv[1])
import soak_fixture
fixture = soak_fixture.Fixture().start()
size = 5 * 1024 * 1024 + 3
url = f"{fixture.base}/f/7/{size}/probe.bin"
whole = urllib.request.urlopen(url).read()
assert hashlib.sha256(whole).hexdigest() == soak_fixture.expected_sha256(7, size), "hash"
tail = urllib.request.urlopen(urllib.request.Request(url, headers={"Range": "bytes=4194310-"}))
assert tail.status == 206 and tail.read() == whole[4194310:], "range"
stale = urllib.request.Request(url, headers={"Range": "bytes=10-", "If-Range": '"other"'})
assert urllib.request.urlopen(stale).status == 200, "if-range"
assert soak_fixture.content(7, 0, 64) != soak_fixture.content(8, 0, 64), "seed"
fixture.go_down()
try:
    urllib.request.urlopen(url, timeout=2)
    raise AssertionError("served during an outage")
except urllib.error.URLError:
    pass
fixture.come_back()
assert urllib.request.urlopen(url).read() == whole, "after the outage"
fixture.stop()
PY
then ok "the fixture serves ranges, honours If-Range and goes away during an outage"
else fail "the fixture: $(tail -3 "$SCRATCH/out")"; fi

finish_tests "soak harness"
