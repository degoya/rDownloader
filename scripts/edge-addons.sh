#!/usr/bin/env bash
#
# The Chrome build into Microsoft Edge Add-ons (RD-190-11), through the Edge Add-ons API v1.1
# (https://learn.microsoft.com/microsoft-edge/extensions/update/api/using-addons-api): `upload`
# replaces the product's draft with the ZIP, `publish` uploads it and submits the draft for
# certification; Microsoft publishes it once the review is through. Edge takes the Chrome build
# (Manifest V3) as it is, so there is no Edge build of its own.
#
# Usage:
#   scripts/edge-addons.sh upload <rdownloader-chrome.zip>
#   scripts/edge-addons.sh publish <rdownloader-chrome.zip>
#
# Credentials come from the environment, never from arguments: EDGE_CLIENT_ID and EDGE_API_KEY
# (Partner Center → Microsoft Edge → Publish API, the v1.1 API key), and EDGE_PRODUCT_ID (the
# listing's product ID, Partner Center → the extension → Overview). Both secrets reach curl in a
# header file, so neither is a command-line argument, and nothing printed contains them.
#
# A release must not fail over the store — the ZIP is on the GitHub release either way — so the
# script ends with exit 0 and a `::warning::` whenever the store does not take the version: no
# credentials (a fork, a local run), a refused key, a rejected upload, a timeout, an outage. It
# exits non-zero only for a wrong call or a ZIP without a manifest version.
#
# Unlike the Chrome Web Store, the API cannot say which version the store has. A version already
# submitted or published there — the owner's upload by hand, a re-run of the release — ends in the
# store's NoModulesUpdated, which is reported as nothing to do.
#
# EDGE_API_URL exists for the test, scripts/tests/edge-addons.sh, which stands in a stub `curl`;
# EDGE_POLL_SECONDS and EDGE_POLL_TRIES bound the wait for an operation the store processes in
# the background.
set -euo pipefail

EDGE_API_URL="${EDGE_API_URL:-https://api.addons.microsoftedge.microsoft.com/}"
EDGE_POLL_SECONDS="${EDGE_POLL_SECONDS:-10}"
EDGE_POLL_TRIES="${EDGE_POLL_TRIES:-30}"

usage() {
    echo "usage: $0 upload <chrome.zip> | publish <chrome.zip>" >&2
    exit 2
}

skip() {
    echo "::warning::Edge Add-ons not updated: $1 — submit rdownloader-chrome.zip by hand"
    exit 0
}

# Prints field $2 (a dotted path, array indices as numbers) of the JSON file $1, empty when absent
# or when the file is no JSON.
json_field() {
    node -e '
let value
try { value = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8")) } catch { value = undefined }
value = process.argv[2].split(".").reduce((node, key) => node?.[key], value)
process.stdout.write(value === undefined || value === null ? "" : String(value))
' "$1" "$2"
}

# The store's own words for a refused call or a failed operation, for the warning.
store_error() {
    local message code
    message="$(json_field "$1" message)"
    code="$(json_field "$1" errorCode)"
    [[ -n "$message" ]] || message="$(tr '\n' ' ' < "$work/curl.err")"
    echo "${message:-no reason given}${code:+ (${code})}"
}

# $1 method, $2 URL, $3 response file, the rest extra curl arguments; prints the HTTP status, 000
# when there was no answer. The key and the client ID come from the header file; the response
# headers land in $work/headers.
api() {
    local method="$1" url="$2" out="$3" status
    shift 3
    status="$(curl -sS -o "$out" -D "$work/headers" -w '%{http_code}' -X "$method" -H @"$work/auth" \
        "$@" "$url" 2> "$work/curl.err")" || status="000"
    echo "$status"
}

# The operation ID of an accepted call: the last segment of its Location header.
operation_id() {
    sed -n 's/^[Ll]ocation:[[:space:]]*//p' "$work/headers" | tr -d '\r' | tail -n 1 | sed 's#.*/##'
}

# Waits for operation $2 under URL $1, in this shell so that a refusal can end the script; leaves
# its last answer in $work/operation.json and its final status (Succeeded, Failed) in $state,
# empty when it was still going after the wait.
wait_for() {
    local url="$1/$2" tries=0 status
    while :; do
        status="$(api GET "$url" "$work/operation.json")"
        [[ "$status" == "200" ]] \
            || skip "the operation query answered HTTP ${status}: $(store_error "$work/operation.json")"
        state="$(json_field "$work/operation.json" status)"
        # The store's "unexpected failure" answer carries a message and no status.
        [[ "$state" == "InProgress" ]] || { state="${state:-Failed}"; return 0; }
        tries=$((tries + 1))
        if [[ "$tries" -ge "$EDGE_POLL_TRIES" ]]; then
            state=""
            return 0
        fi
        sleep "$EDGE_POLL_SECONDS"
    done
}

cmd="${1:-}"
case "$cmd" in
    upload | publish) [[ $# -eq 2 ]] || usage ;;
    *) usage ;;
esac
zip="$2"
[[ -f "$zip" ]] || { echo "no such file: $zip" >&2; exit 2; }
version="$(unzip -p "$zip" manifest.json 2> /dev/null \
    | node -e 'try { process.stdout.write(String(JSON.parse(require("node:fs").readFileSync(0, "utf8")).version ?? "")) } catch {}' \
    || true)"
[[ -n "$version" ]] || { echo "$zip holds no manifest.json with a version" >&2; exit 2; }

for name in EDGE_CLIENT_ID EDGE_API_KEY EDGE_PRODUCT_ID; do
    [[ -n "${!name:-}" ]] || skip "${name} is not set"
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 700 "$work"
product="${EDGE_API_URL}v1/products/${EDGE_PRODUCT_ID}"
# printf is a builtin: the key passes through no other process's arguments.
(umask 077 && printf 'Authorization: ApiKey %s\nX-ClientID: %s\n' "$EDGE_API_KEY" "$EDGE_CLIENT_ID" > "$work/auth")

echo "==> uploading ${zip} (${version}) to Edge Add-ons product ${EDGE_PRODUCT_ID}"
status="$(api POST "${product}/submissions/draft/package" "$work/upload.json" \
    -H 'Content-Type: application/zip' -T "$zip")"
case "$status" in
    202) ;;
    401 | 403) skip "the store refused this key or product (HTTP ${status}: $(store_error "$work/upload.json"))" ;;
    000) skip "the store did not answer ($(tr '\n' ' ' < "$work/curl.err"))" ;;
    *) skip "the upload answered HTTP ${status}: $(store_error "$work/upload.json")" ;;
esac
operation="$(operation_id)"
[[ -n "$operation" ]] || skip "the store accepted the upload without an operation ID"
wait_for "${product}/submissions/draft/package/operations" "$operation"
case "$state" in
    Succeeded) echo "==> uploaded ${version}" ;;
    "") skip "the store was still processing the upload after $((EDGE_POLL_TRIES * EDGE_POLL_SECONDS)) s" ;;
    *) skip "the store did not take the upload (${state}): $(store_error "$work/operation.json")" ;;
esac
[[ "$cmd" == "publish" ]] || exit 0

echo "==> submitting ${version} for certification"
node -e 'process.stdout.write(JSON.stringify({ notes: process.argv[1] }))' \
    "rDownloader ${version}: https://github.com/degoya/rDownloader/releases/tag/v${version}" \
    > "$work/notes.json"
status="$(api POST "${product}/submissions" "$work/publish.json" \
    -H 'Content-Type: application/json' --data-binary @"$work/notes.json")"
[[ "$status" == "202" ]] || skip "the submission answered HTTP ${status}: $(store_error "$work/publish.json")"
operation="$(operation_id)"
[[ -n "$operation" ]] || skip "the store accepted the submission without an operation ID"
wait_for "${product}/submissions/operations" "$operation"
case "$state" in
    Succeeded) echo "==> submitted: ${version} is in certification" ;;
    "") skip "the store was still processing the submission after $((EDGE_POLL_TRIES * EDGE_POLL_SECONDS)) s" ;;
    *)
        if [[ "$(json_field "$work/operation.json" errorCode)" == "NoModulesUpdated" ]]; then
            echo "==> ${version} is in the store already (nothing new to submit); nothing to do"
            exit 0
        fi
        skip "the store refused the submission (${state}): $(store_error "$work/operation.json")"
        ;;
esac
