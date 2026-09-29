#!/usr/bin/env bash
#
# scripts/package-managers.sh against a fixture SHA256SUMS in the release's format (RD-180-06):
# every archive's hash lands beside its own URL in both formulas, the capture formula depends on
# the tap's rdownloader and runs its agent, the Scoop manifest is JSON, and a release that lacks
# an archive renders nothing.
#
#   scripts/tests/package-managers.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

SUMS="$ROOT/scripts/tests/fixtures/package-managers/SHA256SUMS"
render() { "$ROOT/scripts/package-managers.sh" "$@"; }
# The line after the one naming $2 in file $1: the sha256 under a formula's url.
line_after() { awk -v needle="$2" 'found { print; exit } index($0, needle) { found = 1 }' "$1"; }
# One field of the Scoop manifest, by a Python expression over `m`.
manifest() { python3 -c "import json, sys; m = json.load(open(sys.argv[1])); print($2)" "$1"; }
hash_of() { printf "$1%.0s" {1..64}; }

run_status render v1.6.0 "$SUMS" "$SCRATCH/out"
expect_status "renders both files from the release's SHA256SUMS" 0
formula="$SCRATCH/out/rdownloader.rb"
scoop="$SCRATCH/out/rdownloader.json"

expect "the formula's version, without the tag's v" '  version "1.6.0"' "$(grep '^  version ' "$formula")"
base="https://github.com/degoya/rDownloader/releases/download/v1.6.0"
expect "macOS arm64: its archive and its hash" "      sha256 \"$(hash_of d)\"" \
    "$(line_after "$formula" "url \"$base/rdownloader-macos-aarch64.tar.gz\"")"
expect "macOS x86_64: its archive and its hash" "      sha256 \"$(hash_of e)\"" \
    "$(line_after "$formula" "url \"$base/rdownloader-macos-x86_64.tar.gz\"")"
expect "Linux arm64: its archive and its hash" "      sha256 \"$(hash_of b)\"" \
    "$(line_after "$formula" "url \"$base/rdownloader-linux-aarch64.tar.gz\"")"
expect "Linux x86_64: its archive and its hash" "      sha256 \"$(hash_of c)\"" \
    "$(line_after "$formula" "url \"$base/rdownloader-linux-x86_64.tar.gz\"")"
expect "the formula names its tap" "1" "$(grep -c 'in the tap degoya/homebrew-rdownloader' "$formula")"
expect "no placeholder is left in the formula" "0" "$(grep -c '@[A-Z0-9_]*@' "$formula" || true)"

capture="$SCRATCH/out/rdownloader-capture.rb"
expect "the capture formula's version" '  version "1.6.0"' "$(grep '^  version ' "$capture")"
for pair in macos-aarch64=d macos-x86_64=e linux-aarch64=b linux-x86_64=c; do
    expect "capture formula, ${pair%=*}: the same archive and hash" "      sha256 \"$(hash_of "${pair#*=}")\"" \
        "$(line_after "$capture" "url \"$base/rdownloader-${pair%=*}.tar.gz\"")"
done
expect "the capture formula depends on the tap's rdownloader" \
    '  depends_on "degoya/rdownloader/rdownloader"' "$(grep '^  depends_on ' "$capture")"
expect "its service runs the agent from rdownloader's opt path" \
    '    run [Formula["degoya/rdownloader/rdownloader"].opt_libexec/"rdownloader-capture", "run"]' \
    "$(grep '^    run ' "$capture")"
expect "its service runs in the login session" "1" "$(grep -c '^    process_type :interactive$' "$capture")"
expect "its caveats start it through brew services" "1" \
    "$(grep -c '^        brew services start rdownloader-capture$' "$capture")"
expect "the main formula's caveats name the capture formula" "1" \
    "$(grep -c '^        brew install degoya/rdownloader/rdownloader-capture$' "$formula")"
expect "no placeholder is left in the capture formula" "0" "$(grep -c '@[A-Z0-9_]*@' "$capture" || true)"
if command -v ruby > /dev/null; then
    run_status ruby -c "$formula"
    expect_status "the formula is Ruby" 0
    run_status ruby -c "$capture"
    expect_status "the capture formula is Ruby" 0
else
    echo "skip the formulas are Ruby: no ruby on this machine"
fi

expect "the manifest is JSON with the version" "1.6.0" "$(manifest "$scoop" 'm["version"]')"
expect "the manifest's archive" "$base/rdownloader-windows-x86_64.zip" \
    "$(manifest "$scoop" 'm["architecture"]["64bit"]["url"]')"
expect "the manifest's hash" "$(hash_of f)" "$(manifest "$scoop" 'm["architecture"]["64bit"]["hash"]')"
expect "checkver asks the repository's releases" "https://github.com/degoya/rDownloader" \
    "$(manifest "$scoop" 'm["checkver"]["github"]')"
expect "autoupdate keeps Scoop's own variables" \
    "https://github.com/degoya/rDownloader/releases/download/v\$version/rdownloader-windows-x86_64.zip" \
    "$(manifest "$scoop" 'm["autoupdate"]["architecture"]["64bit"]["url"]')"
expect "data and downloads survive an update" "data downloads" "$(manifest "$scoop" '" ".join(m["persist"])')"

expect "the tap's README installs both formulas from the tap" \
    "brew install degoya/rdownloader/rdownloader brew install degoya/rdownloader/rdownloader-capture" \
    "$(grep '^brew install' "$SCRATCH/out/homebrew-README.md" | tr '\n' ' ' | sed 's/ $//')"
expect "the bucket's README adds the bucket" \
    "scoop bucket add rdownloader https://github.com/degoya/scoop-rdownloader" \
    "$(grep '^scoop bucket add' "$SCRATCH/out/scoop-README.md")"
expect "both READMEs name the version" "2" "$(cat "$SCRATCH"/out/*-README.md | grep -c 'Current version: 1.6.0\.')"

# A fork and a local fixture: the repository names tap, bucket and URLs, --base-url the archives.
run_status render 1.6.1-rc.1 "$SUMS" "$SCRATCH/fork" --repository someone/rDownloader \
    --base-url "file:///tmp/fixture/"
expect_status "a pre-release version, another repository and a base URL" 0
expect "the archives come from the base URL, without its trailing slash" \
    '      url "file:///tmp/fixture/rdownloader-macos-aarch64.tar.gz"' \
    "$(grep 'rdownloader-macos-aarch64' "$SCRATCH/fork/rdownloader.rb")"
expect "the fork's tap" "1" "$(grep -c 'in the tap someone/homebrew-rdownloader' "$SCRATCH/fork/rdownloader.rb")"
expect "the fork's releases for checkver" "https://github.com/someone/rDownloader" \
    "$(manifest "$SCRATCH/fork/rdownloader.json" 'm["checkver"]["github"]')"
expect "the fork's tap in its README" "brew install someone/rdownloader/rdownloader" \
    "$(grep -m 1 '^brew install' "$SCRATCH/fork/homebrew-README.md")"
expect "the fork's capture formula depends on the fork's tap" \
    '  depends_on "someone/rdownloader/rdownloader"' "$(grep '^  depends_on ' "$SCRATCH/fork/rdownloader-capture.rb")"
expect "the fork's bucket" "True" \
    "$(manifest "$SCRATCH/fork/rdownloader.json" '"someone/scoop-rdownloader" in m["##"]')"

# sha256sum's other spellings: no ./, binary mode's *, upper case.
sed -e 's#  \./rdownloader-linux#  rdownloader-linux#' -e 's#  \./rdownloader-macos# *rdownloader-macos#' \
    -e 's#^f\{64\}#'"$(hash_of F)"'#' "$SUMS" > "$SCRATCH/spellings"
run_status render 1.6.0 "$SCRATCH/spellings" "$SCRATCH/spellings-out"
expect_status "names without ./ or with * are found" 0
expect "the same formula either way" "" "$(diff "$formula" "$SCRATCH/spellings-out/rdownloader.rb")"
expect "the same manifest either way, hash in lower case" "" \
    "$(diff "$scoop" "$SCRATCH/spellings-out/rdownloader.json")"

grep -v 'rdownloader-linux-aarch64' "$SUMS" > "$SCRATCH/incomplete"
run_status render 1.6.0 "$SCRATCH/incomplete" "$SCRATCH/incomplete-out"
expect_status "a release without one archive is refused" 1
expect_output "and the archive is named" "no SHA-256 for rdownloader-linux-aarch64.tar.gz"
expect_true "and nothing is written" '[[ ! -e "$SCRATCH/incomplete-out" ]]'

sed 's#^f\{64\}#ffff#' "$SUMS" > "$SCRATCH/short"
run_status render 1.6.0 "$SCRATCH/short" "$SCRATCH/short-out"
expect_status "a hash that is no SHA-256 is refused" 1
expect_output "naming its archive" "no SHA-256 for rdownloader-windows-x86_64.zip"

run_status render 1.6 "$SUMS" "$SCRATCH/bad-version"
expect_status "a version that is not X.Y.Z is refused" 1
run_status render 1.6.0 "$SUMS" "$SCRATCH/bad-repo" --repository degoya
expect_status "a repository that is not OWNER/NAME is refused" 1
run_status render 1.6.0 "$SCRATCH/none" "$SCRATCH/no-sums"
expect_status "a missing SHA256SUMS is refused" 1
run_status render 1.6.0 "$SUMS"
expect_status "too few arguments is a usage error" 2
run_status render 1.6.0 "$SUMS" "$SCRATCH/x" --unknown
expect_status "an unknown option is a usage error" 2

finish_tests "package-managers.sh"
