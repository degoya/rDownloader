#!/usr/bin/env bash
#
# The Firefox build against Mozilla's add-on service: `lint` runs AMO's validator locally, so a
# manifest AMO would reject fails CI before a tag (RD-160-07); `submit` uploads the build to the
# listing https://addons.mozilla.org/addon/rdownloader/ (the "listed" channel, RD-170-10), where
# it goes through Mozilla's review and reaches Firefox users as an update once approved. A listed
# version is signed only after that review, so nothing signed comes back to the release.
#
# Usage:
#   scripts/firefox-amo.sh lint <unpacked firefox build>
#   scripts/firefox-amo.sh submit <unpacked firefox build>
#
# `submit` reads the credentials from AMO_JWT_ISSUER and AMO_JWT_SECRET, never from arguments,
# and hands them to web-ext as its WEB_EXT_API_* variables, so neither appears in a command line.
# Nothing it prints contains them.
#
# A release must not fail over the store — the ZIPs are the release — so `submit` ends with exit 0
# and a `::warning::` whenever AMO does not take the version: no credentials (a fork, a local
# run), a rejection, a timeout, an outage. It exits non-zero only for a wrong call or a build
# without an add-on ID or version.
#
# AMO accepts every version number once across both channels. Before uploading, `submit` asks
# for this version; if AMO has it already — the owner's submission by hand, a re-run of the
# release — nothing is uploaded: listed, it is only reported; unlisted (a 1.6.0 self-distributed
# signature), it warns, because that number can never be listed and the listing gets the next one.
#
# AMO_BASE_URL (default https://addons.mozilla.org/api/v5/) exists for the test,
# scripts/tests/firefox-amo.sh, which stands in stub `curl` and `npx`.
set -euo pipefail

# Pinned: a new web-ext can change the validator's verdict or the upload. Raise it deliberately.
WEB_EXT_VERSION="10.7.0"
AMO_BASE_URL="${AMO_BASE_URL:-https://addons.mozilla.org/api/v5/}"
# How long web-ext waits for an answer from AMO's service; the review itself is not waited for
# (`--approval-timeout 0`), it takes days.
RESPONSE_TIMEOUT_MS=900000

usage() {
    echo "usage: $0 lint <source-dir> | submit <source-dir>" >&2
    exit 2
}

web_ext() {
    npx --yes "web-ext@${WEB_EXT_VERSION}" "$@" --no-config-discovery
}

skip() {
    echo "::warning::Firefox extension not submitted to AMO: $1 — submit rdownloader-firefox.zip by hand"
    exit 0
}

# A JWT for AMO's API, valid for one minute: HS256 over {iss, jti, iat, exp}, read from the
# environment inside node so the secret is never an argument.
jwt() {
    node -e '
const { createHmac, randomUUID } = require("node:crypto")
const part = (value) => Buffer.from(JSON.stringify(value)).toString("base64url")
const now = Math.floor(Date.now() / 1000)
const body = part({ alg: "HS256", typ: "JWT" }) + "." +
  part({ iss: process.env.AMO_JWT_ISSUER, jti: randomUUID(), iat: now, exp: now + 60 })
process.stdout.write(body + "." + createHmac("sha256", process.env.AMO_JWT_SECRET).update(body).digest("base64url"))
'
}

# GET $1 into $2 with a fresh token; prints the HTTP status, 000 when there was no answer. The
# header comes from a file, so the token is not a curl argument either.
amo_get() {
    local url="$1" out="$2" status
    (umask 077 && printf 'Authorization: JWT %s\n' "$(jwt)" > "$work/auth")
    status="$(curl -sS -L -o "$out" -w '%{http_code}' -H @"$work/auth" \
        -H 'Accept: application/json' "$url" 2> "$work/curl.err")" || status="000"
    rm -f "$work/auth"
    echo "$status"
}

# Prints manifest field $2 of $1 (a dotted path), empty when absent.
manifest_field() {
    node -e '
const manifest = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"))
const value = process.argv[2].split(".").reduce((node, key) => node?.[key], manifest)
process.stdout.write(value === undefined ? "" : String(value))
' "$1/manifest.json" "$2"
}

cmd="${1:-}"
case "$cmd" in
    lint)
        [[ $# -eq 2 ]] || usage
        [[ -f "$2/manifest.json" ]] || { echo "no manifest.json in $2" >&2; exit 2; }
        echo "==> web-ext ${WEB_EXT_VERSION} lint $2"
        web_ext lint --source-dir "$2"
        exit 0
        ;;
    submit)
        [[ $# -eq 2 ]] || usage
        ;;
    *) usage ;;
esac

source_dir="$2"
[[ -f "$source_dir/manifest.json" ]] || { echo "no manifest.json in $source_dir" >&2; exit 2; }
id="$(manifest_field "$source_dir" browser_specific_settings.gecko.id)"
version="$(manifest_field "$source_dir" version)"
[[ -n "$id" && -n "$version" ]] || { echo "$source_dir/manifest.json names no gecko id or version" >&2; exit 2; }

if [[ -z "${AMO_JWT_ISSUER:-}" || -z "${AMO_JWT_SECRET:-}" ]]; then
    skip "AMO_JWT_ISSUER or AMO_JWT_SECRET is not set"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "==> asking AMO for ${id} ${version}"
encoded_id="$(node -p 'encodeURIComponent(process.argv[1])' "$id")"
status="$(amo_get "${AMO_BASE_URL}addons/addon/${encoded_id}/versions/v${version}/" "$work/version.json")"
case "$status" in
    200)
        # channel and file status, separated by US (0x1f): unlike a tab it is not whitespace to
        # `read`, so an empty field stays a field.
        IFS=$'\x1f' read -r channel file_status < <(node -e '
const version = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"))
console.log([version.channel, version.file?.status].map((v) => v ?? "").join("\x1f"))
' "$work/version.json")
        [[ "$channel" == "listed" ]] \
            || skip "AMO has ${version} in the ${channel:-unknown} channel; a version number exists once, so the listing gets the next version"
        echo "==> ${version} is on AMO already (listed, file '${file_status:-none}'); nothing to upload"
        exit 0
        ;;
    404)
        echo "==> AMO does not know ${version}; web-ext ${WEB_EXT_VERSION} sign --channel listed"
        mkdir -p "$work/artifacts"
        WEB_EXT_API_KEY="$AMO_JWT_ISSUER" WEB_EXT_API_SECRET="$AMO_JWT_SECRET" \
            web_ext sign --channel listed --source-dir "$source_dir" \
            --artifacts-dir "$work/artifacts" --timeout "$RESPONSE_TIMEOUT_MS" --approval-timeout 0 \
            || skip "web-ext sign failed (above)"
        ;;
    401 | 403) skip "AMO refused the credentials or this add-on (HTTP ${status})" ;;
    000) skip "AMO did not answer ($(tr '\n' ' ' < "$work/curl.err"))" ;;
    *) skip "AMO answered HTTP ${status} to the version query" ;;
esac

echo "==> submitted ${version} to the listing; it reaches Firefox users once Mozilla's review approves it"
