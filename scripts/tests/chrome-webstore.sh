#!/usr/bin/env bash
#
# scripts/chrome-webstore.sh against a stub `curl` (RD-170-10): without credentials it warns and
# succeeds; a new version is uploaded and submitted for review through API v2; one the store has
# already is left alone; an upload the store processes in the background is waited for; every
# refusal ends in a warning and exit 0, never a failed release; no secret and no token appears in
# any output or command line.
#
#   scripts/tests/chrome-webstore.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin" "$SCRATCH/ext"
echo '{ "manifest_version": 3, "version": "1.2.3" }' > "$SCRATCH/ext/manifest.json"
ZIP="$SCRATCH/rdownloader-chrome.zip"
(cd "$SCRATCH/ext" && zip -q "$ZIP" manifest.json)

# The stub answers by URL from $FAKE/<name>.status and <name>.json (token, status, upload,
# publish); a status query during a background upload takes $FAKE/poll.json once it exists. It
# records every argument, the header file's first two words and the form files' contents.
cat > "$SCRATCH/bin/curl" <<'EOF'
#!/usr/bin/env bash
out=""; url=""
while [[ $# -gt 0 ]]; do
    echo "$1" >> "$FAKE/calls"
    case "$1" in
        -o) out="$2"; echo "$2" >> "$FAKE/calls"; shift ;;
        -H) [[ "$2" == @* ]] && cut -d' ' -f1-2 "${2#@}" >> "$FAKE/headers"; echo "$2" >> "$FAKE/calls"; shift ;;
        --data-urlencode) [[ "$2" == *@* ]] && echo "${2%%@*}=$(cat "${2#*@}")" >> "$FAKE/form"; shift ;;
        http*) url="$1" ;;
    esac
    shift
done
echo "$url" >> "$FAKE/urls"
case "$url" in
    */token) name=token ;;
    *:fetchStatus) name=status; [[ -f "$FAKE/uploaded" && -f "$FAKE/poll.json" ]] && name=poll ;;
    */upload/v2/*:upload) name=upload; touch "$FAKE/uploaded" ;;
    *:publish) name=publish ;;
    *) printf 404; exit 0 ;;
esac
[[ -f "$FAKE/$name.json" ]] && cp "$FAKE/$name.json" "$out"
printf '%s' "$(cat "$FAKE/$name.status" 2> /dev/null || echo 200)"
EOF
chmod +x "$SCRATCH/bin/curl"
export PATH="$SCRATCH/bin:$PATH"
export CWS_TOKEN_URL="https://oauth.test/token"
export CWS_API_URL="https://cws.test/"
export CWS_POLL_SECONDS=0 CWS_POLL_TRIES=3
SECRET="s3cr3t-value-never-printed"
REFRESH="1//refresh-token-never-printed"
TOKEN="ya29.access-token-never-printed"
# shellcheck disable=SC2034  # read inside the eval of expect_true()
ITEM="https://cws.test/v2/publishers/pub-7/items/nfdbhbkjnbdnaaekabaochlhgkaafnda"

reset() {
    rm -f "$FAKE"/*
    answer token 200 "{\"access_token\":\"$TOKEN\",\"expires_in\":3599}"
    answer status 200 '{"publishedItemRevisionStatus":{"state":"PUBLISHED","distributionChannels":[{"crxVersion":"1.2.2"}]}}'
    answer upload 200 '{"crxVersion":"1.2.3","uploadState":"SUCCEEDED"}'
    answer publish 200 '{"state":"PENDING_REVIEW"}'
}
answer() { echo "$2" > "$FAKE/$1.status"; echo "$3" > "$FAKE/$1.json"; }
store() {
    run_status env CWS_CLIENT_ID=client-1 CWS_CLIENT_SECRET="$SECRET" CWS_REFRESH_TOKEN="$REFRESH" \
        CWS_PUBLISHER_ID=pub-7 "$ROOT/scripts/chrome-webstore.sh" "$1" "$ZIP"
}
leaks() {
    local value
    for value in "$SECRET" "$REFRESH" "$TOKEN"; do
        grep -rqF -- "$value" "$FAKE/calls" "$FAKE/urls" && return 0
        grep -qF -- "$value" <<< "$output" && return 0
    done
    return 1
}

reset
run_status env -u CWS_CLIENT_ID -u CWS_CLIENT_SECRET -u CWS_REFRESH_TOKEN -u CWS_PUBLISHER_ID \
    "$ROOT/scripts/chrome-webstore.sh" publish "$ZIP"
expect_status "without credentials" 0
expect_output "warns" "::warning::Chrome Web Store not updated: CWS_CLIENT_ID is not set"
expect_true "and asks nobody" '[[ ! -e "$FAKE/calls" ]]'

reset
run_status env -u CWS_PUBLISHER_ID CWS_CLIENT_ID=client-1 CWS_CLIENT_SECRET="$SECRET" \
    CWS_REFRESH_TOKEN="$REFRESH" "$ROOT/scripts/chrome-webstore.sh" publish "$ZIP"
expect_status "without the publisher id" 0
expect_output "names it" "CWS_PUBLISHER_ID is not set"

reset
store publish
expect_status "a new version" 0
expect_true "trades the refresh token for an access token" 'grep -qx "https://oauth.test/token" "$FAKE/urls"'
expect_true "with the three values from files" 'grep -qx "client_id=client-1" "$FAKE/form" && grep -qx "client_secret=$SECRET" "$FAKE/form" && grep -qx "refresh_token=$REFRESH" "$FAKE/form"'
expect_true "asks for the item's state first" '[[ "$(sed -n 2p "$FAKE/urls")" == "$ITEM:fetchStatus" ]]'
expect_true "uploads the ZIP through API v2" 'grep -qx "https://cws.test/upload/v2/publishers/pub-7/items/nfdbhbkjnbdnaaekabaochlhgkaafnda:upload" "$FAKE/urls" && grep -qx -- "$ZIP" "$FAKE/calls"'
expect_true "submits it" 'grep -qx "$ITEM:publish" "$FAKE/urls"'
expect_true "with a bearer token from a header file" '[[ "$(sort -u "$FAKE/headers")" == "Authorization: Bearer" ]]'
expect_output "says it is in review" "submitted: 1.2.3 is PENDING_REVIEW"
expect_true "no secret or token in any output or argument" '! leaks'

reset
store upload
expect_status "upload alone" 0
expect_output "uploads" "uploaded 1.2.3"
expect_true "submits nothing" '! grep -q ":publish" "$FAKE/urls"'

reset
answer status 200 '{"submittedItemRevisionStatus":{"state":"PENDING_REVIEW","distributionChannels":[{"crxVersion":"1.2.3"}]}}'
store publish
expect_status "a version already submitted" 0
expect_output "says so" "1.2.3 is in the store already (submitted, PENDING_REVIEW)"
expect_true "uploads nothing" '! grep -q ":upload" "$FAKE/urls"'

reset
answer status 200 '{"submittedItemRevisionStatus":{"state":"REJECTED","distributionChannels":[{"crxVersion":"1.2.3"}]}}'
store publish
expect_status "a version the store rejected" 0
expect_true "is uploaded again" 'grep -q ":upload" "$FAKE/urls"'

reset
answer upload 200 '{"uploadState":"IN_PROGRESS"}'
answer poll 200 '{"lastAsyncUploadState":"SUCCEEDED"}'
store publish
expect_status "an upload processed in the background" 0
expect_true "is waited for, then submitted" 'grep -qx "$ITEM:publish" "$FAKE/urls"'

reset
answer upload 200 '{"uploadState":"IN_PROGRESS"}'
answer poll 200 '{"lastAsyncUploadState":"IN_PROGRESS"}'
store publish
expect_status "an upload that never finishes" 0
expect_output "warns after the wait" "still processing the upload"
expect_true "submits nothing" '! grep -q ":publish" "$FAKE/urls"'

reset
answer upload 400 '{"error":{"code":400,"message":"The version of the new package must be higher."}}'
store publish
expect_status "a refused upload" 0
expect_output "warns with the store's reason" "the upload answered HTTP 400: The version of the new package must be higher."
expect_true "submits nothing" '! grep -q ":publish" "$FAKE/urls"'

reset
answer upload 200 '{"uploadState":"FAILED"}'
store publish
expect_status "a failed upload" 0
expect_output "warns" "the store did not take the upload (FAILED)"

reset
answer token 400 '{"error":"invalid_grant","error_description":"Token has been expired or revoked."}'
store publish
expect_status "an expired refresh token" 0
expect_output "warns with the error code" "Google refused the refresh token (HTTP 400, invalid_grant)"
expect_true "asks the store nothing" '! grep -q "cws.test" "$FAKE/urls"'

reset
answer status 403 '{"error":{"code":403,"message":"The caller does not have permission"}}'
store publish
expect_status "a refused item" 0
expect_output "warns" "the store refused this account or item (HTTP 403: The caller does not have permission)"

reset
answer publish 200 '{"state":"REJECTED"}'
store publish
expect_status "a rejected submission" 0
expect_output "warns" "the store answered the submission with state REJECTED"
expect_true "no secret or token in any output or argument" '! leaks'

run_status "$ROOT/scripts/chrome-webstore.sh" publish
expect_status "a call without the ZIP" 2
echo 'not a zip' > "$SCRATCH/broken.zip"
run_status "$ROOT/scripts/chrome-webstore.sh" publish "$SCRATCH/broken.zip"
expect_status "a ZIP without a manifest version" 2

finish_tests "chrome-webstore"
