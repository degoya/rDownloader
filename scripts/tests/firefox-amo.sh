#!/usr/bin/env bash
#
# scripts/firefox-amo.sh against stub `curl` and `npx` (RD-160-07, RD-170-10): without
# credentials it warns and succeeds; a version AMO does not know is submitted to the listing
# without waiting for the review; one AMO has already is not uploaded again — reported when it is
# listed, a warning when it is unlisted; every other answer ends in a warning and exit 0, never a
# failed release; the secret appears in no output and no command line.
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
cat > "$SCRATCH/ext/manifest.json" <<'EOF2'
{ "version": "1.2.3", "browser_specific_settings": { "gecko": { "id": "rdownloader@degoya.de" } } }
EOF2

# The stub answers the version query from $FAKE/version.status and version.json. It records
# every argument, and the header file's first word.
cat > "$SCRATCH/bin/curl" <<'EOF2'
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
    *) printf 404 ;;
esac
EOF2
# web-ext: `sign` succeeds unless $FAKE/sign.fails exists; a listed submission returns no file.
cat > "$SCRATCH/bin/npx" <<'EOF2'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/calls"
echo "key=${WEB_EXT_API_KEY:-} secret=${WEB_EXT_API_SECRET:+set}" >> "$FAKE/npx.env"
[[ "$3" == "sign" ]] || exit 0
[[ -f "$FAKE/sign.fails" ]] && { echo "WebExtError: Version 1.2.3 already exists." >&2; exit 1; }
exit 0
EOF2
chmod +x "$SCRATCH/bin/curl" "$SCRATCH/bin/npx"
export PATH="$SCRATCH/bin:$PATH"
export AMO_BASE_URL="https://amo.test/api/v5/"
SECRET="s3cr3t-value-never-printed"

reset() { rm -f "$FAKE"/calls "$FAKE"/urls "$FAKE"/headers "$FAKE"/npx.env "$FAKE"/version.* "$FAKE"/sign.fails; }
submit() { run_status env AMO_JWT_ISSUER=user:42:7 AMO_JWT_SECRET="$SECRET" "$ROOT/scripts/firefox-amo.sh" submit "$SCRATCH/ext"; }
version() { echo "$1" > "$FAKE/version.status"; [[ $# -lt 2 ]] || echo "$2" > "$FAKE/version.json"; }
leaks() { grep -rqF -- "$SECRET" "$FAKE" || grep -qF -- "$SECRET" <<< "$output"; }

reset
run_status env -u AMO_JWT_ISSUER -u AMO_JWT_SECRET "$ROOT/scripts/firefox-amo.sh" submit "$SCRATCH/ext"
expect_status "without credentials" 0
expect_output "warns" "::warning::Firefox extension not submitted to AMO: AMO_JWT_ISSUER or AMO_JWT_SECRET is not set"
expect_true "and asks nobody" '[[ ! -e "$FAKE/calls" ]]'

reset
version 404
submit
expect_status "a version AMO does not know" 0
expect_true "is asked for by the add-on ID and v-prefixed version" 'grep -qx "https://amo.test/api/v5/addons/addon/rdownloader%40degoya.de/versions/v1.2.3/" "$FAKE/urls"'
expect_true "with a JWT from a header file" 'grep -qx "Authorization: JWT" "$FAKE/headers"'
expect_true "is submitted listed with the pinned web-ext" 'grep -q "^--yes web-ext@[0-9.]* sign --channel listed --source-dir $SCRATCH/ext" "$FAKE/calls"'
expect_true "without waiting for the review" 'grep -q "sign --channel listed .* --approval-timeout 0 --no-config-discovery" "$FAKE/calls"'
expect_true "credentials through the environment" 'grep -qx "key=user:42:7 secret=set" "$FAKE/npx.env"'
expect_output "says the review decides" "submitted 1.2.3 to the listing"
expect_true "the secret is in no output and no argument" '! leaks'

reset
version 200 '{"channel":"listed","file":{"status":"unreviewed"}}'
submit
expect_status "a version already listed" 0
expect_output "says so" "1.2.3 is on AMO already (listed, file 'unreviewed'); nothing to upload"
expect_true "uploads nothing" '! grep -q " sign " "$FAKE/calls"'
expect_true "the secret is in no output and no argument" '! leaks'

reset
version 200 '{"channel":"unlisted","file":{"status":"public"}}'
submit
expect_status "a version signed unlisted" 0
expect_output "warns that the listing gets the next one" "AMO has 1.2.3 in the unlisted channel; a version number exists once, so the listing gets the next version"
expect_true "uploads nothing" '! grep -q " sign " "$FAKE/calls"'

reset
version 401
submit
expect_status "refused credentials" 0
expect_output "warn" "AMO refused the credentials or this add-on (HTTP 401)"

reset
version 404
touch "$FAKE/sign.fails"
submit
expect_status "a failed submission" 0
expect_output "warns" "web-ext sign failed"
expect_true "the secret is in no output and no argument" '! leaks'

reset
run_status "$ROOT/scripts/firefox-amo.sh" lint "$SCRATCH/ext"
expect_status "lint" 0
expect_true "runs web-ext lint on the build" 'grep -q "^--yes web-ext@[0-9.]* lint --source-dir $SCRATCH/ext --no-config-discovery" "$FAKE/calls"'

run_status "$ROOT/scripts/firefox-amo.sh" submit
expect_status "a call without the build" 2
run_status "$ROOT/scripts/firefox-amo.sh" sign "$SCRATCH/ext" "$SCRATCH/out.xpi"
expect_status "the old sign call" 2

finish_tests "firefox-amo"
