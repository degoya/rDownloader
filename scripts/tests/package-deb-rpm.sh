#!/usr/bin/env bash
#
# scripts/package-deb-rpm.sh on a scratch release tarball (RD-180-05): the nfpm configs it renders
# name the tarball's version and the architecture nfpm expects, mark each format's install kind,
# install the menu entry and its icon from files the checkout has (RD-1120-20), and leave no
# placeholder; the menu entry names that icon and a command; a tarball in the old nested layout or of another platform is refused.
# With nfpm installed, the deb is built as well and its content checked with dpkg-deb when that
# is present; without them those two cases say so and pass.
#
#   scripts/tests/package-deb-rpm.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"
SCRIPT="$ROOT/scripts/package-deb-rpm.sh"

stage="$SCRATCH/stage"
mkdir -p "$stage/plugins"
while IFS= read -r entry; do echo "$entry" > "$stage/$entry"; done < <(rd_archive_entries linux)
chmod 755 "$stage/rdownloader" "$stage/rdownloader-capture"
printf 'rDownloader 1.8.0-beta.1\ncommit   Release-Build 1.8.0-beta.1 (Basis 0123abcd)\nbuilt    2026-09-30T00:00:00Z\nplatform linux aarch64\nprofile  release\n' \
    > "$stage/VERSION.txt"
echo package > "$stage/plugins/http-1.0.0.rdplug"
tar --directory "$stage" --create --gzip --file "$SCRATCH/rdownloader-linux-aarch64.tar.gz" .

run_status "$SCRIPT" --render-only "$SCRATCH/rdownloader-linux-aarch64.tar.gz" "$SCRATCH/out"
expect_status "rendering the configs of an aarch64 tarball" 0
for kind in deb rpm; do
    config="$SCRATCH/out/nfpm-$kind-aarch64.yaml"
    expect "the $kind config names the tarball's version" 'version: "1.8.0-beta.1"' \
        "$(grep '^version:' "$config")"
    expect "the $kind config names nfpm's architecture" 'arch: "arm64"' "$(grep '^arch:' "$config")"
    expect "the $kind config fills every placeholder" "" "$(grep -o '@[A-Z_]*@' "$config" || true)"
    marker="$(sed -n 's/^  - src: "\(.*install-kind\.[a-z]*\)"$/\1/p' "$config")"
    expect "the $kind config takes the $kind marker" "install-kind.$kind" "$(basename "$marker")"
    expect "the $kind config installs the marker beside the binaries" \
        "    dst: /usr/lib/rdownloader/install-kind" \
        "$(grep -A1 '^  - src: .*install-kind\.' "$config" | sed -n '2p')"
    for pair in rdownloader.desktop:/usr/share/applications/rdownloader.desktop \
        favicon.svg:/usr/share/icons/hicolor/scalable/apps/rdownloader.svg \
        icon-512.png:/usr/share/icons/hicolor/512x512/apps/rdownloader.png; do
        source="$(grep -B1 "^    dst: ${pair#*:}$" "$config" | sed -n 's/^  - src: "\(.*\)"$/\1/p')"
        expect "the $kind config installs ${pair#*:} from the checkout" "${pair%%:*}" \
            "$([[ "$source" == "$ROOT"/* && -s "$source" ]] && basename "$source")"
    done
done
run_status python3 - "$ROOT/packaging/linux/rdownloader.desktop" <<'PY'
import configparser, sys
entry = configparser.ConfigParser(interpolation=None, comment_prefixes=("#",))
entry.optionxform = str
entry.read(sys.argv[1], encoding="utf-8")
group = entry["Desktop Entry"]
assert group["Type"] == "Application", group["Type"]
assert group["Icon"] == "rdownloader", group["Icon"]
assert group["Terminal"] == "false"
assert "xdg-open http://127.0.0.1:8710/" in group["Exec"], group["Exec"]
for key in ("GenericName", "Comment"):
    for language in ("de", "es", "fr"):
        assert group[f"{key}[{language}]"], (key, language)
PY
expect_status "the menu entry opens the interface with the packaged icon, in four languages" 0
if command -v desktop-file-validate > /dev/null; then
    run_status desktop-file-validate "$ROOT/packaging/linux/rdownloader.desktop"
    expect_status "desktop-file-validate accepts the menu entry" 0
else
    echo "skip desktop-file-validate is not installed; the menu entry is not validated by it"
fi
expect "the rpm config asks for glibc's floor" "      - glibc >= 2.39" \
    "$(grep -F 'glibc >= 2.39' "$SCRATCH/out/nfpm-rpm-aarch64.yaml")"

mkdir -p "$SCRATCH/nested"
cp -r "$stage" "$SCRATCH/nested/linux"
tar --directory "$SCRATCH/nested" --create --gzip --file "$SCRATCH/nested/rdownloader-linux-x86_64.tar.gz" linux
run_status "$SCRIPT" --render-only "$SCRATCH/nested/rdownloader-linux-x86_64.tar.gz" "$SCRATCH/out2"
expect_status "a tarball in the layout package-linux.sh wrote until 1.8" 1

run_status "$SCRIPT" --render-only "$SCRATCH/rdownloader-macos-aarch64.tar.gz" "$SCRATCH/out3"
expect_status "a tarball of another platform" 2

if command -v nfpm > /dev/null; then
    cp "$SCRATCH/rdownloader-linux-aarch64.tar.gz" "$SCRATCH/rdownloader-linux-x86_64.tar.gz"
    run_status "$SCRIPT" "$SCRATCH/rdownloader-linux-x86_64.tar.gz" "$SCRATCH/packages"
    expect_status "nfpm builds both packages" 0
    if command -v dpkg-deb > /dev/null; then
        deb="$SCRATCH/packages/rdownloader-linux-x86_64.deb"
        expect "the deb's version sorts the pre-release first" "1.8.0~beta.1" \
            "$(dpkg-deb --field "$deb" Version)"
        expect "the deb depends on glibc's floor" "libc6 (>= 2.39), libgcc-s1" \
            "$(dpkg-deb --field "$deb" Depends)"
        run_status dpkg-deb --contents "$deb"
        for path in ./usr/lib/rdownloader/rdownloader ./usr/lib/rdownloader/install-kind \
            ./usr/lib/rdownloader/plugins/http-1.0.0.rdplug ./usr/lib/systemd/user/rdownloader.service \
            ./usr/lib/systemd/user/rdownloader-capture.service "./usr/bin/rdownloader -> /usr/lib/rdownloader/rdownloader" \
            ./usr/share/applications/rdownloader.desktop ./usr/share/icons/hicolor/scalable/apps/rdownloader.svg; do
            expect_output "the deb carries $path" "$path"
        done
        mkdir -p "$SCRATCH/unpacked"
        dpkg-deb --extract "$deb" "$SCRATCH/unpacked"
        expect "the deb's marker says deb" "deb" "$(cat "$SCRATCH/unpacked/usr/lib/rdownloader/install-kind")"
        expect "no maintainer script removes anything" "" \
            "$(dpkg-deb --info "$deb" postinst | grep -E '\brm\b|rmdir|userdel' || true)"
    else
        echo "skip dpkg-deb is not installed; the deb's content is not inspected"
    fi
else
    echo "skip nfpm is not installed; only the rendered configs are checked"
fi

finish_tests package-deb-rpm
