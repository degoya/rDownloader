#!/usr/bin/env bash
#
# The `rd-pins` merge driver for crates/rd-db/migrations.sha384 (RD-1100-13), which .gitattributes
# names and scripts/integrate.sh registers. Two branches that each pin a new migration both append
# a line at the end of the file, and git calls that a conflict every time; the answer is always
# both lines. The pins only ever grow (scripts/migration-pin.sh appends and never rewrites), so
# the result is every line of both sides, once, in migration order.
#
# One case is a real conflict and stays one: the same migration pinned with two different sums,
# which means an applied migration was edited. Then the file is merged as text, with conflict
# markers, for a person.
#
#   migration-pins.sh <base> <ours> <theirs> [<path>]    # git's %O %A %B %P
#
# Writes the result into <ours>; exit 0 when merged, 1 on a conflict.
set -euo pipefail

base="$1"
ours="$2"
theirs="$3"
path="${4:-crates/rd-db/migrations.sha384}"

merged="$(mktemp)"
trap 'rm -f "$merged"' EXIT
# By name, then sum: with both as keys, -u drops only lines that are equal in both.
cat "$ours" "$theirs" | grep -v '^[[:space:]]*$' | LC_ALL=C sort -b -u -k2,2 -k1,1 > "$merged" || true

twice="$(awk '{ print $2 }' "$merged" | uniq -d)"
if [[ -n "$twice" ]]; then
    echo "rd-pins: $path pins $(tr '\n' ' ' <<< "$twice")with two different sums; merged as text" >&2
    git merge-file -L ours -L base -L theirs "$ours" "$base" "$theirs" || true
    exit 1
fi
cat "$merged" > "$ours"
