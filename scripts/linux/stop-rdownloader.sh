#!/usr/bin/env bash
set -u

BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
RUN_DIR="${BASE_DIR}/run"
MODE="${1:-all}"

case "${MODE}" in
    all|server|capture) ;;
    *) echo "Usage: $(basename "$0") [all|server|capture]" >&2; exit 2 ;;
esac

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

stop_process() {
    local binary="$1" executable="${BASE_DIR}/$1" pid_file="${RUN_DIR}/$1.pid"
    local pid="" attempt
    if [[ -f "${pid_file}" ]]; then
        pid="$(tr -d '[:space:]' < "${pid_file}")"
        if [[ ! "${pid}" =~ ^[0-9]+$ ]] || ! process_matches "${pid}" "${executable}"; then pid=""; fi
    fi
    if [[ -z "${pid}" ]]; then pid="$(find_process "${executable}" || true)"; fi
    if [[ -z "${pid}" ]]; then
        echo "${binary} is not running."
        rm -f "${pid_file}"
        return 0
    fi
    kill -TERM "${pid}" 2>/dev/null || true
    for attempt in {1..10}; do
        if ! kill -0 "${pid}" 2>/dev/null; then
            echo "${binary} stopped."
            rm -f "${pid_file}"
            return 0
        fi
        sleep 1
    done
    kill -KILL "${pid}" 2>/dev/null || true
    rm -f "${pid_file}"
    echo "${binary} terminated forcefully."
}

if [[ "${MODE}" == all || "${MODE}" == capture ]]; then stop_process rdownloader-capture; fi
if [[ "${MODE}" == all || "${MODE}" == server ]]; then stop_process rdownloader; fi
