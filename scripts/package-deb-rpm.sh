#!/usr/bin/env bash
#
# Builds the deb and the rpm package of one Linux architecture from its release tarball
# (RD-180-05), with nfpm and packaging/linux/nfpm.yaml.in. The packages hold exactly the tarball's
# binaries, plugins and VERSION.txt, so they are what the tarball is, installed: the program in
# /usr/lib/rdownloader, the commands linked into /usr/bin, systemd user units for the service and
# the capture agent, a menu entry with its icon (RD-1120-20), and an install-kind marker (`deb`,
# `rpm`) that moves the data into ~/.local/share/rdownloader. The version is the one VERSION.txt
# names.
#
# Usage:
#   scripts/package-deb-rpm.sh <rdownloader-linux-ARCH.tar.gz> <out-dir>
#   scripts/package-deb-rpm.sh --render-only <tarball> <out-dir>   # the nfpm configs only
#
# Writes <out-dir>/rdownloader-linux-ARCH.deb and .rpm. Needs nfpm (release.yml installs a pinned
# one; docs/development.md names it), except with --render-only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
render_only=0
if [[ "${1:-}" == --render-only ]]; then
    render_only=1
    shift
fi
archive="${1:?usage: scripts/package-deb-rpm.sh [--render-only] <rdownloader-linux-ARCH.tar.gz> <out-dir>}"
out="${2:?usage: scripts/package-deb-rpm.sh [--render-only] <rdownloader-linux-ARCH.tar.gz> <out-dir>}"

# nfpm names architectures as Go does and writes each format's own name into the package.
case "$(basename "$archive")" in
    rdownloader-linux-x86_64.tar.gz) arch=x86_64 nfpm_arch=amd64 ;;
    rdownloader-linux-aarch64.tar.gz) arch=aarch64 nfpm_arch=arm64 ;;
    *)
        echo "not a Linux release tarball: $archive (rdownloader-linux-x86_64.tar.gz or -aarch64)" >&2
        exit 2
        ;;
esac
if [[ "$render_only" -eq 0 ]] && ! command -v nfpm > /dev/null; then
    echo "nfpm is not installed; see docs/development.md (Linux packages)" >&2
    exit 1
fi

# shellcheck source=lib/archive-layout.sh
source "$ROOT/scripts/lib/archive-layout.sh"
rd_check_archive_layout "$archive" linux

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/stage" "$out"
tar -xzf "$archive" -C "$work/stage"
version="$(sed -n '1s/^rDownloader //p' "$work/stage/VERSION.txt")"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
    echo "$archive: VERSION.txt names no version ('$version')" >&2
    exit 1
fi
if ! grep -qx 'profile  release' "$work/stage/VERSION.txt"; then
    echo "!! $archive is a test package (VERSION.txt names no release profile); packaging it anyway" >&2
fi

for kind in deb rpm; do
    printf '%s\n' "$kind" > "$work/install-kind.$kind"
    sed -e "s|@VERSION@|$version|g" -e "s|@ARCH@|$nfpm_arch|g" \
        -e "s|@STAGE@|$work/stage|g" -e "s|@KIND_FILE@|$work/install-kind.$kind|g" \
        -e "s|@PACKAGING@|$ROOT/packaging/linux|g" -e "s|@SYSTEMD@|$ROOT/packaging/systemd|g" \
        -e "s|@ICONS@|$ROOT/web/public|g" \
        "$ROOT/packaging/linux/nfpm.yaml.in" > "$work/nfpm-$kind.yaml"
    if grep -q '@[A-Z_]*@' "$work/nfpm-$kind.yaml"; then
        echo "packaging/linux/nfpm.yaml.in has a placeholder this script does not fill" >&2
        exit 1
    fi
    if [[ "$render_only" -eq 1 ]]; then
        cp "$work/nfpm-$kind.yaml" "$out/nfpm-$kind-$arch.yaml"
        continue
    fi
    target="$out/rdownloader-linux-$arch.$kind"
    rm -f "$target"
    nfpm package --config "$work/nfpm-$kind.yaml" --packager "$kind" --target "$target"
    echo "    $target ($(stat -c %s "$target") bytes, rdownloader $version)"
done
