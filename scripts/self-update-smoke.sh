#!/usr/bin/env bash
#
# The self-update's process half with real binaries (RD-180-02), on Linux and on Windows (Git Bash):
# `rdownloader apply-update` stops a running service, switches the portable program folder, starts
# the new version and waits for its health route, or takes the switch back.
#
# Usage:
#   scripts/self-update-smoke.sh <old-binary> <old-version> <new-binary> <new-version>
#
# Two builds of this tree, the second with a raised version (.github/workflows/self-update.yml
# builds them). The old one runs as a portable installation in a scratch folder; three updates
# are handed to it the way the service hands one over — the backup before the update through the
# local control token, the journal in <data>/update/, the updater run from a copy of the running
# executable:
#
#   1. the new version: exit 0, it answers with its version, the old files wait in .previous/ --
#      with the web interface's and a capture agent's event streams open across the stop, which
#      must not hold the old version up (live test 2026-10-01: an open stream kept it running
#      until the updater gave up), nor need the updater's stop by force;
#   2. a candidate whose program ends at once: exit 2, rolled back to what ran before, with
#      update.new_version_exited, and the database copy from before the update back in place;
#   3. only with a debug build: a candidate that answers but is declared unhealthy
#      (RD_UPDATE_TEST_FAIL_HEALTH): exit 2, stopped and rolled back.
#
# The manifest, the download and the hand-over by the service are rd-api's `admin::updates`
# suite; the file switch and its crash points are rd-update's tests.
set -euo pipefail

[[ $# -eq 4 ]] || {
    echo "usage: scripts/self-update-smoke.sh <old-binary> <old-version> <new-binary> <new-version>" >&2
    exit 2
}
OLD_BINARY="$1" OLD_VERSION="$2" NEW_BINARY="$3" NEW_VERSION="$4"
PORT="${RD_SMOKE_PORT:-18731}"
EXT=""
[[ "$OLD_BINARY" == *.exe ]] && EXT=".exe"
# On Windows `python3` may be the Store's placeholder; the runner's Python is `python`.
if [[ -n "$EXT" ]]; then PY="$(command -v python)"; else PY="$(command -v python3 || command -v python)"; fi
WORK="$(mktemp -d)"
INSTALL="$WORK/install"
DATA="$INSTALL/data"
LOGS="${RD_SMOKE_LOGS:-$WORK/logs}"
mkdir -p "$INSTALL/plugins" "$LOGS"

# A path the Rust binary understands: Git Bash's /d/a/… is D:/a/… on Windows.
native() {
    if command -v cygpath > /dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

fail() {
    echo "!! $*" >&2
    for pid in ${STREAM_PIDS:-}; do kill "$pid" 2> /dev/null || true; done
    for log in "$DATA/update/updater.log" "$INSTALL/logs/rdownloader.err.log" "$DATA/update/journal.json"; do
        [[ -f "$log" ]] && { echo "--- $log" >&2; tail -n 60 "$log" >&2; cp "$log" "$LOGS/" 2>/dev/null || true; }
    done
    exit 1
}

health_version() {
    curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/v1/health" 2>/dev/null \
        | "$PY" -c 'import json, sys; print(json.load(sys.stdin)["version"])' 2>/dev/null || true
}

await_version() {
    local want="$1" seen=""
    for _ in $(seq 1 120); do
        seen="$(health_version)"
        [[ "$seen" == "$want" ]] && return 0
        sleep 1
    done
    fail "the service answers as '${seen:-nothing}', not $want"
}

journal_field() {
    "$PY" -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]] or "")' \
        "$DATA/update/journal.json" "$1"
}

# A candidate archive as the release names it, flat, from a folder.
pack() {
    local folder="$1" archive="$2"
    "$PY" - "$folder" "$archive" <<'PY'
import os, sys, tarfile, zipfile
folder, archive = sys.argv[1], sys.argv[2]
names = sorted(os.listdir(folder))
if archive.endswith(".zip"):
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as out:
        for name in names:
            out.write(os.path.join(folder, name), name)
else:
    with tarfile.open(archive, "w:gz") as out:
        for name in names:
            out.add(os.path.join(folder, name), name)
PY
}

candidate() {
    local name="$1" program="$2" version="$3" readme="$4" folder="$WORK/$1"
    mkdir -p "$folder/plugins"
    cp "$program" "$folder/rdownloader$EXT"
    printf '%s\n' "$version" > "$folder/VERSION.txt"
    printf '%s\n' "$readme" > "$folder/README.md"
    printf 'licence\n' > "$folder/LICENSE"
    local archive="$WORK/rdownloader-smoke-$name.tar.gz"
    [[ -n "$EXT" ]] && archive="$WORK/rdownloader-smoke-$name.zip"
    pack "$folder" "$archive"
    printf '%s\n' "$archive"
}

# What the service does before it hands over: the backup through the local control token, the
# journal, and the updater as a copy of the running program. Prints the updater's path.
hand_over() {
    local from="$1" target="$2" archive="$3" running="$4"
    local control="$DATA/local-control.json"
    [[ -f "$control" ]] || fail "no local control file; is the service running?"
    local token pid copy
    token="$("$PY" -c 'import json, sys; print(json.load(open(sys.argv[1]))["token"])' "$control")"
    pid="$("$PY" -c 'import json, sys; print(json.load(open(sys.argv[1]))["pid"])' "$control")"
    copy="$(curl -fsS -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        -d "{\"target_version\":\"$target\"}" "http://127.0.0.1:$PORT/api/v1/system/update/prepare" \
        | "$PY" -c 'import json, sys; print(json.load(sys.stdin)["database_copy"]["path"])')" \
        || fail "the backup before the update was refused"
    "$PY" - "$(native "$archive")" "$(native "$INSTALL")" "$(native "$DATA")" "$(native "$copy")" \
        "$from" "$target" "$pid" "$EXT" "$PORT" "$DATA/update/journal.json" <<'PY'
import datetime, hashlib, json, os, sys
archive, install, data, copy, source, target, pid, ext, port, path = sys.argv[1:]
# The service answers with the copy's path as it knows it, relative to its folder; the journal
# names every path absolute, and the copy inside <data>/pre-update (Journal::check).
if not os.path.isabs(copy):
    copy = os.path.join(install, copy)
with open(archive, "rb") as handle:
    body = handle.read()
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
plan = {
    "kind": "portable", "from_version": source, "target_version": target,
    "artifact": archive, "sha256": hashlib.sha256(body).hexdigest(), "size": len(body),
    "install_dir": install, "executable": "rdownloader" + ext, "data_dir": data,
    "database": data + "/rdownloader.sqlite3", "database_copy": copy,
    "service_pid": int(pid), "service_args": ["serve", "--listen", "127.0.0.1:" + port],
    "service_cwd": install, "health_timeout_secs": 60, "previous_installer": None,
}
journal = {
    "plan": plan, "phase": "handed", "entries": [], "replaced": [], "new_started": False,
    "start_attempts": 0, "reason": None, "detail": None, "started_at": now,
    "updated_at": now, "cleaned": False,
}
os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path, "w") as handle:
    json.dump(journal, handle)
PY
    mkdir -p "$DATA/update/updater"
    cp "$running" "$DATA/update/updater/rdownloader-updater$EXT"
    printf '%s\n' "$DATA/update/updater/rdownloader-updater$EXT"
}

apply() {
    local updater="$1" code=0
    (cd "$INSTALL" && "$updater" apply-update --journal "$(native "$DATA/update/journal.json")") \
        >> "$LOGS/updater-console.log" 2>&1 || code=$?
    printf '%s\n' "$code"
}

echo "==> the old version as a portable installation"
cp "$OLD_BINARY" "$INSTALL/rdownloader$EXT"
printf '%s\n' "$OLD_VERSION" > "$INSTALL/VERSION.txt"
printf 'old readme\n' > "$INSTALL/README.md"
mkdir -p "$INSTALL/downloads" "$INSTALL/logs"
printf 'payload\n' > "$INSTALL/downloads/kept.bin"
(cd "$INSTALL" && nohup "./rdownloader$EXT" serve --listen "127.0.0.1:$PORT" \
    >> "$INSTALL/logs/rdownloader.log" 2>> "$INSTALL/logs/rdownloader.err.log" < /dev/null &)
await_version "$OLD_VERSION"

echo "==> event streams held open: the web interface's (a session) and a capture agent's"
BASE="http://127.0.0.1:$PORT"
JAR="$WORK/cookies"
PASSWORD="self-update-smoke-password"
curl -fsS -c "$JAR" -H 'Content-Type: application/json' -d "{\"password\":\"$PASSWORD\"}" \
    "$BASE/api/v1/auth/setup" > /dev/null || fail "the setup was refused"
curl -fsS -b "$JAR" -c "$JAR" -H 'Content-Type: application/json' -d "{\"password\":\"$PASSWORD\"}" \
    "$BASE/api/v1/auth/login" > /dev/null || fail "the sign-in was refused"
bearer="$(curl -fsS -b "$JAR" -H 'Content-Type: application/json' -d '{"label":"self-update smoke"}' \
    "$BASE/api/v1/capture/pair" | "$PY" -c 'import json, sys; print(json.load(sys.stdin)["bearer"])')" \
    || fail "the capture pairing was refused"
curl -sN -b "$JAR" "$BASE/api/v1/events" > "$LOGS/events.log" 2>&1 &
STREAM_PIDS="$!"
curl -sN -H "Authorization: Bearer $bearer" "$BASE/api/v1/capture/events" > "$LOGS/capture-events.log" 2>&1 &
STREAM_PIDS="$STREAM_PIDS $!"
sleep 2
for pid in $STREAM_PIDS; do kill -0 "$pid" 2> /dev/null || fail "an event stream did not stay open"; done
grep -q '^retry:' "$LOGS/events.log" || fail "the web interface's event stream did not open"
grep -q '^retry:' "$LOGS/capture-events.log" || fail "the capture event stream did not open"

echo "==> 1. the next version"
archive="$(candidate next "$NEW_BINARY" "$NEW_VERSION" 'new readme')"
updater="$(hand_over "$OLD_VERSION" "$NEW_VERSION" "$archive" "$INSTALL/rdownloader$EXT")"
code="$(apply "$updater")"
[[ "$code" == 0 ]] || fail "the update ended with $code, not 0"
await_version "$NEW_VERSION"
! grep -q 'ended by force' "$LOGS/updater-console.log" \
    || fail "the old version did not end with event streams open; the updater ended it by force"
for pid in $STREAM_PIDS; do
    ! kill -0 "$pid" 2> /dev/null || fail "an event stream outlived the version that served it"
done
STREAM_PIDS=""
[[ "$(journal_field phase)" == verified ]] || fail "the journal says $(journal_field phase)"
grep -qx 'new readme' "$INSTALL/README.md" || fail "README.md is not the new one"
grep -qx 'old readme' "$INSTALL/.previous/README.md" || fail ".previous/ lacks the old README.md"
grep -qx 'payload' "$INSTALL/downloads/kept.bin" || fail "the downloads folder changed"
[[ -f "$DATA/rdownloader.sqlite3" ]] || fail "the database is gone"

echo "==> 2. a candidate that ends at once is taken back"
if [[ -n "$EXT" ]]; then
    broken="$(native "${SYSTEMROOT:-C:/Windows}")/System32/whoami.exe"
else
    broken="$WORK/broken-program"
    printf '#!/bin/sh\nexit 3\n' > "$broken"
    chmod +x "$broken"
fi
archive="$(candidate broken "$broken" 99.0.0 'broken readme')"
updater="$(hand_over "$NEW_VERSION" 99.0.0 "$archive" "$INSTALL/rdownloader$EXT")"
code="$(apply "$updater")"
[[ "$code" == 2 ]] || fail "the broken update ended with $code, not 2 (rolled back)"
await_version "$NEW_VERSION"
[[ "$(journal_field phase)" == rolled_back ]] || fail "the journal says $(journal_field phase)"
[[ "$(journal_field reason)" == update.new_version_exited ]] || fail "the reason is $(journal_field reason)"
grep -qx 'new readme' "$INSTALL/README.md" || fail "README.md is not the one from before the broken update"
[[ -f "$DATA/update/replaced-database/rdownloader.sqlite3" ]] \
    || fail "the database copy from before the update was not put back"

if [[ "${RD_SMOKE_DEBUG_BUILD:-0}" == 1 ]]; then
    echo "==> 3. a candidate that answers but is declared unhealthy is stopped and taken back"
    archive="$(candidate unhealthy "$NEW_BINARY" "$NEW_VERSION" 'unhealthy readme')"
    updater="$(hand_over "$NEW_VERSION" "$NEW_VERSION" "$archive" "$INSTALL/rdownloader$EXT")"
    export RD_UPDATE_TEST_FAIL_HEALTH=1
    code="$(apply "$updater")"
    unset RD_UPDATE_TEST_FAIL_HEALTH
    [[ "$code" == 2 ]] || fail "the unhealthy update ended with $code, not 2 (rolled back)"
    await_version "$NEW_VERSION"
    [[ "$(journal_field reason)" == update.health_failed_test ]] || fail "the reason is $(journal_field reason)"
    grep -qx 'new readme' "$INSTALL/README.md" || fail "README.md is not the one from before"
fi

echo "==> stop"
(cd "$INSTALL" && "./rdownloader$EXT" stop --wait 60) || fail "the service did not stop"
cp "$DATA/update/updater.log" "$LOGS/" 2>/dev/null || true
echo "==> all requested checks passed"
