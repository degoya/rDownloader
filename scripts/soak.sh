#!/usr/bin/env bash
#
# A soak run of a built rdownloader against a local fixture that keeps dropping out (RD-180-12).
#
# The service runs from a throwaway data directory for the given time, keeps a queue of HTTP
# downloads going and has every transfer cut off every two minutes; memory, open files, threads,
# database size and verified throughput are sampled and judged against scripts/soak-budgets.toml.
# Exit 1 names each exceeded budget; exit 2 means the run itself could not be carried out.
# The nightly workflow .github/workflows/soak.yml runs the same for two hours.
#
# It builds nothing: it takes the binary it is given, else artifacts/linux/rdownloader, else
# target/release/rdownloader. A debug binary works, but its memory is not what the budgets are for.
#
# Usage:
#   scripts/soak.sh [--duration 5m] [--binary PATH] [--out DIR] [--budgets FILE] [--keep]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary=""
out=""
args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) binary="${2:?--binary needs a path}"; shift 2 ;;
        --out) out="${2:?--out needs a directory}"; shift 2 ;;
        --duration | --budgets) args+=("$1" "${2:?$1 needs a value}"); shift 2 ;;
        --keep) args+=("$1"); shift ;;
        -h | --help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
    esac
done

if [[ -z "$binary" ]]; then
    for candidate in "$ROOT/artifacts/linux/rdownloader" "$ROOT/target/release/rdownloader"; do
        [[ -x "$candidate" ]] && { binary="$candidate"; break; }
    done
fi
[[ -n "$binary" && -x "$binary" ]] || {
    echo "no rdownloader binary: pass --binary, or build one (scripts/package-linux.sh)" >&2
    exit 2
}
out="${out:-$ROOT/artifacts/soak/$(date +%Y%m%d-%H%M%S)}"

exec python3 "$ROOT/scripts/lib/soak.py" run --binary "$binary" --out "$out" "${args[@]}"
