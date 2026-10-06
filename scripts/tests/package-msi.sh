#!/usr/bin/env bash
#
# scripts/package-msi.sh --sources-only on a scratch unpacked zip (RD-180-05): WiX itself needs
# Windows, so this holds what the script generates. The MSI version drops a pre-release suffix,
# every bundled plugin becomes a file of the plugins component, the licence page is RTF with its
# special characters escaped, the marker says `msi`, and the generated and the checked-in WiX
# sources are well-formed XML. Each of the four languages picks its culture and .wxl, and every
# .wxl defines exactly the strings the WiX source asks for (RD-1120-20). An unverified package, a
# stage without plugins, a version MSI cannot hold and an unknown language are refused.
#
#   scripts/tests/package-msi.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"
SCRIPT="$ROOT/scripts/package-msi.sh"

stage="$SCRATCH/stage"
mkdir -p "$stage/plugins"
while IFS= read -r entry; do echo "$entry" > "$stage/$entry"; done < <(rd_archive_entries windows)
printf 'rDownloader 1.8.0-beta.1\r\nprofile  release\r\n' > "$stage/VERSION.txt"
printf 'GNU GENERAL PUBLIC LICENSE {braces} and C:\\path\n' > "$stage/LICENSE"
echo package > "$stage/plugins/http-1.0.0.rdplug"
echo package > "$stage/plugins/usenet-nzb-2.1.0.rdplug"

run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out"
expect_status "generating the sources" 0
expect_output "naming both versions" "rdownloader 1.8.0-beta.1, MSI 1.8.0 en-US, 2 plugins"
expect "English without a language" "en-US" "$(grep -A1 '^-culture$' "$SCRATCH/out/wix-arguments.txt" | sed -n '2p')"
expect "the MSI version drops the pre-release" "Version=1.8.0" \
    "$(grep '^Version=' "$SCRATCH/out/wix-arguments.txt")"
expect "the marker says msi" "msi" "$(cat "$SCRATCH/out/install-kind")"
fragment="$SCRATCH/out/plugins.wxs"
for name in http-1.0.0.rdplug usenet-nzb-2.1.0.rdplug; do
    expect "the plugins component carries $name" 1 \
        "$(grep -c "Name=\"$name\" Source=\"\$(var.Stage)\\\\plugins\\\\$name\"" "$fragment")"
done
expect "the licence escapes RTF's special characters" \
    'GNU GENERAL PUBLIC LICENSE \{braces\} and C:\\path\par' "$(sed -n '2p' "$SCRATCH/out/license.rtf")"
for language in de:de-DE es:es-ES fr:fr-FR; do
    run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out-${language%%:*}" "${language%%:*}"
    expect_status "generating the ${language%%:*} sources" 0
    expect "${language%%:*} builds the ${language#*:} culture" "${language#*:}" \
        "$(grep -A1 '^-culture$' "$SCRATCH/out-${language%%:*}/wix-arguments.txt" | sed -n '2p')"
    expect "${language%%:*} takes its own strings" "${language#*:}.wxl" \
        "$(basename "$(grep -A1 '^-loc$' "$SCRATCH/out-${language%%:*}/wix-arguments.txt" | sed -n '2p')")"
done
run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out-it" it
expect_status "a language the interface does not have" 2
# Every string the source names, in every language, and nothing else.
run_status python3 - "$ROOT/packaging/msi" <<'PY'
import pathlib, re, sys, xml.etree.ElementTree as tree
folder = pathlib.Path(sys.argv[1])
wanted = set(re.findall(r"!\(loc\.([A-Za-z]+)\)", (folder / "rdownloader.wxs").read_text()))
cultures = sorted(path.stem for path in folder.glob("*.wxl"))
assert cultures == ["de-DE", "en-US", "es-ES", "fr-FR"], cultures
for culture in cultures:
    root = tree.parse(folder / f"{culture}.wxl").getroot()
    assert root.get("Culture") == culture, culture
    ids = {string.get("Id") for string in root if string.get("Value")}
    assert ids == wanted, (culture, sorted(ids ^ wanted))
PY
expect_status "every .wxl defines the strings rdownloader.wxs names" 0
for source in "$fragment" "$ROOT/packaging/msi/rdownloader.wxs"; do
    run_status python3 -c 'import sys, xml.etree.ElementTree as tree; tree.parse(sys.argv[1])' "$source"
    expect_status "$(basename "$source") is well-formed XML" 0
done
expect "the checked-in source keeps its UpgradeCode" 1 \
    "$(grep -c 'UpgradeCode="3346ED88-7297-43CC-9BA2-0478E5E2D851"' "$ROOT/packaging/msi/rdownloader.wxs")"
# The data folder is the service's; the installer must never name it.
expect "the installer never names the data folder" 0 \
    "$(grep -c 'LOCALAPPDATA%\\rDownloader\\\|AppData\\Local\\rDownloader' "$ROOT/packaging/msi/rdownloader.wxs" || true)"

echo "not verified" > "$stage/UNVERIFIED.txt"
run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out2"
expect_status "an unverified package" 1
rm "$stage/UNVERIFIED.txt"

printf 'rDownloader 1.256.0\n' > "$stage/VERSION.txt"
run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out3"
expect_status "a version MSI cannot hold" 1
printf 'rDownloader 1.8.0\n' > "$stage/VERSION.txt"

rm "$stage"/plugins/*.rdplug
run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out4"
expect_status "a stage without plugins" 1

rm "$stage/rdownloader-capture.exe"
run_status "$SCRIPT" --sources-only "$stage" "$SCRATCH/out5"
expect_status "a stage without the capture agent" 1
expect_output "naming the missing file" "has no rdownloader-capture.exe"

finish_tests package-msi
