#!/usr/bin/env bash
#
# scripts/ci-published-repo.sh's apt half against stubs (`apt-get`, `apt-cache`, `curl`,
# `useradd`, `runuser`, `rdownloader`) under a scratch root: the key and the .sources file are
# fetched from the base URL and a `trusted=` in it is refused; the newest version must be the
# expected one, waited for while Pages publishes it; with an earlier release it is installed first, started, given a marker, upgraded
# and started again with the marker kept; one the repository no longer keeps falls back to the
# newest with a warning; the removal must leave the database (RD-1120-07). The stub `runuser`
# lets the tester write only below its home and into folders it made itself, as a real one does
# with root's 0700 `mktemp -d` (RD-1130-05). The dnf half differs in the package manager's
# commands; the stub `dnf` fails a look at the signed metadata without `-y`, as dnf does when CI
# answers its key import question with no (RD-1130-05).
#
#   scripts/tests/ci-published-repo.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake" RD_PUBLISHED_REPO_FS="$SCRATCH/fs" RD_PUBLISHED_REPO_WAIT=0 RD_PUBLISHED_REPO_POLL=0
mkdir -p "$FAKE" "$SCRATCH/bin"
cat > "$SCRATCH/bin/apt-get" <<'EOF'
#!/usr/bin/env bash
echo "apt-get $*" >> "$FAKE/calls"
newest="$(sort -V "$FAKE/versions" | tail -n 1)"
case "$*" in
    *"curl ca-certificates"*) ;;
    *--only-upgrade*) echo "$newest" > "$FAKE/installed" ;;
    *"rdownloader="*) args="$*"; echo "${args##*rdownloader=}" > "$FAKE/installed" ;;
    install*rdownloader) echo "$newest" > "$FAKE/installed" ;;
    purge*) rm -f "$FAKE/installed" ;;
esac
EOF
# apt-cache: the versions in $FAKE/versions; with $FAKE/late, 1.12.0 appears on the third look.
cat > "$SCRATCH/bin/apt-cache" <<'EOF'
#!/usr/bin/env bash
looks=$(( $(cat "$FAKE/looks" 2> /dev/null || echo 0) + 1 )); echo "$looks" > "$FAKE/looks"
[[ -f "$FAKE/late" && "$looks" -ge 3 ]] && echo 1.12.0 >> "$FAKE/versions"
while read -r version; do echo " rdownloader | $version | https://example.invalid/deb stable/main amd64 Packages"; done < <(sort -u "$FAKE/versions")
EOF
# curl: writes what -o asks for; the health check answers once the service wrote its database.
cat > "$SCRATCH/bin/curl" <<'EOF'
#!/usr/bin/env bash
echo "curl $*" >> "$FAKE/calls"
if [[ "$*" == *"/api/v1/health"* ]]; then
    # Up to 30 s: under a full run's load the fake service took longer than the 5 s this was
    # (the 1.12.0 release chain, 2026-10-06). It answers as soon as the database is there.
    for _ in $(seq 1 300); do
        [[ -s "$RD_PUBLISHED_REPO_FS/home/tester/.local/share/rdownloader/data/rdownloader.sqlite3" ]] && exit 0
        sleep 0.1
    done
    exit 7
fi
while [[ $# -gt 0 ]]; do
    if [[ "$1" == -o ]]; then
        if [[ "$2" == *.sources && -f "$FAKE/sources" ]]; then cp "$FAKE/sources" "$2"; else echo "Signed-By: x" > "$2"; fi
    fi
    shift
done
EOF
# dnf: installs as the apt-get stub does; repoquery lists $FAKE/versions, but only with -y.
cat > "$SCRATCH/bin/dnf" <<'EOF'
#!/usr/bin/env bash
echo "dnf $*" >> "$FAKE/calls"
newest="$(sort -V "$FAKE/versions" | tail -n 1)"
case "$*" in
    *util-linux*) ;;
    *repoquery*)
        [[ " $* " == *" -y "* ]] \
            || { echo "Error: repomd.xml GPG signature verification error: Signing key not found" >&2; exit 1; }
        sort -u "$FAKE/versions" ;;
    *"rdownloader-"*) args="$*"; echo "${args##*rdownloader-}" > "$FAKE/installed" ;;
    install*rdownloader | upgrade*rdownloader) echo "$newest" > "$FAKE/installed" ;;
    remove*) rm -f "$FAKE/installed" ;;
esac
EOF
cat > "$SCRATCH/bin/useradd" <<'EOF'
#!/usr/bin/env bash
mkdir -p "$RD_PUBLISHED_REPO_FS/home/tester"
EOF
cat > "$SCRATCH/bin/runuser" <<'EOF'
#!/usr/bin/env bash
shift 3
export HOME="$RD_PUBLISHED_REPO_FS/home/tester"
if [[ "$1" == mktemp ]]; then
    "$@" | tee -a "$FAKE/tester-dirs"
    exit "${PIPESTATUS[0]}"
fi
if [[ "$1" == bash && "$2" == -c ]]; then
    while read -r target; do
        folder="$(dirname "$target")"
        [[ "$folder" == "$HOME"* ]] || grep -qxF -- "$folder" "$FAKE/tester-dirs" 2> /dev/null \
            || { echo "bash: line 1: $target: Permission denied" >&2; exit 1; }
    done < <(grep -o "> '[^']*'" <<< "$3" | sed "s/^> '//; s/'\$//")
fi
exec "$@"
EOF
cat > "$SCRATCH/bin/rdownloader" <<'EOF'
#!/usr/bin/env bash
case "$1" in
    --version) echo "rdownloader $(cat "$FAKE/installed")" ;;
    serve)
        echo "serve $(cat "$FAKE/installed")" >> "$FAKE/calls"
        mkdir -p "$HOME/.local/share/rdownloader/data"
        echo sqlite > "$HOME/.local/share/rdownloader/data/rdownloader.sqlite3"
        # A service that outlives SIGTERM, standing in for one the container never reaps: `ps`
        # below calls it a zombie, `kill -0` keeps answering for it.
        if [[ -f "$FAKE/stubborn" ]]; then
            echo $$ >> "$FAKE/stubborn-pids"
            trap '' TERM
            while :; do sleep 1; done
        fi
        exec sleep 30 ;;
esac
EOF
# ps: a zombie while $FAKE/zombie exists, the real one otherwise.
cat > "$SCRATCH/bin/ps" <<'EOF'
#!/usr/bin/env bash
if [[ -f "$FAKE/zombie" ]]; then echo "Z"; exit 0; fi
exec /usr/bin/ps "$@"
EOF
chmod +x "$SCRATCH/bin/"*
export PATH="$SCRATCH/bin:$PATH"
BASE=https://example.invalid/packages
published() {
    rm -rf "$RD_PUBLISHED_REPO_FS" "$FAKE/calls" "$FAKE/installed" "$FAKE/looks" "$FAKE/tester-dirs"
    mkdir -p "$RD_PUBLISHED_REPO_FS/etc/apt/sources.list.d" "$RD_PUBLISHED_REPO_FS/etc/yum.repos.d"
    run_status "$ROOT/scripts/ci-published-repo.sh" "$@"
}
printf '%s\n' 1.10.1 1.11.0 > "$FAKE/versions"

published apt "$BASE" 1.11.0 v1.10.1
expect_status "an upgrade from the release before" 0
expect_output "says so" "rdownloader 1.11.0 from $BASE (upgraded from 1.10.1), data kept"
expect_true "the key and the source file from the base URL" \
    'grep -q "curl -fsSL $BASE/rdownloader.asc -o" "$FAKE/calls" && grep -q "curl -fsSL $BASE/rdownloader.sources -o" "$FAKE/calls"'
expect "the older one installed and started, upgraded, the newest started" \
    "apt-get install -y --allow-downgrades rdownloader=1.10.1|serve 1.10.1|apt-get install -y --only-upgrade rdownloader|serve 1.11.0|apt-get purge -y rdownloader" \
    "$(grep -E '^(serve|apt-get install -y --|apt-get purge)' "$FAKE/calls" | paste -sd'|' -)"
expect_true "the database is still there after the removal" '[[ -s "$RD_PUBLISHED_REPO_FS/home/tester/.local/share/rdownloader/data/rdownloader.sqlite3" ]]'
expect_true "the service's log went to a folder the tester made, removed at the end" \
    'run_dir="$(cat "$FAKE/tester-dirs")" && [[ -n "$run_dir" && ! -e "$run_dir" ]]'

published dnf "$BASE" 1.11.0 v1.10.1
expect_status "an upgrade from the release before through dnf" 0
expect_output "says so" "rdownloader 1.11.0 from $BASE (upgraded from 1.10.1), data kept"
expect_true "the metadata looked at with -y, so dnf imports the repository's key" 'grep -q "^dnf -y repoquery --refresh" "$FAKE/calls"'
expect "the older one installed and started, upgraded, the newest started" \
    "dnf install -y rdownloader-1.10.1|serve 1.10.1|dnf upgrade -y rdownloader|serve 1.11.0|dnf remove -y rdownloader" \
    "$(grep -E '^(serve|dnf (install|upgrade|remove) -y rdownloader)' "$FAKE/calls" | paste -sd'|' -)"

published apt "$BASE" 1.11.0
expect_status "a fresh install" 0
expect_output "says so" "(fresh install), data kept"

published apt "$BASE" 1.11.0 v1.9.0
expect_status "a release the repository no longer keeps" 0
expect_output "warns" "::warning::the repository keeps no 1.9.0 any more"
expect_true "and installs the newest alone" '! grep -q "rdownloader=1.9.0" "$FAKE/calls"'

# Pages publishes late: the expected version appears on the third look.
echo late > "$FAKE/late"
RD_PUBLISHED_REPO_WAIT=60 published apt "$BASE" 1.12.0
expect_status "a version Pages publishes late is waited for" 0
expect "looked at three times" "3" "$(cat "$FAKE/looks")"
rm -f "$FAKE/late" "$FAKE/looks"
printf '%s\n' 1.10.1 1.11.0 > "$FAKE/versions"

published apt "$BASE" 1.12.0
expect_status "a repository the release did not move" 1
expect_output "is named" "the repository carries 1.11.0, the release was 1.12.0"

printf 'Types: deb\nTrusted=yes\n' > "$FAKE/sources"
published apt "$BASE"
expect_status "a source file that trusts without a signature" 1
expect_output "is refused" "trusts the repository without its signature"
rm -f "$FAKE/sources"

published zypper "$BASE"
expect_status "an unknown package manager" 2

# A service the container keeps as a zombie: stopped is stopped (RD-1130-05, 1.14.0's channels run).
printf '%s\n' 1.11.0 > "$FAKE/versions"
touch "$FAKE/stubborn" "$FAKE/zombie"
SECONDS=0
published apt "$BASE" 1.11.0
expect_status "a service left a zombie counts as stopped" 0
expect_true "and the check does not wait for it" '(( SECONDS < 15 ))'
rm -f "$FAKE/zombie"
RD_PUBLISHED_REPO_STOP_WAIT=2 published apt "$BASE" 1.11.0
expect_status "a service that does not stop fails the check" 1
expect_output "naming it" "did not stop within"
rm -f "$FAKE/stubborn"
# The stubborn services ignore SIGTERM on purpose; the test ends them itself.
while read -r pid; do kill -9 "$pid" 2> /dev/null || true; done < "$FAKE/stubborn-pids"

finish_tests "ci-published-repo"
