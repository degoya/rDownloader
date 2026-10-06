#!/usr/bin/env bash
#
# scripts/edge-addons.sh against a stub `curl` (RD-190-11): without credentials it warns and
# succeeds; a new version is uploaded, waited for — through HTTP 202 too — and submitted for
# certification through API v1.1; a version the store has already ends in "nothing to do"; every refusal ends in a warning
# and exit 0, never a failed release; no key appears in any output or command line.
#
#   scripts/tests/edge-addons.sh
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

# The stub answers by URL from $FAKE/<name>.status and <name>.json (upload, upload-op, publish,
# publish-op); an operation query takes <name>.2.json and <name>.2.status from its second call on
# when they exist.
# Accepted calls carry a Location header with the operation ID. It records every argument, the
# header file's lines with their values cut off, and the body it was sent.
cat > "$SCRATCH/bin/curl" <<'EOF'
#!/usr/bin/env bash
out=""; headers=""; url=""
while [[ $# -gt 0 ]]; do
    echo "$1" >> "$FAKE/calls"
    case "$1" in
        -o) out="$2"; echo "$2" >> "$FAKE/calls"; shift ;;
        -D) headers="$2"; shift ;;
        -H) [[ "$2" == @* ]] && sed -E 's/^(Authorization: ApiKey) .*/\1/; s/^(X-ClientID):.*/\1/' "${2#@}" >> "$FAKE/headers"
            echo "$2" >> "$FAKE/calls"; shift ;;
        --data-binary) [[ "$2" == @* ]] && cat "${2#@}" > "$FAKE/body"; shift ;;
        http*) url="$1" ;;
    esac
    shift
done
echo "$url" >> "$FAKE/urls"
case "$url" in
    */submissions/draft/package/operations/*) name=upload-op ;;
    */submissions/draft/package) name=upload; location=op-upload-1 ;;
    */submissions/operations/*) name=publish-op ;;
    */submissions) name=publish; location=op-publish-1 ;;
    *) printf 404; exit 0 ;;
esac
count=$(( $(cat "$FAKE/$name.count" 2> /dev/null || echo 0) + 1 ))
echo "$count" > "$FAKE/$name.count"
body="$FAKE/$name.json"
[[ "$count" -ge 2 && -f "$FAKE/$name.2.json" ]] && body="$FAKE/$name.2.json"
[[ -f "$body" ]] && cp "$body" "$out"
code="$(cat "$FAKE/$name.status" 2> /dev/null || echo 200)"
[[ "$count" -ge 2 && -f "$FAKE/$name.2.status" ]] && code="$(cat "$FAKE/$name.2.status")"
if [[ -n "$headers" ]]; then
    printf 'HTTP/1.1 %s\r\n' "$code" > "$headers"
    [[ -n "${location:-}" && ! -f "$FAKE/no-location" ]] && printf 'Location: %s\r\n' "$location" >> "$headers"
fi
printf '%s' "$code"
EOF
chmod +x "$SCRATCH/bin/curl"
export PATH="$SCRATCH/bin:$PATH"
export EDGE_API_URL="https://edge.test/"
export EDGE_POLL_SECONDS=0 EDGE_POLL_TRIES=3
KEY="edge-api-key-never-printed"
CLIENT="client-id-never-printed"
# shellcheck disable=SC2034  # read inside the eval of expect_true()
PRODUCT="https://edge.test/v1/products/prod-42"

reset() {
    rm -f "$FAKE"/*
    answer upload 202 ''
    answer upload-op 200 '{"id":"op-upload-1","status":"Succeeded","message":"Successfully updated package to rdownloader-chrome.zip","errorCode":"","errors":null}'
    answer publish 202 ''
    answer publish-op 200 '{"id":"op-publish-1","status":"Succeeded","message":"Successfully created submission with ID 7","errorCode":"","errors":null}'
}
answer() { echo "$2" > "$FAKE/$1.status"; echo "$3" > "$FAKE/$1.json"; }
store() {
    run_status env EDGE_CLIENT_ID="$CLIENT" EDGE_API_KEY="$KEY" EDGE_PRODUCT_ID=prod-42 \
        "$ROOT/scripts/edge-addons.sh" "$1" "$ZIP"
}
leaks() {
    local value
    for value in "$KEY" "$CLIENT"; do
        grep -rqF -- "$value" "$FAKE/calls" "$FAKE/urls" "$FAKE/headers" && return 0
        grep -qF -- "$value" <<< "$output" && return 0
    done
    return 1
}

reset
run_status env -u EDGE_CLIENT_ID -u EDGE_API_KEY -u EDGE_PRODUCT_ID \
    "$ROOT/scripts/edge-addons.sh" publish "$ZIP"
expect_status "without credentials" 0
expect_output "warns" "::warning::Edge Add-ons not updated: EDGE_CLIENT_ID is not set"
expect_true "and asks nobody" '[[ ! -e "$FAKE/calls" ]]'

reset
run_status env -u EDGE_PRODUCT_ID EDGE_CLIENT_ID="$CLIENT" EDGE_API_KEY="$KEY" \
    "$ROOT/scripts/edge-addons.sh" publish "$ZIP"
expect_status "without the product id" 0
expect_output "names it" "EDGE_PRODUCT_ID is not set"

reset
store publish
expect_status "a new version" 0
expect_true "uploads the ZIP to the draft" '[[ "$(sed -n 1p "$FAKE/urls")" == "$PRODUCT/submissions/draft/package" ]] && grep -qx -- "$ZIP" "$FAKE/calls"'
expect_true "as application/zip" 'grep -qx "Content-Type: application/zip" "$FAKE/calls"'
expect_true "waits for the upload's operation" '[[ "$(sed -n 2p "$FAKE/urls")" == "$PRODUCT/submissions/draft/package/operations/op-upload-1" ]]'
expect_true "submits the draft" '[[ "$(sed -n 3p "$FAKE/urls")" == "$PRODUCT/submissions" ]]'
expect_true "with certification notes" 'grep -qF "\"notes\":\"rDownloader 1.2.3: https://github.com/degoya/rDownloader/releases/tag/v1.2.3\"" "$FAKE/body"'
expect_true "and waits for the submission's operation" '[[ "$(sed -n 4p "$FAKE/urls")" == "$PRODUCT/submissions/operations/op-publish-1" ]]'
expect_true "with the key and client id from a header file" '[[ "$(sort -u "$FAKE/headers")" == "$(printf "Authorization: ApiKey\nX-ClientID")" ]]'
expect_output "says it is in certification" "submitted: 1.2.3 is in certification"
expect_true "no key or client id in any output or argument" '! leaks'

reset
store upload
expect_status "upload alone" 0
expect_output "uploads" "uploaded 1.2.3"
expect_true "submits nothing" '! grep -qx "$PRODUCT/submissions" "$FAKE/urls"'

reset
answer upload-op 200 '{"id":"op-upload-1","status":"InProgress","message":null,"errorCode":null,"errors":null}'
echo '{"id":"op-upload-1","status":"Succeeded","message":"done","errorCode":"","errors":null}' > "$FAKE/upload-op.2.json"
store publish
expect_status "an upload processed in the background" 0
expect_true "is waited for, then submitted" '[[ "$(grep -c "/draft/package/operations/" "$FAKE/urls")" == 2 ]] && grep -qx "$PRODUCT/submissions" "$FAKE/urls"'

# HTTP 202 without a body: accepted, still processing (v1.11.0, run 37381495818; RD-1120-07).
reset
answer upload-op 202 ''
echo 200 > "$FAKE/upload-op.2.status"
echo '{"id":"op-upload-1","status":"Succeeded","message":"done","errorCode":"","errors":null}' > "$FAKE/upload-op.2.json"
store publish
expect_status "an operation answered with 202, then finished" 0
expect_true "is waited for, not refused" '! grep -qF "::warning::" <<< "$output" && [[ "$(grep -c "/draft/package/operations/" "$FAKE/urls")" == 2 ]]'
expect_output "and submitted" "submitted: 1.2.3 is in certification"

reset
answer publish-op 202 ''
store publish
expect_status "a submission that stays at 202" 0
expect_output "warns after the wait, not at the first answer" "still processing the submission after 0 s"
expect "asked as often as the wait allows" "3" "$(grep -c "/submissions/operations/" "$FAKE/urls")"

reset
answer upload-op 200 '{"id":"op-upload-1","status":"InProgress","message":null,"errorCode":null,"errors":null}'
store publish
expect_status "an upload that never finishes" 0
expect_output "warns after the wait" "still processing the upload"
expect_true "submits nothing" '! grep -qx "$PRODUCT/submissions" "$FAKE/urls"'

reset
answer upload-op 200 '{"id":"op-upload-1","status":"Failed","message":"The manifest version must be higher.","errorCode":"InvalidVersion","errors":["version"]}'
store publish
expect_status "a refused upload" 0
expect_output "warns with the store's reason" "the store did not take the upload (Failed): The manifest version must be higher. (InvalidVersion)"
expect_true "submits nothing" '! grep -qx "$PRODUCT/submissions" "$FAKE/urls"'

reset
answer upload 401 '{"message":"Unauthorized"}'
store publish
expect_status "a refused key" 0
expect_output "warns" "the store refused this key or product (HTTP 401: Unauthorized)"

reset
touch "$FAKE/no-location"
store publish
expect_status "an upload without an operation ID" 0
expect_output "warns" "accepted the upload without an operation ID"

reset
answer publish-op 200 '{"id":"op-publish-1","status":"Failed","message":"Can'"'"'t publish extension since there are no updates, please try again after updating the package.","errorCode":"NoModulesUpdated","errors":null}'
store publish
expect_status "a version the store has already" 0
expect_output "says so" "1.2.3 is in the store already"
expect_true "without a warning" '! grep -qF "::warning::" <<< "$output"'

reset
answer publish-op 200 '{"id":"op-publish-1","status":"Failed","message":"Can'"'"'t publish extension as your extension submission is in progress. Please try again later.","errorCode":"InProgressSubmission","errors":null}'
store publish
expect_status "an earlier submission still in review" 0
expect_output "warns with the store's reason" "the store refused the submission (Failed): Can't publish extension as your extension submission is in progress. Please try again later. (InProgressSubmission)"

reset
answer publish-op 200 '{"id":"op-publish-1","message":"An error occurred while processing the request."}'
store publish
expect_status "an unexpected failure without a status" 0
expect_output "is a refusal, not a wait" "the store refused the submission (Failed): An error occurred while processing the request."

reset
answer publish 500 '{"message":"Internal error"}'
store publish
expect_status "a failed submission call" 0
expect_output "warns" "the submission answered HTTP 500: Internal error"
expect_true "no key in any output or argument" '! leaks'

run_status "$ROOT/scripts/edge-addons.sh" publish
expect_status "a call without the ZIP" 2
echo 'not a zip' > "$SCRATCH/broken.zip"
run_status "$ROOT/scripts/edge-addons.sh" publish "$SCRATCH/broken.zip"
expect_status "a ZIP without a manifest version" 2

finish_tests "edge-addons"
