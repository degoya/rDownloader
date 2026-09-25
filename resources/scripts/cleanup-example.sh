#!/bin/sh
# Example rDownloader post-processing script.
#
# Copy this file into your scripts directory (Settings -> Post-processing, by default
# `scripts/` next to the database), make it executable (chmod +x), then select it as the
# script of a package or a category.
#
# Positional arguments (SABnzbd-compatible):
#   $1 final directory   $2 package name   $3 clean package name   $4 (empty)
#   $5 category          $6 (empty)        $7 status: 0 ok, 1 download, 2 unpack, 3 par2
# The same values are available as RD_* / SAB_* environment variables.

set -eu

status="${7:-${RD_STATUS:-0}}"
final_dir="${1:-$RD_FINAL_DIR}"

# Leave incomplete packages untouched; a failed unpack still has archives lying around.
if [ "$status" != "0" ]; then
    echo "skipping $RD_CLEAN_NAME (status $status)"
    exit 0
fi

removed=0
for leftover in "$final_dir"/*.url "$final_dir"/*.txt; do
    [ -f "$leftover" ] || continue
    rm -f "$leftover"
    removed=$((removed + 1))
done

echo "cleanup done for $RD_CLEAN_NAME in $final_dir ($removed file(s) removed)"
