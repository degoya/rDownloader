#!/usr/bin/env bash
#
# The hand-over between release-plugins.yml's build jobs and its signing job (RD-1480-14, SEC-6):
# scripts/ci-plugins.sh `hand-over`, `digest` and `take-over` in a scratch checkout. The signing
# job takes the components and the packager only with the digests the build jobs put out, and
# only files named like a component; a link, a path or a changed byte stops it before anything is
# unpacked.
#
#   scripts/tests/ci-plugins.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

BUILD="$SCRATCH/build" SIGN="$SCRATCH/sign"
WASM=target/wasm32-unknown-unknown/release
mkdir -p "$BUILD/$WASM" "$BUILD/target/release-test" "$SIGN/target/release-test"
for name in http one_fichier unrelated; do echo "$name" > "$BUILD/$WASM/rd_plugin_$name.wasm"; done
echo stamp > "$BUILD/$WASM/rd_plugin_http.wasm.src-sha256"
echo packager > "$BUILD/target/release-test/rd-pack"
export GITHUB_OUTPUT="$SCRATCH/output"
: > "$GITHUB_OUTPUT"

in_dir() { (cd "$1" && shift && "$ROOT/scripts/ci-plugins.sh" "$@"); }
last_output() { sed -n 's/^sha256=//p' "$GITHUB_OUTPUT" | tail -n 1; }
take() { COMPONENTS_SHA256="$components" PACKAGER_SHA256="$packager" in_dir "$SIGN" take-over plugin-components.tar; }
deliver() {
    cp "$1" "$SIGN/plugin-components.tar"
    cp "$BUILD/target/release-test/rd-pack" "$SIGN/target/release-test/rd-pack"
    rm -rf "${SIGN:?}/$WASM"
}

run_status in_dir "$BUILD" hand-over plugin-components.tar http one-fichier
expect_status "hand-over" 0
components="$(last_output)"
expect "it puts out the archive's digest" "$(sha256sum "$BUILD/plugin-components.tar" | cut -c1-64)" "$components"
expect "the archive holds the named components alone" "rd_plugin_http.wasm rd_plugin_one_fichier.wasm" \
    "$(tar -tf "$BUILD/plugin-components.tar" | paste -sd' ')"
run_status in_dir "$BUILD" hand-over plugin-components.tar
expect_status "hand-over without a plugin" 1
run_status in_dir "$BUILD" hand-over elsewhere.tar missing
expect_true "a component that was not built fails the hand-over" '[[ $status -ne 0 ]]'
run_status in_dir "$BUILD" digest target/release-test/rd-pack
expect_status "digest" 0
packager="$(last_output)"

deliver "$BUILD/plugin-components.tar"
run_status take
expect_status "take-over" 0
expect "the components lie where package-signed looks" "http one_fichier" \
    "$(cat "$SIGN/$WASM"/rd_plugin_*.wasm | paste -sd' ')"
expect_true "the packager is executable" '[[ -x "$SIGN/target/release-test/rd-pack" ]]'

deliver "$BUILD/plugin-components.tar"
echo more >> "$SIGN/target/release-test/rd-pack"
run_status take
expect_status "another packager is refused" 1
expect_output "and named as such" "not what the build jobs made"
expect_true "before anything is unpacked" '[[ ! -e "$SIGN/$WASM" ]]'

deliver "$BUILD/plugin-components.tar"
echo more >> "$SIGN/plugin-components.tar"
run_status take
expect_status "another archive is refused" 1

deliver "$BUILD/plugin-components.tar"
for bad in "" "0123" "$components; true"; do
    run_status env COMPONENTS_SHA256="$bad" PACKAGER_SHA256="$packager" \
        bash -c 'cd "$1" && "$2" take-over plugin-components.tar' _ "$SIGN" "$ROOT/scripts/ci-plugins.sh"
    expect_status "a digest that is none is refused: '$bad'" 1
    expect_output "by its form" "no SHA-256 to compare with"
done

# With the right digest, but not only components: what a build job that was taken over could send.
evil="$SCRATCH/evil"
mkdir -p "$evil/sub"
ln -s /etc/passwd "$evil/rd_plugin_link.wasm"
echo x > "$evil/other.txt"
echo x > "$evil/sub/rd_plugin_sub.wasm"
for entry in rd_plugin_link.wasm other.txt sub/rd_plugin_sub.wasm; do
    tar -cf "$evil/a.tar" -C "$evil" "$entry"
    deliver "$evil/a.tar"
    components="$(sha256sum "$evil/a.tar" | cut -c1-64)"
    run_status take
    expect_status "$entry in the archive is refused" 1
    expect_output "as more than components" "holds more than components"
    expect_true "and not unpacked" '[[ ! -e "$SIGN/$WASM" ]]'
done

finish_tests "ci-plugins"
