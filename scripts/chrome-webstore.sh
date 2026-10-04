#!/usr/bin/env bash
#
# The Chrome build into the Chrome Web Store (RD-170-10), through the Web Store API v2
# (https://developer.chrome.com/docs/webstore/api — v1.1 is served only until 2026-10-15):
# `upload` replaces the item's draft with the ZIP, `publish` uploads it and submits it for review;
# Google publishes it once the review is through.
#
# Usage:
#   scripts/chrome-webstore.sh upload <rdownloader-chrome.zip>
#   scripts/chrome-webstore.sh publish <rdownloader-chrome.zip>
#
# Credentials come from the environment, never from arguments: CWS_CLIENT_ID, CWS_CLIENT_SECRET
# and CWS_REFRESH_TOKEN (the owner's OAuth client, scope chromewebstore), and CWS_PUBLISHER_ID
# (Developer Dashboard → Account; v2 names it in every path). CWS_ITEM_ID defaults to the
# listing, nfdbhbkjnbdnaaekabaochlhgkaafnda. The secrets reach curl as files and the access token
# as a header file, so none is a command-line argument, and nothing printed contains them.
#
# A release must not fail over the store — the ZIP is on the GitHub release either way — so the
# script ends with exit 0 and a `::warning::` whenever the store does not take the version: no
# credentials (a fork, a local run), a refused token, a rejected upload, a timeout, an outage. It
# exits non-zero only for a wrong call or a ZIP without a manifest version.
#
# Before uploading it asks the store for the item's state: a version already submitted or
# published there — the owner's upload by hand, a re-run of the release — is left alone.
#
# CWS_TOKEN_URL and CWS_API_URL exist for the test, scripts/tests/chrome-webstore.sh, which
# stands in a stub `curl`; CWS_POLL_SECONDS and CWS_POLL_TRIES bound the wait for an upload the
# store processes in the background.
set -euo pipefail

CWS_TOKEN_URL="${CWS_TOKEN_URL:-https://oauth2.googleapis.com/token}"
CWS_API_URL="${CWS_API_URL:-https://chromewebstore.googleapis.com/}"
CWS_ITEM_ID="${CWS_ITEM_ID:-nfdbhbkjnbdnaaekabaochlhgkaafnda}"
CWS_POLL_SECONDS="${CWS_POLL_SECONDS:-10}"
CWS_POLL_TRIES="${CWS_POLL_TRIES:-30}"

usage() {
    echo "usage: $0 upload <chrome.zip> | publish <chrome.zip>" >&2
    exit 2
}

skip() {
    echo "::warning::Chrome Web Store not updated: $1 — submit rdownloader-chrome.zip by hand"
    exit 0
}

# json_field, api and store_reason (RD-191-09).
# shellcheck source=lib/store-api.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/store-api.sh"

# The store's own words for a refused call, for the warning.
store_error() { store_reason "$1" error.message; }

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

for name in CWS_CLIENT_ID CWS_CLIENT_SECRET CWS_REFRESH_TOKEN CWS_PUBLISHER_ID; do
    [[ -n "${!name:-}" ]] || skip "${name} is not set"
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 700 "$work"
item="${CWS_API_URL}v2/publishers/${CWS_PUBLISHER_ID}/items/${CWS_ITEM_ID}"
upload_url="${CWS_API_URL}upload/v2/publishers/${CWS_PUBLISHER_ID}/items/${CWS_ITEM_ID}:upload"

# An access token for this run. The three values go to curl as files it URL-encodes itself;
# node turns the answer into the header file, so the token never passes through the shell either.
(
    umask 077
    printf '%s' "$CWS_CLIENT_ID" > "$work/client_id"
    printf '%s' "$CWS_CLIENT_SECRET" > "$work/client_secret"
    printf '%s' "$CWS_REFRESH_TOKEN" > "$work/refresh_token"
)
echo "==> asking Google for an access token"
status="$(curl -sS -o "$work/token.json" -w '%{http_code}' -X POST \
    --data-urlencode "client_id@$work/client_id" \
    --data-urlencode "client_secret@$work/client_secret" \
    --data-urlencode "refresh_token@$work/refresh_token" \
    --data-urlencode "grant_type=refresh_token" \
    "$CWS_TOKEN_URL" 2> "$work/curl.err")" || status="000"
rm -f "$work/client_id" "$work/client_secret" "$work/refresh_token"
case "$status" in
    200) ;;
    000) skip "Google did not answer ($(tr '\n' ' ' < "$work/curl.err"))" ;;
    # Only the error code, never the body: `invalid_grant` is an expired or revoked refresh token.
    *) skip "Google refused the refresh token (HTTP ${status}, $(json_field "$work/token.json" error))" ;;
esac
(umask 077 && node -e '
const fs = require("node:fs")
const token = JSON.parse(fs.readFileSync(process.argv[1], "utf8")).access_token ?? ""
if (token) fs.writeFileSync(process.argv[2], `Authorization: Bearer ${token}\n`)
' "$work/token.json" "$work/auth")
rm -f "$work/token.json"
[[ -s "$work/auth" ]] || skip "Google's answer carried no access token"

echo "==> asking the Chrome Web Store for ${CWS_ITEM_ID}"
status="$(api GET "${item}:fetchStatus" "$work/status.json")"
case "$status" in
    200) ;;
    401 | 403) skip "the store refused this account or item (HTTP ${status}: $(store_error "$work/status.json"))" ;;
    000) skip "the store did not answer ($(tr '\n' ' ' < "$work/curl.err"))" ;;
    *) skip "the store answered HTTP ${status} to the status query: $(store_error "$work/status.json")" ;;
esac
for revision in published submitted; do
    store_version="$(json_field "$work/status.json" "${revision}ItemRevisionStatus.distributionChannels.0.crxVersion")"
    state="$(json_field "$work/status.json" "${revision}ItemRevisionStatus.state")"
    if [[ "$store_version" == "$version" && "$state" != "REJECTED" && "$state" != "CANCELLED" ]]; then
        echo "==> ${version} is in the store already (${revision}, ${state:-no state}); nothing to do"
        exit 0
    fi
done

echo "==> uploading ${zip} (${version})"
status="$(api POST "$upload_url" "$work/upload.json" -T "$zip")"
[[ "$status" == "200" ]] || skip "the upload answered HTTP ${status}: $(store_error "$work/upload.json")"
upload_state="$(json_field "$work/upload.json" uploadState)"
tries=0
while [[ "$upload_state" == "IN_PROGRESS" || "$upload_state" == "UPLOAD_IN_PROGRESS" ]]; do
    tries=$((tries + 1))
    [[ "$tries" -le "$CWS_POLL_TRIES" ]] \
        || skip "the store was still processing the upload after $((CWS_POLL_TRIES * CWS_POLL_SECONDS)) s"
    sleep "$CWS_POLL_SECONDS"
    status="$(api GET "${item}:fetchStatus" "$work/status.json")"
    [[ "$status" == "200" ]] || skip "the status query during the upload answered HTTP ${status}"
    upload_state="$(json_field "$work/status.json" lastAsyncUploadState)"
done
[[ "$upload_state" == "SUCCEEDED" ]] \
    || skip "the store did not take the upload (${upload_state:-no state}): $(store_error "$work/upload.json")"
echo "==> uploaded ${version}"
[[ "$cmd" == "publish" ]] || exit 0

echo "==> submitting ${version} for review"
status="$(api POST "${item}:publish" "$work/publish.json" \
    -H 'Content-Type: application/json' --data '{"publishType":"DEFAULT_PUBLISH"}')"
[[ "$status" == "200" ]] || skip "the submission answered HTTP ${status}: $(store_error "$work/publish.json")"
state="$(json_field "$work/publish.json" state)"
case "$state" in
    PENDING_REVIEW | STAGED | PUBLISHED) echo "==> submitted: ${version} is ${state}" ;;
    *) skip "the store answered the submission with state ${state:-none}" ;;
esac
