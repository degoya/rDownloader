#!/usr/bin/env bash
set -u
umask 077

BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
LOG_DIR="${BASE_DIR}/logs"
RUN_DIR="${BASE_DIR}/run"
MODE="${1:-all}"

case "${MODE}" in
    all|server|capture) ;;
    *) echo "Usage: $(basename "$0") [all|server|capture]" >&2; exit 2 ;;
esac

mkdir -p "${LOG_DIR}" "${RUN_DIR}" || exit 1
cd "${BASE_DIR}" || exit 1

# Optional settings (program defaults, adjust as needed):
# export RDOWNLOADER_LISTEN="127.0.0.1:8710"
# export RDOWNLOADER_DATABASE="data/rdownloader.sqlite3"
# export RDOWNLOADER_DOWNLOADS="downloads"
# export RUST_LOG="rdownloader=info,rd_=info"

process_matches() {
    local pid="$1" executable="$2" command
    kill -0 "${pid}" 2>/dev/null || return 1
    command="$(ps -p "${pid}" -o command= 2>/dev/null)" || return 1
    case "${command}" in
        "${executable}"|"${executable} "*) return 0 ;;
        *) return 1 ;;
    esac
}

find_process() {
    local executable="$1" pid command
    while read -r pid command; do
        case "${command}" in
            "${executable}"|"${executable} "*) printf '%s\n' "${pid}"; return 0 ;;
        esac
    done < <(ps -ax -o pid= -o command= 2>/dev/null)
    return 1
}

start_process() {
    local binary="$1" argument="$2"
    local executable="${BASE_DIR}/${binary}" pid_file="${RUN_DIR}/${binary}.pid" pid
    if [[ ! -x "${executable}" ]]; then
        echo "Not found or not executable: ${executable}" >&2
        return 1
    fi
    if [[ -f "${pid_file}" ]]; then
        pid="$(tr -d '[:space:]' < "${pid_file}")"
        if [[ "${pid}" =~ ^[0-9]+$ ]] && process_matches "${pid}" "${executable}"; then
            echo "${binary} is already running (PID ${pid})."
            return 0
        fi
        rm -f "${pid_file}"
    fi
    if pid="$(find_process "${executable}")"; then
        printf '%s\n' "${pid}" > "${pid_file}"
        echo "${binary} is already running (PID ${pid})."
        return 0
    fi
    nohup "${executable}" "${argument}" >>"${LOG_DIR}/${binary}.log" \
        2>>"${LOG_DIR}/${binary}.err.log" </dev/null &
    pid=$!
    printf '%s\n' "${pid}" > "${pid_file}"
    sleep 1
    if ! process_matches "${pid}" "${executable}"; then
        local code=0
        wait "${pid}" 2>/dev/null || code=$?
        rm -f "${pid_file}"
        # 10 means the capture agent has nothing to connect to yet, which is what a fresh
        # install looks like: pairing happens in the web interface once the server is up.
        if [ "${code}" -eq 10 ]; then
            echo "${binary} is not paired yet. Open the web interface, go to Settings >" \
                 "Desktop client, and run the command it shows. Then start this again."
            return 0
        fi
        # 11 means a Click'n'Load listener already holds port 9666. Nothing but a capture
        # agent binds it, so there is already one running -- a second copy from autostart,
        # or JDownloader. Not a failed start.
        if [ "${code}" -eq 11 ]; then
            echo "${binary} did not start: another Click'n'Load listener already has port" \
                 "9666. That is a second ${binary} (check your autostart) or JDownloader."
            return 0
        fi
        echo "${binary} exited during startup; see ${LOG_DIR}/${binary}.err.log" >&2
        return 1
    fi
    echo "${binary} started (PID ${pid})."
}

open_browser_if_unconfigured() {
    # First run: once the server answers, open the browser so the setup wizard shows up.
    local addr="${RDOWNLOADER_LISTEN:-127.0.0.1:8710}" base response attempt fetch
    if command -v curl >/dev/null 2>&1; then fetch="curl -fsS --max-time 2"
    elif command -v wget >/dev/null 2>&1; then fetch="wget -qO- --timeout=2"
    else return 0; fi
    base="http://${addr/#0.0.0.0/127.0.0.1}"
    for attempt in $(seq 1 30); do
        response="$(${fetch} "${base}/api/v1/auth/status" 2>/dev/null)" || { sleep 0.5; continue; }
        if [[ "${response//[[:space:]]/}" == *'"setup_required":true'* ]]; then
            command -v xdg-open >/dev/null 2>&1 && xdg-open "${base}" >/dev/null 2>&1 &
        fi
        return 0
    done
    return 0
}

status=0
if [[ "${MODE}" == all || "${MODE}" == server ]]; then start_process rdownloader serve || status=1; fi
if [[ "${MODE}" == all || "${MODE}" == capture ]]; then start_process rdownloader-capture run || status=1; fi
if [[ "${MODE}" != capture ]]; then echo "Web UI: http://localhost:8710"; fi
if [[ "${MODE}" != capture && "${status}" -eq 0 ]]; then open_browser_if_unconfigured; fi
echo "Logs: ${LOG_DIR}"
exit "${status}"
