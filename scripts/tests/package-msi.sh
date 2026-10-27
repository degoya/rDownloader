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
# --capture (RD-1390-04): the capture agent's installer from the agent's zip as
# `scripts/release-assets.sh agent` writes it and from `agent-stage`, with its own source and
# strings, no service and no plugins. Its source detects the full product and refuses to install
# beside it, reads the agent's autostart entry, shows its checkbox on a first install only, and the
# full product's Upgrade table removes it by its UpgradeCode.
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
# Every string each source names, in every language, and nothing else: <culture>.wxl for
# rdownloader.wxs, capture-<culture>.wxl for rdownloader-capture.wxs.
for pair in ":rdownloader.wxs" "capture-:rdownloader-capture.wxs"; do
    run_status python3 - "$ROOT/packaging/msi" "${pair%%:*}" "${pair#*:}" <<'PY'
import pathlib, re, sys, xml.etree.ElementTree as tree
folder, prefix, source = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
wanted = set(re.findall(r"!\(loc\.([A-Za-z]+)\)", (folder / source).read_text()))
cultures = sorted(path.stem[len(prefix):] for path in folder.glob(f"{prefix}*.wxl")
                  if re.fullmatch(r"[a-z]{2}-[A-Z]{2}", path.stem[len(prefix):]))
assert cultures == ["de-DE", "en-US", "es-ES", "fr-FR"], cultures
for culture in cultures:
    root = tree.parse(folder / f"{prefix}{culture}.wxl").getroot()
    assert root.get("Culture") == culture, culture
    ids = {string.get("Id") for string in root if string.get("Value")}
    assert ids == wanted, (culture, sorted(ids ^ wanted))
PY
    expect_status "every ${pair%%:*}<culture>.wxl defines the strings ${pair#*:} names" 0
done
for source in "$fragment" "$ROOT/packaging/msi/rdownloader.wxs" "$ROOT/packaging/msi/rdownloader-capture.wxs"; do
    run_status python3 -c 'import sys, xml.etree.ElementTree as tree; tree.parse(sys.argv[1])' "$source"
    expect_status "$(basename "$source") is well-formed XML" 0
done
expect "the checked-in source keeps its UpgradeCode" 1 \
    "$(grep -c 'UpgradeCode="3346ED88-7297-43CC-9BA2-0478E5E2D851"' "$ROOT/packaging/msi/rdownloader.wxs")"
# The data folder is the service's; neither installer may name it.
for source in rdownloader.wxs rdownloader-capture.wxs; do
    expect "$source never names the data folder" 0 \
        "$(grep -c 'LOCALAPPDATA%\\rDownloader\\\|AppData\\Local\\rDownloader' "$ROOT/packaging/msi/$source" || true)"
done

# The capture agent's installer, from the agent's zip as the release writes it.
mkdir -p "$SCRATCH/assets" "$SCRATCH/capture-stage"
(cd "$stage" && zip -q -r "$SCRATCH/assets/rdownloader-windows-x86_64.zip" .)
run_status "$ROOT/scripts/release-assets.sh" agent "$SCRATCH/assets"
expect_status "the agent's zip for the capture stage" 0
unzip -q "$SCRATCH/assets/rdownloader-capture-windows-x86_64.zip" -d "$SCRATCH/capture-stage"
run_status "$SCRIPT" --sources-only --capture "$SCRATCH/capture-stage" "$SCRATCH/out-capture"
expect_status "the capture sources from the agent's zip" 0
expect_output "naming the agent's versions and no plugin" "rdownloader-capture 1.8.0-beta.1, MSI 1.8.0 en-US, 0 plugins"
arguments="$SCRATCH/out-capture/wix-arguments.txt"
expect "the capture build compiles rdownloader-capture.wxs alone" "rdownloader-capture.wxs" \
    "$(grep '\.wxs$' "$arguments" | xargs -n1 basename | paste -sd' ')"
expect "the capture build takes its own strings" "capture-en-US.wxl" \
    "$(basename "$(grep -A1 '^-loc$' "$arguments" | sed -n '2p')")"
expect "the capture marker says msi" "msi" "$(cat "$SCRATCH/out-capture/install-kind")"
expect_true "the capture sources carry no plugins fragment" "[[ ! -e '$SCRATCH/out-capture/plugins.wxs' ]]"
run_status "$ROOT/scripts/release-assets.sh" agent-stage "$stage" "$SCRATCH/agent-stage"
expect_status "agent-stage over the unpacked application zip" 0
expect "agent-stage copies what the agent's zip holds" \
    "$(cd "$SCRATCH/capture-stage" && find . -type f | sort | paste -sd' ')" \
    "$(cd "$SCRATCH/agent-stage" && find . -type f | sort | paste -sd' ')"
for language in de:de-DE es:es-ES fr:fr-FR; do
    run_status "$SCRIPT" --capture --sources-only "$SCRATCH/agent-stage" "$SCRATCH/out-capture-${language%%:*}" "${language%%:*}"
    expect_status "the ${language%%:*} capture sources" 0
    expect "the ${language%%:*} capture build takes capture-${language#*:}.wxl" "capture-${language#*:}.wxl" \
        "$(basename "$(grep -A1 '^-loc$' "$SCRATCH/out-capture-${language%%:*}/wix-arguments.txt" | sed -n '2p')")"
done
run_status "$SCRIPT" --sources-only --capture "$stage" "$SCRATCH/out-capture-full"
expect_status "the capture installer refuses the application's zip" 1
expect_output "naming why" "holds rdownloader.exe"
rm "$SCRATCH/agent-stage/start-rdownloader.bat"
run_status "$SCRIPT" --sources-only --capture "$SCRATCH/agent-stage" "$SCRATCH/out-capture-short"
expect_status "a capture stage without the script start-capture.bat calls" 1
expect_output "naming the missing file" "has no start-rdownloader.bat"
run_status "$SCRIPT" --sources-only --portable "$stage" "$SCRATCH/out-option"
expect_status "an unknown option" 2

# What the capture source declares, and what the full product's declares about it.
run_status python3 - "$ROOT/packaging/msi" <<'PY'
import pathlib, sys, xml.etree.ElementTree as tree
folder = pathlib.Path(sys.argv[1])
ns = {"w": "http://wixtoolset.org/schemas/v4/wxs"}
file_tag = "{%s}File" % ns["w"]
capture_text = (folder / "rdownloader-capture.wxs").read_text()
capture = tree.parse(folder / "rdownloader-capture.wxs").getroot().find("w:Package", ns)
full = tree.parse(folder / "rdownloader.wxs").getroot().find("w:Package", ns)
code, full_code = capture.get("UpgradeCode"), full.get("UpgradeCode")
assert code and code != full_code, code
files = {f.get("Source").split("\\")[-1] for f in capture.iter(file_tag)}
assert "rdownloader-capture.exe" in files and "rdownloader.exe" not in files, files
assert any(f.get("Name") == "install-kind" for f in capture.iter(file_tag))
assert "rdownloader.exe" not in capture_text
folder_name = capture.find(".//w:Directory[@Id='INSTALLFOLDER']", ns).get("Name")
assert folder_name == "rDownloader Capture", folder_name
# Beside the full product: detected, never removed, and a launch condition refuses the install.
detect = capture.find(f"w:Upgrade[@Id='{full_code}']/w:UpgradeVersion", ns)
assert detect.get("OnlyDetect") == "yes" and detect.get("Property") == "FULLPRODUCTFOUND", detect.attrib
launch = capture.find("w:Launch", ns)
assert "NOT FULLPRODUCTFOUND" in launch.get("Condition") and launch.get("Message") == "!(loc.FullProductPresent)"
# The autostart entry rd-autostart writes, read before the upgrade.
search = capture.find("w:Property[@Id='CAPTUREAUTOSTARTREG']/w:RegistrySearch", ns)
assert (search.get("Root"), search.get("Key"), search.get("Name")) == (
    "HKCU", r"Software\Microsoft\Windows\CurrentVersion\Run", "rDownloader Capture"), search.attrib
# The checkbox: checked, its text only on a first install, no static text to show it on an upgrade.
assert capture.find("w:Property[@Id='WIXUI_EXITDIALOGOPTIONALCHECKBOX']", ns).get("Value") == "1"
assert capture.find("w:Property[@Id='WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT']", ns) is None
text = capture.find("w:SetProperty[@Id='WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT']", ns)
assert text.get("Condition") == "NOT Installed AND NOT WIX_UPGRADE_DETECTED", text.attrib
publishes = capture.findall("w:UI/w:Publish", ns)
launches = [p for p in publishes if p.get("Value") == "LaunchAgent"]
assert len(launches) == 1, [p.attrib for p in publishes]
assert launches[0].get("Condition") == "NOT Installed AND (NOT WIX_UPGRADE_DETECTED OR CAPTUREAUTOSTARTREG)"
for p in publishes:
    if "autostart install" in (p.get("Value") or "") or p.get("Value") == "AutostartFromDialog":
        assert "WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT AND" in p.get("Condition"), p.attrib
# The silent switches and the uninstall's clean-up, as in the full product.
for name in ("AUTOSTART", "ASSOCIATION", "DROPSHORTCUT", "SENDTO"):
    assert capture.find(f"w:Property[@Id='{name}']", ns).get("Secure") == "yes", name
customs = {c.get("Action"): c.get("Condition") for c in capture.findall("w:InstallExecuteSequence/w:Custom", ns)}
for action in ("AutostartRemove", "AssociationRemove", "DropShortcutRemove", "SendToRemove"):
    assert customs[action] == 'REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE', (action, customs[action])
for action, switch in (("AutostartInstall", "AUTOSTART"), ("AssociationInstall", "ASSOCIATION"),
                       ("DropShortcutInstall", "DROPSHORTCUT"), ("SendToInstall", "SENDTO")):
    assert customs[action] == f'{switch}="1" AND NOT REMOVE', (action, customs[action])
# The full product removes the capture package (no OnlyDetect) and registers again what it left.
remove = full.find(f"w:Upgrade[@Id='{code}']/w:UpgradeVersion", ns)
assert remove is not None and remove.get("OnlyDetect") is None, "the full product's Upgrade row"
assert remove.get("Property") == "CAPTUREPRODUCTFOUND" and remove.get("Minimum") == "0.0.0", remove.attrib
full_customs = {c.get("Action"): c.get("Condition") for c in full.findall("w:InstallExecuteSequence/w:Custom", ns)}
assert full_customs["CaptureAutostartInstall"] == "CAPTUREPRODUCTFOUND AND CAPTUREAUTOSTARTREG AND NOT REMOVE"
for switch in ("ASSOCIATION", "DROPSHORTCUT", "SENDTO"):
    assert "CAPTUREPRODUCTFOUND" in full.find(f"w:SetProperty[@Id='{switch}']", ns).get("Condition"), switch
PY
expect_status "the capture source detects, refuses and is removed as designed" 0

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
