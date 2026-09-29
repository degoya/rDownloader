#!/usr/bin/env bash
#
# The Homebrew formula and the Scoop manifest of one release (RD-180-06), rendered from
# packaging/homebrew/rdownloader.rb.in and packaging/scoop/rdownloader.json.in with the version
# and the archives' SHA-256 from the release's SHA256SUMS, and the README of the tap and of the
# bucket from the README.md.in beside each.
#
# Usage:
#   scripts/package-managers.sh <version> <SHA256SUMS> <outdir> [--repository OWNER/NAME]
#                               [--base-url URL]
#
# Writes <outdir>/rdownloader.rb (the tap's Formula/rdownloader.rb), <outdir>/rdownloader.json
# (the bucket's bucket/rdownloader.json), <outdir>/homebrew-README.md and
# <outdir>/scoop-README.md (each repository's README.md). The release workflow pushes them after
# the release is published; ci.yml renders them against archives made from the tree and installs
# the formula and the manifest.
#
#   --repository   the GitHub repository whose releases they install, default degoya/rDownloader;
#                  the tap and the bucket are <owner>/homebrew-rdownloader and
#                  <owner>/scoop-rdownloader
#   --base-url     where the archives are downloaded from, default
#                  https://github.com/<repository>/releases/download/v<version>; CI points it at
#                  a local fixture
#
# Every archive the two files name must be in SHA256SUMS (`<sha256>  ./<name>`, as the release
# writes it, or `<sha256>  <name>`), and nothing of a template may stay unreplaced.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Bash 5.2 would put the matched text in place of a `&` in a substitution's replacement.
shopt -u patsub_replacement 2> /dev/null || true

usage() {
    echo "usage: $0 <version> <SHA256SUMS> <outdir> [--repository OWNER/NAME] [--base-url URL]" >&2
    exit 2
}

[[ $# -ge 3 ]] || usage
version="${1#v}"
sums="$2"
outdir="$3"
shift 3
repository="degoya/rDownloader"
base_url=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --repository) [[ $# -ge 2 ]] || usage; repository="$2"; shift 2 ;;
        --base-url) [[ $# -ge 2 ]] || usage; base_url="$2"; shift 2 ;;
        *) usage ;;
    esac
done

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "error: '$version' is not a release version (X.Y.Z or X.Y.Z-suffix)" >&2
    exit 1
fi
if [[ ! "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
    echo "error: '$repository' is not OWNER/NAME" >&2
    exit 1
fi
[[ -f "$sums" ]] || { echo "error: $sums does not exist" >&2; exit 1; }
owner="${repository%%/*}"
base_url="${base_url:-https://github.com/${repository}/releases/download/v${version}}"
base_url="${base_url%/}"

# macOS's /bin/bash is 3.2: no associative arrays, so KEY=value pairs and a lookup by awk.
hash_of() {
    awk -v wanted="$1" '{
        name = $2
        sub(/^\*/, "", name)
        sub(/^\.\//, "", name)
        if (name == wanted) { print tolower($1); exit }
    }' "$sums"
}

values=(
    "VERSION=$version"
    "REPOSITORY=$repository"
    "OWNER=$owner"
    "BASE_URL=$base_url"
    "TAP=${owner}/homebrew-rdownloader"
    "BUCKET=${owner}/scoop-rdownloader"
)
# One placeholder per archive the two files install.
missing=0
for pair in \
    SHA256_MACOS_AARCH64=rdownloader-macos-aarch64.tar.gz \
    SHA256_MACOS_X86_64=rdownloader-macos-x86_64.tar.gz \
    SHA256_LINUX_AARCH64=rdownloader-linux-aarch64.tar.gz \
    SHA256_LINUX_X86_64=rdownloader-linux-x86_64.tar.gz \
    SHA256_WINDOWS_X86_64=rdownloader-windows-x86_64.zip; do
    archive="${pair#*=}"
    hash="$(hash_of "$archive")"
    if [[ ! "$hash" =~ ^[0-9a-f]{64}$ ]]; then
        echo "error: $sums has no SHA-256 for $archive" >&2
        missing=1
        continue
    fi
    values+=("${pair%%=*}=$hash")
done
[[ "$missing" -eq 0 ]] || exit 1

render() {
    local template="$1" output="$2" content pair
    content="$(< "$template")"
    for pair in "${values[@]}"; do
        content="${content//@${pair%%=*}@/${pair#*=}}"
    done
    if [[ "$content" =~ @[A-Z0-9_]+@ ]]; then
        echo "error: $template keeps ${BASH_REMATCH[0]} after rendering" >&2
        exit 1
    fi
    printf '%s\n' "$content" > "$output"
    echo "wrote $output"
}

mkdir -p "$outdir"
render "$ROOT/packaging/homebrew/rdownloader.rb.in" "$outdir/rdownloader.rb"
render "$ROOT/packaging/scoop/rdownloader.json.in" "$outdir/rdownloader.json"
render "$ROOT/packaging/homebrew/README.md.in" "$outdir/homebrew-README.md"
render "$ROOT/packaging/scoop/README.md.in" "$outdir/scoop-README.md"
