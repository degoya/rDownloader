#!/usr/bin/env bash
#
# scripts/ci-published-repo.sh's apt half against stubs (`apt-get`, `apt-cache`, `curl`,
# `useradd`, `runuser`, `rdownloader`) under a scratch root: the key and the .sources file are
# fetched from the base URL and a `trusted=` in it is refused; the newest version must be the
# expected one, waited for while Pages publishes it; with an earlier release it is installed first, started, given a marker, upgraded
# and started again with the marker kept; one the repository no longer keeps falls back to the
# newest with a warning; the removal must leave the database (RD-1120-07). The dnf half differs
# only in the package manager's commands.
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
cat > "$SCRATCH/bin/useradd" <<'EOF'
#!/usr/bin/env bash
mkdir -p "$RD_PUBLISHED_REPO_FS/home/tester"
EOF
cat > "$SCRATCH/bin/runuser" <<'EOF'
#!/usr/bin/env bash
shift 3
HOME="$RD_PUBLISHED_REPO_FS/home/tester" exec "$@"
EOF
cat > "$SCRATCH/bin/rdownloader" <<'EOF'
#!/usr/bin/env bash
case "$1" in
    --version) echo "rdownloader $(cat "$FAKE/installed")" ;;
    serve)
        echo "serve $(cat "$FAKE/installed")" >> "$FAKE/calls"
        mkdir -p "$HOME/.local/share/rdownloader/data"
        echo sqlite > "$HOME/.local/share/rdownloader/data/rdownloader.sqlite3"
        exec sleep 30 ;;
esac
EOF
chmod +x "$SCRATCH/bin/"*
export PATH="$SCRATCH/bin:$PATH"
BASE=https://example.invalid/packages
published() {
    rm -rf "$RD_PUBLISHED_REPO_FS" "$FAKE/calls" "$FAKE/installed" "$FAKE/looks"
    mkdir -p "$RD_PUBLISHED_REPO_FS/etc/apt/sources.list.d"
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

finish_tests "ci-published-repo"
