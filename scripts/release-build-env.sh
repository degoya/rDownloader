#!/usr/bin/env bash
#
# The build environment of a release binary, as KEY=VALUE lines for $GITHUB_ENV (RD-180-12).
# release.yml's binary jobs and repro.yml's two rebuilds both take it from here, so the published
# executable and its rebuilds are made the same way by construction, not by two copies kept in
# step. How the comparison uses it: docs/reproducible-builds.md.
#
# Every target gets the build stamp from the checked-out commit instead of the clock:
#   SOURCE_DATE_EPOCH  the commit's committer time; rust-embed takes the embedded web files'
#                      times from it
#   RD_VERSION         the workspace version in Cargo.toml
#   RD_BUILD_COMMIT    eight characters of the commit, what crates/rdownloader/build.rs would
#                      work out from a clean checkout; with --release-tag the line VERSION.txt and
#                      the About page carry for a release, `Release-Build X.Y.Z (Basis <sha>)`,
#                      from rd_build_stamp in scripts/lib/version-file.sh, which also stops on a
#                      tag that is not Cargo.toml's version (RD-180-05)
#   RD_BUILD_TIME      SOURCE_DATE_EPOCH in the About page's format
# A Linux target also gets its paths remapped, the checkout to /rdownloader, the cargo home to
# /cargo and the rustup home to /rustup: `--remap-path-prefix` through the target's own
# CARGO_TARGET_<TRIPLE>_RUSTFLAGS and `-ffile-prefix-map` for the C code of the -sys crates. Never
# through RUSTFLAGS, which would replace the `+crt-static` .cargo/config.toml gives Windows, and
# named for the target only, so build scripts and proc macros (host code that never reaches the
# binary) build unchanged. Windows and macOS are not remapped yet; their binaries have sources of
# difference of their own (docs/reproducible-builds.md, "What is not covered").
#
#   scripts/release-build-env.sh --target TRIPLE [--release-tag vX.Y.Z] [--source DIR] [--cargo-home DIR] >> "$GITHUB_ENV"
#
# --source is the checkout (default: the current directory), --cargo-home a cargo home to build
# with instead of $CARGO_HOME, created when missing and exported as CARGO_HOME.
set -euo pipefail

usage() {
    echo "usage: $0 --target TRIPLE [--release-tag vX.Y.Z] [--source DIR] [--cargo-home DIR]" >&2
    exit 2
}

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target=""
release_tag=""
source_dir="."
cargo_home=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) target="${2:-}"; shift 2 || usage ;;
        --release-tag) release_tag="${2:-}"; shift 2 || usage ;;
        --source) source_dir="${2:-}"; shift 2 || usage ;;
        --cargo-home) cargo_home="${2:-}"; shift 2 || usage ;;
        *) usage ;;
    esac
done
[[ -n "$target" && -n "$source_dir" ]] || usage

src="$(cd "$source_dir" && pwd -P)"
epoch="$(git -C "$src" log -1 --format=%ct HEAD)"
# GNU date on Linux and in Git for Windows' bash, BSD date on macOS.
built="$(date -u -d "@$epoch" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "$epoch" +%Y-%m-%dT%H:%M:%SZ)"

echo "SRC=$src"
if [[ -n "$cargo_home" ]]; then
    mkdir -p "$cargo_home"
    cargo_home="$(cd "$cargo_home" && pwd -P)"
    echo "CARGO_HOME=$cargo_home"
fi
version="$(sed -n '/^\[workspace\.package\]/,/^\[/p' "$src/Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -n 1)"
if [[ -n "$release_tag" ]]; then
    # In the checkout, whose git rd_build_stamp asks; a stamp already exported is not taken over.
    commit="$(
        cd "$src"
        unset RD_BUILD_COMMIT
        RD_BUILD_TIME="$built"
        RD_RELEASE_VERSION="${release_tag#v}"
        source "$here/lib/version-file.sh"
        rd_build_stamp "$version" || exit 1
        echo "$RD_BUILD_COMMIT"
    )"
else
    commit="$(git -C "$src" rev-parse --short=8 HEAD)"
fi
echo "SOURCE_DATE_EPOCH=$epoch"
echo "RD_VERSION=$version"
echo "RD_BUILD_COMMIT=$commit"
echo "RD_BUILD_TIME=$built"

case "$target" in
    *-unknown-linux-gnu) ;;
    *) exit 0 ;;
esac
cargo_home="${cargo_home:-${CARGO_HOME:-$HOME/.cargo}}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
# Disjoint prefixes, so which one wins where two would match never comes up.
remap="--remap-path-prefix=$src=/rdownloader --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$rustup_home=/rustup"
cmap="-ffile-prefix-map=$src=/rdownloader -ffile-prefix-map=$cargo_home=/cargo"
upper="$(tr '[:lower:]-' '[:upper:]_' <<< "$target")"
lower="${target//-/_}"
echo "CARGO_TARGET_${upper}_RUSTFLAGS=$remap"
echo "CFLAGS_${lower}=$cmap"
echo "CXXFLAGS_${lower}=$cmap"
