#!/usr/bin/env bash
#
# What Homebrew, the service manager and the system know about the `rdownloader` service once
# channels.yml's `homebrew` job found it not answering (RD-1130-05): on macos-15 and
# macos-15-intel the 1.12.0 service upgraded from 1.11.0 was "Successfully started" and wrote
# nothing. Prints only, one group per probe, and never fails — a probe the image lacks says so.
# Last, the service binary started in the foreground from the service's working folder for up
# to 20 s, its output shown: one that answers there points at the service manager, not the
# binary.
#
#   scripts/ci-brew-service-diagnostics.sh
#
# CI only: it starts the service binary on the machine it runs on.
set -uo pipefail

prefix="$(brew --prefix)"
binary="${prefix}/opt/rdownloader/libexec/rdownloader"
var="${prefix}/var/rdownloader"

probe() {
    echo "::group::$1"
    bash -c "$2" 2>&1 || echo "(exit $?)"
    echo "::endgroup::"
}

export prefix binary var
probe "brew services" 'brew services list; brew services info rdownloader'
probe "processes" 'ps ax -o pid,ppid,etime,command | grep -E "/rdownloade[r]( |$)"'
probe "port 8710" 'lsof -nP -iTCP:8710'
probe "kegs and folders" 'ls -la "${prefix}/Cellar/rdownloader" "${prefix}/opt/rdownloader" \
    "${prefix}/opt/rdownloader/libexec" "${var}" "${var}/data" "${prefix}/var/log"'
probe "binary" '"${binary}" --version'
if [[ "$(uname -s)" == Darwin ]]; then
    probe "launch agent plists" 'ls -la ~/Library/LaunchAgents
        for plist in ~/Library/LaunchAgents/*rdownloader*.plist; do echo "== ${plist}"; plutil -p "${plist}"; done'
    probe "launchd" 'for label in sh.brew.rdownloader homebrew.mxcl.rdownloader; do
            launchctl print "gui/$(id -u)/${label}"
        done
        launchctl print-disabled "gui/$(id -u)" | grep -i rdownloader'
    probe "code signature" 'codesign --display --verbose=2 "${binary}"; codesign --verify --verbose=2 "${binary}"
        xattr -l "${binary}"'
    probe "system log" 'log show --last 5m --style compact \
        --predicate "process == \"rdownloader\" OR eventMessage CONTAINS[c] \"rdownloader\"" | tail -n 200'
else
    probe "systemd" 'systemctl --user list-units --all "*rdownloader*"
        systemctl --user status "*rdownloader*" --no-pager
        journalctl --user --unit "*rdownloader*" --lines 100 --no-pager'
fi

echo "::group::foreground start from ${var}"
output="$(mktemp)"
(cd "${var}" && exec "${binary}" serve) > "${output}" 2>&1 &
pid=$!
for _ in $(seq 1 20); do
    curl --fail --silent --show-error http://127.0.0.1:8710/api/v1/health && { echo; echo "answered in the foreground"; break; }
    kill -0 "${pid}" 2> /dev/null || break
    sleep 1
done
kill "${pid}" 2> /dev/null
wait "${pid}"
echo "foreground exit: $?"
cat "${output}"
rm -f "${output}"
echo "::endgroup::"
exit 0
