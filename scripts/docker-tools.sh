#!/usr/bin/env bash
#
# The Python tools of the container image, pinned with their hashes (RD-191-09 T17). Until then
# docker/Dockerfile named four versions and pip resolved every dependency at build time, unhashed:
# two builds of one tag could ship different code, and a replaced file on PyPI would not be
# noticed. docker/requirements.in names the tools; docker/requirements.txt is compiled from it by
# uv for every platform (`--universal`, so amd64 and arm64 alike) with every dependency and every
# file's hash, and the image installs it with `pip --require-hashes`.
#
# Usage:
#   scripts/docker-tools.sh              # check: requirements.txt is the compile of requirements.in
#   scripts/docker-tools.sh --lock       # compile requirements.txt from requirements.in (needs uv)
#   scripts/docker-tools.sh --bump       # every tool to its newest PyPI release, then --lock
#
# The check reads files only and is one of check.sh's file checks (scripts/lib/preflight.sh,
# PIPE-05), so every run and the preflight hold it; --lock and --bump need the network.
# Dependabot's `docker` updates move the base images; these tools move with --bump, which is worth
# a run before every release (CHANGELOG line: the versions it moved).
#
# DOCKER_TOOLS_DIR names another directory holding the two files, for the test.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="${DOCKER_TOOLS_DIR:-$ROOT/docker}"
IN="$DIR/requirements.in"
OUT="$DIR/requirements.txt"
# The image's Python: debian:trixie-slim's python3.
PYTHON_VERSION=3.13
HEADER="# Compiled from docker/requirements.in by scripts/docker-tools.sh --lock (uv pip compile --universal
# --generate-hashes --python-version $PYTHON_VERSION). Never edited by hand: change requirements.in."

# The `name==version` lines of requirements.in, comments and blanks removed.
tools() {
    grep -vE '^[[:space:]]*(#|$)' "$IN"
}

check() {
    local problems=() line name
    [[ -f "$OUT" ]] || { echo "!! $OUT is missing: scripts/docker-tools.sh --lock" >&2; return 1; }
    [[ "$(head -n 2 "$OUT")" == "$HEADER" ]] || problems+=("the header is not the one --lock writes")
    while read -r line; do
        [[ "$line" =~ ^[A-Za-z0-9._-]+==[^[:space:]]+$ ]] || { problems+=("not name==version: $line"); continue; }
        grep -qixF "$line \\" "$OUT" || problems+=("$line is not in requirements.txt")
    done < <(tools)
    # Every requirement line is followed by its hashes; pip --require-hashes refuses the file
    # otherwise, but only at the image build.
    while read -r name; do
        problems+=("no hash for $name")
    done < <(awk '/^[A-Za-z0-9]/ { if (pending) print pending; pending = $1; next }
                  /--hash=sha256:/ { pending = "" }
                  END { if (pending) print pending }' "$OUT")
    if [[ ${#problems[@]} -gt 0 ]]; then
        printf '!! %s\n' "${problems[@]}" >&2
        echo "   scripts/docker-tools.sh --lock compiles $OUT again." >&2
        return 1
    fi
    echo "==> $(tools | wc -l) tools, $(grep -cE '^[A-Za-z0-9]' "$OUT") packages, every one hashed"
}

lock() {
    command -v uv > /dev/null || { echo "uv is required: https://docs.astral.sh/uv/" >&2; exit 1; }
    local body
    body="$(mktemp)"
    uv pip compile --universal --generate-hashes --python-version "$PYTHON_VERSION" \
        --no-header --quiet "$IN" -o "$body"
    { printf '%s\n' "$HEADER"; sed "s|$IN|docker/requirements.in|" "$body"; } > "$OUT"
    rm -f "$body"
    echo "==> compiled $OUT"
}

bump() {
    local line name old new
    while read -r line; do
        name="${line%%==*}"
        old="${line#*==}"
        new="$(curl -fsS "https://pypi.org/pypi/$name/json" \
            | python3 -c 'import json, sys; print(json.load(sys.stdin)["info"]["version"])')"
        [[ "$new" == "$old" ]] && continue
        echo "    $name $old -> $new"
        sed -i "s/^$name==$old\$/$name==$new/" "$IN"
    done < <(tools)
}

case "${1:-}" in
    "") check ;;
    --lock) lock; check ;;
    --bump) bump; lock; check ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
esac
