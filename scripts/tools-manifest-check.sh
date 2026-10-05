#!/usr/bin/env bash
#
# Every download the tool manifest names still answers, with the size the manifest declares
# (RD-1101-13). The manifest is signed and compiled into the release, so a URL that dies after
# the release breaks the managed install of that tool for every user until the next one; the
# 1.10.0 manifest pinned BtbN daily builds that were gone three weeks later. This asks each
# distinct URL with a HEAD request (redirects followed, nothing downloaded) and compares the
# final Content-Length with the manifest's `size`. The hash is not checked here — that needs the
# bytes, and the installer checks it on every install anyway; a size that changed is the cheap
# sign of a re-uploaded asset. `.github/workflows/tools-manifest.yml` runs it weekly.
#
# Usage:
#   scripts/tools-manifest-check.sh                  # the embedded manifest
#   scripts/tools-manifest-check.sh <manifest.json>  # a signed document or a bare payload
#   scripts/tools-manifest-check.sh --list [file]    # print `url<TAB>size`, no network
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

list_only=0
if [[ "${1:-}" == "--list" ]]; then
    list_only=1
    shift
fi
manifest="${1:-$ROOT/crates/rd-tools/resources/tools-manifest.json}"
[[ -f "$manifest" ]] || { echo "no manifest at $manifest" >&2; exit 2; }

# One line per distinct URL; ffmpeg and ffprobe share an archive and are asked once.
entries="$(python3 - "$manifest" <<'EOF'
import json, sys
document = json.load(open(sys.argv[1], encoding="utf-8"))
payload = document.get("payload", document)
seen = {}
for tool in payload["tools"]:
    if seen.setdefault(tool["url"], tool["size"]) != tool["size"]:
        sys.exit(f"{tool['url']} is named with two different sizes")
for url, size in seen.items():
    print(f"{url}\t{size}")
EOF
)"
[[ -n "$entries" ]] || { echo "the manifest at $manifest names no download" >&2; exit 2; }

if [[ "$list_only" -eq 1 ]]; then
    printf '%s\n' "$entries"
    exit 0
fi

headers="$(mktemp)"
trap 'rm -f "$headers"' EXIT

checked=0
dead=0
while IFS=$'\t' read -r url size; do
    checked=$((checked + 1))
    code="$(curl --silent --head --location --retry 2 --max-time 60 \
        --output /dev/null --dump-header "$headers" --write-out '%{http_code}' "$url" 2>/dev/null)" || true
    if [[ "$code" != "200" ]]; then
        [[ "$code" =~ ^[1-9] ]] && code="HTTP $code" || code="no answer"
        echo "DEAD $url: $code"
        dead=$((dead + 1))
        continue
    fi
    # The last response's length: a redirect's own `content-length: 0` comes first.
    length="$(tr -d '\r' < "$headers" | awk 'tolower($1) == "content-length:" { n = $2 } END { print n }')"
    if [[ "$length" != "$size" ]]; then
        echo "SIZE $url: ${length:-no Content-Length}, the manifest says $size"
        dead=$((dead + 1))
        continue
    fi
    echo "ok   $url ($size bytes)"
done <<< "$entries"

echo
if [[ "$dead" -gt 0 ]]; then
    echo "$dead of $checked download(s) in $manifest no longer answer as the manifest says"
    exit 1
fi
echo "$checked download(s) in $manifest answer with their declared size"
