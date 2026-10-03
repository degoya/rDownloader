#!/usr/bin/env bash
#
# scripts/package-managers.sh against a fixture SHA256SUMS in the release's format (RD-180-06):
# every archive's hash lands beside its own URL in both formulas, the capture formula depends on
# the tap's rdownloader and runs its agent, the Scoop manifest is JSON, the winget manifests and
# the AUR's PKGBUILD and .SRCINFO name the same archives and hashes (RD-180-07, RD-180-08), and a
# release that lacks an archive renders nothing.
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

# winget (RD-180-07): three manifests of one version, the portable ZIP with its folder on PATH.
winget="$SCRATCH/out/winget"
installer="$winget/degoya.rDownloader.installer.yaml"
locale="$winget/degoya.rDownloader.locale.en-US.yaml"
expect "the three winget manifests" \
    "degoya.rDownloader.installer.yaml degoya.rDownloader.locale.en-US.yaml degoya.rDownloader.yaml" \
    "$(ls "$winget" | tr '\n' ' ' | sed 's/ $//')"
expect "every manifest names the version" "3" "$(cat "$winget"/*.yaml | grep -c '^PackageVersion: 1.6.0$')"
expect "every manifest names the package" "3" \
    "$(cat "$winget"/*.yaml | grep -c '^PackageIdentifier: degoya.rDownloader$')"
expect "the installer is the Windows ZIP" \
    "  InstallerUrl: $base/rdownloader-windows-x86_64.zip" "$(grep '^  InstallerUrl: ' "$installer")"
expect "with its hash in upper case" "  InstallerSha256: $(hash_of F)" "$(grep '^  InstallerSha256: ' "$installer")"
expect "a portable inside a zip" "InstallerType: zip NestedInstallerType: portable" \
    "$(grep -E '^(Nested)?InstallerType: ' "$installer" | tr '\n' ' ' | sed 's/ $//')"
expect "both executables, each under its own name" \
    "- RelativeFilePath: rdownloader.exe - RelativeFilePath: rdownloader-capture.exe" \
    "$(grep '^- RelativeFilePath: ' "$installer" | tr '\n' ' ' | sed 's/ $//')"
expect "the package folder goes on PATH, not a symlink" "ArchiveBinariesDependOnPath: true" \
    "$(grep '^ArchiveBinariesDependOnPath: ' "$installer")"
expect "no Scope on a portable package (winget validate warns)" "" "$(grep '^Scope: ' "$installer" || true)"
expect "the release notes of the tag" \
    "ReleaseNotesUrl: https://github.com/degoya/rDownloader/releases/tag/v1.6.0" \
    "$(grep '^ReleaseNotesUrl: ' "$locale")"
expect "the licence of the tag" "LicenseUrl: https://github.com/degoya/rDownloader/blob/v1.6.0/LICENSE" \
    "$(grep '^LicenseUrl: ' "$locale")"
expect "no placeholder is left in the manifests" "0" "$(cat "$winget"/*.yaml | grep -c '@[A-Z0-9_]*@' || true)"
if python3 -c 'import yaml' 2> /dev/null; then
    expect "the manifests are YAML of one schema version" "1.12.0 1.12.0 1.12.0" \
        "$(python3 -c 'import sys, yaml; print(" ".join(yaml.safe_load(open(f))["ManifestVersion"] for f in sys.argv[1:]))' \
            "$winget"/*.yaml)"
else
    echo "skip the manifests are YAML: no PyYAML on this machine"
fi

# AUR (RD-180-08): rdownloader-bin from both Linux archives, .SRCINFO beside it.
aur="$SCRATCH/out/aur"
pkgbuild="$aur/PKGBUILD"
expect "the AUR repository's files" "PKGBUILD rdownloader-capture.service rdownloader.service" \
    "$(ls "$aur" | tr '\n' ' ' | sed 's/ $//')"
expect "and its .SRCINFO" "pkgbase = rdownloader-bin" "$(head -n 1 "$aur/.SRCINFO")"
expect "pkgver is the version" "pkgver=1.6.0" "$(grep '^pkgver=' "$pkgbuild")"
expect "x86_64: its archive" \
    "source_x86_64=(\"rdownloader-1.6.0-x86_64.tar.gz::$base/rdownloader-linux-x86_64.tar.gz\")" \
    "$(grep '^source_x86_64=' "$pkgbuild")"
expect "x86_64: its hash" "sha256sums_x86_64=('$(hash_of c)')" "$(grep '^sha256sums_x86_64=' "$pkgbuild")"
expect "aarch64: its archive" \
    "source_aarch64=(\"rdownloader-1.6.0-aarch64.tar.gz::$base/rdownloader-linux-aarch64.tar.gz\")" \
    "$(grep '^source_aarch64=' "$pkgbuild")"
expect "aarch64: its hash" "sha256sums_aarch64=('$(hash_of b)')" "$(grep '^sha256sums_aarch64=' "$pkgbuild")"
service_hash="$(sha256sum "$ROOT/packaging/aur/rdownloader.service" | awk '{ print $1 }')"
capture_hash="$(sha256sum "$ROOT/packaging/aur/rdownloader-capture.service" | awk '{ print $1 }')"
expect "the units' hashes, in the order of source=()" "sha256sums=('$service_hash' '$capture_hash')" \
    "$(sed -n '/^sha256sums=(/,/)/p' "$pkgbuild" | tr -s ' \n' ' ' | sed 's/ $//')"
expect "the units are copied as they are" "" \
    "$(diff "$ROOT/packaging/aur/rdownloader.service" "$aur/rdownloader.service"; \
       diff "$ROOT/packaging/aur/rdownloader-capture.service" "$aur/rdownloader-capture.service")"
expect ".SRCINFO: the same version" "	pkgver = 1.6.0" "$(grep $'^\tpkgver = ' "$aur/.SRCINFO")"
# What makepkg reads from the PKGBUILD, in .SRCINFO's order and spelling (the PKGBUILD only assigns
# at its top level, so sourcing it runs nothing); package-channels.yml compares the whole file
# with `makepkg --printsrcinfo` on Arch.
srcinfo_fields() {
    # shellcheck disable=SC1090,SC2154
    bash -c 'source "$1"
        for name in depends optdepends source sha256sums source_x86_64 sha256sums_x86_64 \
            source_aarch64 sha256sums_aarch64; do
            eval "values=(\"\${${name}[@]}\")"
            for value in "${values[@]}"; do printf "\t%s = %s\n" "$name" "$value"; done
        done' _ "$1"
}
expect ".SRCINFO: the PKGBUILD's dependencies, archives and hashes" "$(srcinfo_fields "$pkgbuild")" \
    "$(grep -E $'^\t(depends|optdepends|source|sha256sums)(_x86_64|_aarch64)? = ' "$aur/.SRCINFO")"
expect "the marker the update check reads" "1" "$(grep -c '^  echo aur > "$pkgdir/usr/lib/rdownloader/install-kind"$' "$pkgbuild")"
expect "no placeholder is left in PKGBUILD or .SRCINFO" "0" \
    "$(cat "$pkgbuild" "$aur/.SRCINFO" | grep -c '@[A-Z0-9_]*@' || true)"
run_status bash -n "$pkgbuild"
expect_status "the PKGBUILD is bash" 0

# --check-aur (RD-190-10): the release refuses to push the template's maintainer placeholder.
# Both cases are made here, so the test holds before and after the template names the account.
mkdir -p "$SCRATCH/placeholder/aur" "$SCRATCH/maintained/aur" "$SCRATCH/unnamed/aur"
sed 's/^# Maintainer: .*/# Maintainer: AUR_MAINTAINER <AUR_MAINTAINER_EMAIL>/' "$pkgbuild" \
    > "$SCRATCH/placeholder/aur/PKGBUILD"
sed 's/^# Maintainer: .*/# Maintainer: Jane Doe <jane at example dot org>/' "$pkgbuild" \
    > "$SCRATCH/maintained/aur/PKGBUILD"
grep -v '^# Maintainer: ' "$pkgbuild" > "$SCRATCH/unnamed/aur/PKGBUILD"
run_status render --check-aur "$SCRATCH/placeholder"
expect_status "a PKGBUILD with the maintainer placeholder is refused" 1
expect_output "naming the placeholder and the template" \
    "(# Maintainer: AUR_MAINTAINER <AUR_MAINTAINER_EMAIL>); put the AUR account's name and address into packaging/aur/PKGBUILD.in"
run_status render --check-aur "$SCRATCH/unnamed"
expect_status "a PKGBUILD without a maintainer line is refused" 1
expect_output "saying so" "(no '# Maintainer:' line)"
run_status render --check-aur "$SCRATCH/maintained"
expect_status "a PKGBUILD that names its maintainer passes" 0
expect_output "and the maintainer is printed" "Maintainer: Jane Doe <jane at example dot org>"
run_status render --check-aur "$SCRATCH/none"
expect_status "a missing PKGBUILD is refused" 1
run_status render --check-aur
expect_status "--check-aur without a directory is a usage error" 2

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
expect "the pre-release's pkgver has no hyphen" "pkgver=1.6.1_rc.1" "$(grep '^pkgver=' "$SCRATCH/fork/aur/PKGBUILD")"
expect "but its archive keeps the version" \
    'source_x86_64=("rdownloader-1.6.1-rc.1-x86_64.tar.gz::file:///tmp/fixture/rdownloader-linux-x86_64.tar.gz")' \
    "$(grep '^source_x86_64=' "$SCRATCH/fork/aur/PKGBUILD")"
expect "the pre-release's winget version" "PackageVersion: 1.6.1-rc.1" \
    "$(grep '^PackageVersion: ' "$SCRATCH/fork/winget/degoya.rDownloader.yaml")"
expect "the fork's release notes" "ReleaseNotesUrl: https://github.com/someone/rDownloader/releases/tag/v1.6.1-rc.1" \
    "$(grep '^ReleaseNotesUrl: ' "$SCRATCH/fork/winget/degoya.rDownloader.locale.en-US.yaml")"

# sha256sum's other spellings: no ./, binary mode's *, upper case.
sed -e 's#  \./rdownloader-linux#  rdownloader-linux#' -e 's#  \./rdownloader-macos# *rdownloader-macos#' \
    -e 's#^f\{64\}#'"$(hash_of F)"'#' "$SUMS" > "$SCRATCH/spellings"
run_status render 1.6.0 "$SCRATCH/spellings" "$SCRATCH/spellings-out"
expect_status "names without ./ or with * are found" 0
expect "the same formula either way" "" "$(diff "$formula" "$SCRATCH/spellings-out/rdownloader.rb")"
expect "the same manifest either way, hash in lower case" "" \
    "$(diff "$scoop" "$SCRATCH/spellings-out/rdownloader.json")"
expect "the same winget installer either way, hash in upper case" "" \
    "$(diff "$installer" "$SCRATCH/spellings-out/winget/degoya.rDownloader.installer.yaml")"
expect "the same PKGBUILD either way" "" "$(diff "$pkgbuild" "$SCRATCH/spellings-out/aur/PKGBUILD")"

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
