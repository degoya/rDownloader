#!/usr/bin/env bash
#
# scripts/firefox-amo.sh against stub `curl` and `npx` (RD-160-07): without credentials it warns
# and succeeds; a version AMO does not know is signed unlisted; one AMO has signed already is
# downloaded and checked against AMO's hash instead of uploaded again; every other answer ends
# in a warning and exit 0, never a failed release; the secret appears in no output and no
# command line.
#
#   scripts/tests/firefox-amo.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin" "$SCRATCH/ext"
cat > "$SCRATCH/ext/manifest.json" <<'EOF'
{ "version": "1.2.3", "browser_specific_settings": { "gecko": { "id": "rdownloader@degoya.de" } } }
EOF
printf 'signed by the store' > "$FAKE/store.xpi"
STORE_HASH="$(sha256sum "$FAKE/store.xpi" | cut -d' ' -f1)"
export STORE_HASH

# The stub answers by URL: $FAKE/version.status and version.json for the version query, the
# store file for its download. It records every argument, and the header file's first word.
cat > "$SCRATCH/bin/curl" <<'EOF'
#!/usr/bin/env bash
out=""; header=""; url=""
while [[ $# -gt 0 ]]; do
    echo "$1" >> "$FAKE/calls"
    case "$1" in
        -o) out="$2"; echo "$2" >> "$FAKE/calls"; shift ;;
        -w) echo "$2" >> "$FAKE/calls"; shift ;;
        -H) [[ "$2" == @* ]] && header="${2#@}"; echo "$2" >> "$FAKE/calls"; shift ;;
        http*) url="$1" ;;
    esac
    shift
done
[[ -n "$header" ]] && cut -d' ' -f1-2 "$header" >> "$FAKE/headers"
echo "$url" >> "$FAKE/urls"
case "$url" in
    */versions/v1.2.3/)
        [[ -f "$FAKE/version.json" ]] && cp "$FAKE/version.json" "$out"
        printf '%s' "$(cat "$FAKE/version.status")"
        ;;
    https://amo.test/firefox/downloads/file/1/store.xpi)
        cp "$FAKE/store.xpi" "$out"; printf 200 ;;
    *) printf 404 ;;
esac
EOF
# web-ext: `sign` writes one .xpi into --artifacts-dir unless $FAKE/sign.fails exists.
cat > "$SCRATCH/bin/npx" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/calls"
echo "key=${WEB_EXT_API_KEY:-} secret=${WEB_EXT_API_SECRET:+set}" >> "$FAKE/npx.env"
[[ "$3" == "sign" ]] || exit 0
[[ -f "$FAKE/sign.fails" ]] && { echo "WebExtError: Version 1.2.3 already exists." >&2; exit 1; }
while [[ $# -gt 0 ]]; do
    [[ "$1" == "--artifacts-dir" ]] && printf 'signed unlisted' > "$2/rdownloader-1.2.3.xpi"
    shift
done
EOF
chmod +x "$SCRATCH/bin/curl" "$SCRATCH/bin/npx"
export PATH="$SCRATCH/bin:$PATH"
export AMO_BASE_URL="https://amo.test/api/v5/"
SECRET="s3cr3t-value-never-printed"
OUT="$SCRATCH/rdownloader-firefox.xpi"

reset() { rm -f "$FAKE"/calls "$FAKE"/urls "$FAKE"/headers "$FAKE"/npx.env "$FAKE"/version.* "$FAKE"/sign.fails; }
sign() { run_status env AMO_JWT_ISSUER=user:42:7 AMO_JWT_SECRET="$SECRET" "$ROOT/scripts/firefox-amo.sh" sign "$SCRATCH/ext" "$OUT"; }
version() { echo "$1" > "$FAKE/version.status"; [[ $# -lt 2 ]] || echo "$2" > "$FAKE/version.json"; }
leaks() { grep -rqF -- "$SECRET" "$FAKE" || grep -qF -- "$SECRET" <<< "$output"; }

reset
run_status env -u AMO_JWT_ISSUER -u AMO_JWT_SECRET "$ROOT/scripts/firefox-amo.sh" sign "$SCRATCH/ext" "$OUT"
expect_status "without credentials" 0
expect_output "warns" "::warning::Firefox extension not signed: AMO_JWT_ISSUER or AMO_JWT_SECRET is not set"
expect_true "writes no .xpi" '[[ ! -e "$OUT" ]]'
expect_true "and asks nobody" '[[ ! -e "$FAKE/calls" ]]'

reset
version 404
sign
expect_status "a version AMO does not know" 0
expect_true "is asked for by the add-on ID and v-prefixed version" 'grep -qx "https://amo.test/api/v5/addons/addon/rdownloader%40degoya.de/versions/v1.2.3/" "$FAKE/urls"'
expect_true "with a JWT from a header file" 'grep -qx "Authorization: JWT" "$FAKE/headers"'
expect_true "is signed unlisted with the pinned web-ext" 'grep -q "^--yes web-ext@[0-9.]* sign --channel unlisted --source-dir $SCRATCH/ext" "$FAKE/calls"'
expect_true "credentials through the environment" 'grep -qx "key=user:42:7 secret=set" "$FAKE/npx.env"'
expect "lands at the requested path" "signed unlisted" "$(cat "$OUT" 2> /dev/null)"
expect_true "the secret is in no output and no argument" '! leaks'

reset
version 200 "{\"channel\":\"listed\",\"file\":{\"status\":\"public\",\"url\":\"https://amo.test/firefox/downloads/file/1/store.xpi\",\"hash\":\"sha256:$STORE_HASH\"}}"
sign
expect_status "a version the store has signed" 0
expect_output "says so" "is on AMO already (listed channel, signed)"
expect "uses the store's file" "signed by the store" "$(cat "$OUT" 2> /dev/null)"
expect_true "uploads nothing" '! grep -q " sign " "$FAKE/calls"'
expect_true "the secret is in no output and no argument" '! leaks'

reset
version 200 '{"channel":"listed","file":{"status":"public","url":"https://amo.test/firefox/downloads/file/1/store.xpi","hash":"sha256:0000"}}'
sign
expect_status "a download that does not match AMO's hash" 0
expect_output "warns" "does not match AMO's hash"
expect_true "keeps no .xpi" '[[ ! -e "$OUT" ]]'

reset
version 200 '{"channel":"listed","file":{"status":"public","url":"https://elsewhere.test/store.xpi","hash":"sha256:0000"}}'
sign
expect_status "a download URL on another host" 0
expect_true "gets no token" '! grep -q elsewhere.test "$FAKE/urls"'

reset
version 200 '{"channel":"listed","file":{"status":"unreviewed","url":"","hash":""}}'
sign
expect_status "a version awaiting review" 0
expect_output "warns with channel and status" "AMO has 1.2.3 (listed channel) but its file is 'unreviewed', not signed"
expect_true "uploads nothing" '! grep -q " sign " "$FAKE/calls"'
expect_true "writes no .xpi" '[[ ! -e "$OUT" ]]'

reset
version 401
sign
expect_status "refused credentials" 0
expect_output "warn" "AMO refused the credentials or this add-on (HTTP 401)"

reset
version 404
touch "$FAKE/sign.fails"
sign
expect_status "a failed signing" 0
expect_output "warns" "web-ext sign failed"
expect_true "writes no .xpi" '[[ ! -e "$OUT" ]]'

reset
run_status "$ROOT/scripts/firefox-amo.sh" lint "$SCRATCH/ext"
expect_status "lint" 0
expect_true "runs web-ext lint on the build" 'grep -q "^--yes web-ext@[0-9.]* lint --source-dir $SCRATCH/ext --no-config-discovery" "$FAKE/calls"'

run_status "$ROOT/scripts/firefox-amo.sh" sign "$SCRATCH/ext"
expect_status "a call without the output path" 2

finish_tests "firefox-amo"
