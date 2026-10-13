#!/usr/bin/env bash
#
# scripts/winget-supersede.sh (RD-1280-03) against a `gh` that answers from files: after a
# submission it closes the account's own open pull requests for an older rDownloader version with
# "Superseded by #<new>", the new one's number from komac's log or, without it, from the open
# pull request of the submitted version; it never closes the new one, one of another author, one
# for the same or a newer version, a pre-release of a newer one or another package's; every
# failure — no account, no list, no new pull request, a refused close — is a warning and exit 0.
#
#   scripts/tests/winget-supersede.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
# gh: every call logged; `api user` prints $FAKE/login (fails without it); `pr list` prints
# $FAKE/open, the lines its --jq would (fails without it); `pr close <n>` fails when $FAKE/refuse
# names <n>.
cat > "$SCRATCH/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/calls"
case "$1 $2" in
    "api user") cat "$FAKE/login" 2> /dev/null ;;
    "pr list") cat "$FAKE/open" 2> /dev/null ;;
    "pr close") ! grep -qx "$3" "$FAKE/refuse" 2> /dev/null ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH"

title() { printf 'New version: degoya.rDownloader version %s' "$1"; }
reset() {
    rm -f "$FAKE"/*
    echo degoya > "$FAKE/login"
    {
        printf '451400\tdegoya\t%s\n' "$(title 1.28.0)"
        printf '451308\tdegoya\t%s\n' "$(title 1.27.0)"
        printf '450001\tdegoya\t%s\n' "$(title 1.16.1)"
        printf '450900\tdegoya\t%s\n' "$(title 1.9.0)"
        printf '451000\tdegoya\t%s\n' "$(title 1.28.0-beta.1)"
        printf '451401\tdegoya\t%s\n' "$(title 1.28.0)"
        printf '451402\tdegoya\t%s\n' "$(title 1.29.0-beta.1)"
        printf '451403\tdegoya\t%s\n' "$(title 1.30.0)"
        printf '449000\tsomeone\t%s\n' "$(title 1.10.0)"
        printf '449001\tdegoya\tNew version: degoya.OtherTool version 1.0.0\n'
    } > "$FAKE/open"
}
closes() { grep -c '^pr close' "$FAKE/calls" || true; }
closed() { grep -q "^pr close $1 " "$FAKE/calls"; }

run_status "$ROOT/scripts/winget-supersede.sh"
expect_status "no version is a usage error" 2
run_status "$ROOT/scripts/winget-supersede.sh" v1.28.0
expect_status "a tag instead of a version is a usage error" 2

reset
printf 'Created pull request: https://github.com/microsoft/winget-pkgs/pull/451400\n' > "$SCRATCH/komac.log"
run_status "$ROOT/scripts/winget-supersede.sh" 1.28.0 "$SCRATCH/komac.log"
expect_status "a submission with older ones open succeeds" 0
expect "the four older ones are closed, nothing else" "4" "$(closes)"
for number in 451308 450001 450900; do
    expect_true "#$number, older, is closed" "closed $number"
done
expect_true "the pre-release of the same version is closed" "closed 451000"
expect_output "each superseded by the new one" "closed #451308 (degoya.rDownloader 1.27.0), superseded by #451400"
expect_true "with the comment winget-pkgs' moderators read" \
    'grep -q "^pr close 451308 --repo microsoft/winget-pkgs --comment Superseded by #451400$" "$FAKE/calls"'
expect_true "the new one stays open" "! closed 451400"
expect_true "a second one for the same version stays (a re-run's duplicate)" "! closed 451401"
expect_true "a pre-release of a newer version stays" "! closed 451402"
expect_true "a newer version stays" "! closed 451403"
expect_true "another author's stays" "! closed 449000"
expect_true "another package's stays" "! closed 449001"
expect_true "the list asks for the account's open pull requests" \
    'grep -q "^pr list --repo microsoft/winget-pkgs --author degoya --state open " "$FAKE/calls"'

reset
run_status "$ROOT/scripts/winget-supersede.sh" 1.27.0
expect_status "without komac's log the new one is found by its title" 0
expect_true "the highest number for the version is the new one" \
    'grep -q "Superseded by #451308$" "$FAKE/calls"'
expect_true "and only older versions are closed" "closed 450001 && closed 450900 && ! closed 451000"

reset
run_status "$ROOT/scripts/winget-supersede.sh" 1.31.0
expect_status "no pull request for the version is a warning" 0
expect_output "saying so" "::warning::no open pull request for degoya.rDownloader 1.31.0"
expect "and nothing is closed" "0" "$(closes)"

reset
echo 450001 > "$FAKE/refuse"
run_status "$ROOT/scripts/winget-supersede.sh" 1.28.0 "$SCRATCH/komac.log"
expect_status "a refused close is a warning, not a red job" 0
expect_output "naming it" "::warning::#450001 (degoya.rDownloader 1.16.1) could not be closed"
expect_true "the others are still closed" "closed 451308 && closed 450900"

reset
rm "$FAKE/open"
run_status "$ROOT/scripts/winget-supersede.sh" 1.28.0 "$SCRATCH/komac.log"
expect_status "a list that fails is a warning" 0
expect_output "saying none is closed" "could not be listed; none is closed"

reset
rm "$FAKE/login"
run_status "$ROOT/scripts/winget-supersede.sh" 1.28.0 "$SCRATCH/komac.log"
expect_status "an account that cannot be read is a warning" 0
expect_output "saying nothing is closed" "::warning::the token's account could not be read"

finish_tests "winget-supersede"
