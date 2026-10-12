#!/usr/bin/env bash
#
# The screenshot tool (RD-1270-16): a demo service with synthetic data, the views of the user
# wiki, the website and the README taken from it, and their delivery. Until 1.27 it lived
# unversioned in a session directory and every release brief repeated its targets and rules.
#
#   views.mjs     every view: its wiki name, the demo state it needs (its group), how to reach it
#   targets.json  where each view goes — wiki PNG 1440x900, website WebP at device scale 2, README
#                 — and the wiki images taken by hand, each with its reason
#   seed.mjs      the demo data through the REST API; mock.py the origin the demo downloads from,
#                 behind a proxy profile so the queue shows example.* hosts, and on a second port
#                 nntp_mock.py, the news server of the `usenet` group behind a SOCKS5 profile
#   capture.mjs   takes views into <work>/out/<scale>x/; deliver.py copies them to the targets;
#                 alts.py writes the wiki's alt texts from alts.json, which name no numbers
#
# Usage:
#   scripts/screenshots/shots.sh --pkg DIR all                  # fresh service, seed, every group
#   scripts/screenshots/shots.sh --pkg DIR fresh start seed     # the demo state, kept running
#   scripts/screenshots/shots.sh --views 05,19-logs capture     # those views, each in its state
#   scripts/screenshots/shots.sh --views 05,42 --line 1.27 deliver   # into the release worktrees
#   scripts/screenshots/shots.sh stop
#
# Steps, in the order given; a failed step skips the ones after it, but a later `stop` still runs
# when this run ran `start` (steps.sh), and the exit status is non-zero:
#   fresh     a new work directory's service data, demo roots and password; refuses leftovers
#   start     the mock and the service (`<pkg>/rdownloader serve`), by their PID files
#   seed      the demo data (restarts the service once, after installing the bundled plugins)
#   capture   every group with a view in --views (all views without it): main, pause, restart,
#             held, signin, usenet, each with its demo state set up and put back
#   firstrun|wizard|main|pause|restart|held|signin|usenet   that group alone (firstrun before seed)
#   deliver   deliver.py for --views (--line X.Y: the release worktrees of release-branches.sh)
#   stop      the service and the mock
#   all       fresh start firstrun wizard seed capture stop
#
# Options: --pkg DIR (a package directory with the `rdownloader` binary and its bundled plugins,
# for start), --views LIST, --version X.Y.Z (shown where the service names its own version),
# --scales 1,2, --line X.Y. Nothing runs against the owner's service: the demo listens on its own
# port, with its own data directory.
#
# Environment:
#   RD_SHOTS_WORK        the work directory: service/, demo/, out/, logs/, the password, the cookie
#                        (default: /var/tmp/rdownloader-shots-<uid>)
#   RD_SHOTS_PORT        the service's port (18790); RD_SHOTS_MOCK_PORT the mock's (18791),
#                        RD_SHOTS_NNTP_PORT its news server's (18792)
#   RD_SHOTS_PLAYWRIGHT  Playwright's index.mjs (default: web/node_modules/playwright/index.mjs)
#   RD_SHOTS_CHROMIUM    a Chromium binary (default: Playwright's own, `pnpm --dir web exec
#                        playwright install chromium`)
#   RD_WIKI_SRC, RD_WEBSITE_DIR   the wiki's and the website's main checkout, for deliver
#
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORK="${RD_SHOTS_WORK:-/var/tmp/rdownloader-shots-$(id -u)}"
PORT="${RD_SHOTS_PORT:-18790}"
MOCK_PORT="${RD_SHOTS_MOCK_PORT:-18791}"
NNTP_PORT="${RD_SHOTS_NNTP_PORT:-18792}"
export RD_SHOTS_WORK="$WORK" RD_SHOTS_DEMO="$WORK/demo" RD_SHOTS_PORT="$PORT" RD_SHOTS_MOCK_PORT="$MOCK_PORT"
export RD_SHOTS_NNTP_PORT="$NNTP_PORT"
export RD_SHOTS_PLAYWRIGHT="${RD_SHOTS_PLAYWRIGHT:-$ROOT/web/node_modules/playwright/index.mjs}"
export RD_DATA="$WORK/demo" TZ=Europe/Berlin

pkg="" views="" version="" scales="" line=""
steps=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --pkg) pkg="${2:?--pkg needs a directory}"; shift 2 ;;
        --views) views="${2:?--views needs a list}"; shift 2 ;;
        --version) version="${2:?--version needs X.Y.Z}"; shift 2 ;;
        --scales) scales="${2:?--scales needs a list}"; shift 2 ;;
        --line) line="${2:?--line needs X.Y}"; shift 2 ;;
        -h|--help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) steps+=("$1"); shift ;;
    esac
done
[[ ${#steps[@]} -gt 0 ]] || { echo "no step; --help lists them" >&2; exit 2; }

LOGS="$WORK/logs"
alive() { [[ -f "$1" ]] && kill -0 "$(cat "$1")" 2> /dev/null; }

step_fresh() {
    alive "$LOGS/serve.pid" && { echo "the demo service is still running (stop first)" >&2; return 1; }
    # Never reused and never deleted here: a leftover is moved aside by hand.
    if [[ -e "$WORK/service" || -e "$WORK/demo" ]]; then
        echo "$WORK/service or $WORK/demo exists; move it aside first" >&2
        return 1
    fi
    mkdir -p "$WORK/service" "$WORK/demo/downloads" "$LOGS"
    rm -f "$WORK/cookie.txt" "$WORK/more-music" "$WORK/usenet-before.json"
    (umask 077 && python3 -c 'import secrets; print(secrets.token_urlsafe(18))' > "$WORK/password")
    echo "fresh: $WORK"
}

step_start() {
    mkdir -p "$LOGS"
    [[ -n "$pkg" ]] || { echo "start needs --pkg DIR (a package with the rdownloader binary)" >&2; return 1; }
    [[ -x "$pkg/rdownloader" ]] || { echo "no rdownloader in '$pkg'" >&2; return 1; }
    if ! alive "$LOGS/mock.pid"; then
        nohup python3 "$HERE/mock.py" "$MOCK_PORT" "$NNTP_PORT" > "$LOGS/mock.log" 2>&1 < /dev/null &
        echo $! > "$LOGS/mock.pid"
    fi
    if ! alive "$LOGS/serve.pid"; then
        nohup "$pkg/rdownloader" serve --database "$WORK/service/rdownloader.sqlite3" \
            --downloads "$WORK/demo/downloads" --listen "127.0.0.1:$PORT" \
            --plugin-root "$WORK/service/plugins" >> "$LOGS/serve.log" 2>&1 < /dev/null &
        echo $! > "$LOGS/serve.pid"
    fi
    local _
    for _ in $(seq 1 60); do
        if curl -fsS "http://127.0.0.1:$PORT/api/v1/auth/status" > /dev/null 2>&1; then
            # The answer has to come from the process just started, not from one left over.
            if ! ss -ltnp | grep ":$PORT " | grep -q "pid=$(cat "$LOGS/serve.pid"),"; then
                echo "port $PORT is held by another process" >&2
                return 1
            fi
            echo "service up (PID $(cat "$LOGS/serve.pid"))"
            return 0
        fi
        sleep 1
    done
    echo "the service did not come up; see $LOGS/serve.log" >&2
    return 1
}

stop_pid() {
    local file="$1" pid _
    alive "$file" || { rm -f "$file"; return 0; }
    pid="$(cat "$file")"
    kill "$pid"
    for _ in $(seq 1 30); do kill -0 "$pid" 2> /dev/null || break; sleep 1; done
    if kill -0 "$pid" 2> /dev/null; then echo "PID $pid still alive after 30 s" >&2; return 1; fi
    rm -f "$file"
}

step_stop() {
    stop_pid "$LOGS/serve.pid"
    stop_pid "$LOGS/mock.pid"
    if ss -ltn | grep -E ":($PORT|$MOCK_PORT|$NNTP_PORT) "; then echo "a port is still held" >&2; return 1; fi
}

seed() {
    local status
    node "$HERE/seed.mjs" "$1" > "$LOGS/seed-$1.log" 2>&1
    status=$?
    echo "seed $1: exit $status"
    tail -n 4 "$LOGS/seed-$1.log"
    return "$status"
}

step_seed() {
    seed plugins || return 1
    stop_pid "$LOGS/serve.pid" && step_start || return 1
    seed base || return 1
    seed ntfy
    seed categories
    seed accounts
    seed small || return 1
    sleep 25
    seed big || return 1
    seed grab || return 1
    seed windows || return 1
    seed state
    # The throttled transfers show progress and a rate.
    sleep 60
}

# The views of one group, as capture.mjs selects them; empty when --views names none of them.
selected() { node "$HERE/capture.mjs" --list --group "$1" ${views:+--views "$views"}; }

capture() {
    local group="$1" status
    local args=(--group "$group")
    [[ -n "$views" ]] && args+=(--views "$views")
    [[ -n "$version" ]] && args+=(--version "$version")
    [[ -n "$scales" ]] && args+=(--scales "$scales")
    node "$HERE/capture.mjs" "${args[@]}" > "$LOGS/capture-$group.log" 2>&1
    status=$?
    echo "capture $group: exit $status"
    cat "$LOGS/capture-$group.log"
    return "$status"
}

step_firstrun() { capture firstrun; }
step_wizard() { seed login && capture wizard; }
step_main() { capture main; }
step_pause() { seed pause-on && sleep 6 && capture pause; seed pause-off; }
step_restart() { seed restart-on && sleep 2 && capture restart; seed restart-off; }
step_held() { touch "$WORK/more-music"; capture held; rm -f "$WORK/more-music"; }
step_signin() { capture signin; }
# The files need a moment to open their connections before the first view; a failed capture
# stays a failure after the put-back.
step_usenet() {
    local status=0
    seed usenet-on && sleep 20 && capture usenet || status=1
    seed usenet-off || status=1
    return "$status"
}

step_capture() {
    local group list status=0
    for group in main pause restart held signin usenet; do
        list="$(selected "$group")" || return 2
        [[ -n "$list" ]] || continue
        "step_$group" || status=1
    done
    return "$status"
}

step_deliver() {
    [[ -n "$views" ]] || { echo "deliver needs --views: name the views that changed" >&2; return 2; }
    python3 "$HERE/deliver.py" --views "$views" ${line:+--line "$line"}
}

step_all() {
    local status=0
    step_fresh && step_start && step_firstrun && step_wizard && step_seed && step_capture || status=1
    step_stop || status=1
    return "$status"
}

# Every step known before the first one runs.
for step in "${steps[@]}"; do
    case "$step" in
        all|fresh|start|seed|capture|firstrun|wizard|main|pause|restart|held|signin|usenet|deliver|stop) ;;
        *) echo "unknown step $step" >&2; exit 2 ;;
    esac
done
# shellcheck source=steps.sh
source "$HERE/steps.sh"
run_steps "${steps[@]}"
