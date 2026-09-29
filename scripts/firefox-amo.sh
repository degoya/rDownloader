#!/usr/bin/env bash
#
# The Firefox build against Mozilla's add-on service (RD-160-07): `lint` runs AMO's validator
# locally, so a manifest AMO would reject fails CI before a tag; `sign` has Mozilla sign the build
# as a self-distributed ("unlisted") version, so release Firefox installs it permanently.
#
# Usage:
#   scripts/firefox-amo.sh lint <unpacked firefox build>
#   scripts/firefox-amo.sh sign <unpacked firefox build> <out.xpi>
#
# `sign` reads the credentials from AMO_JWT_ISSUER and AMO_JWT_SECRET, never from arguments, and
# hands them to web-ext as its WEB_EXT_API_* variables, so neither appears in a command line.
# Nothing it prints contains them.
#
# A release must not fail over the signed copy — the ZIPs are still the release — so `sign` ends
# with exit 0 and a `::warning::` whenever no .xpi comes out: no credentials (a fork, a local
# run), a version AMO already has but has not signed, a rejection, a timeout, an outage. It exits
# non-zero only for a wrong call or a build without an add-on ID or version.
#
# AMO accepts every version number once across both channels. Before uploading, `sign` asks
# for this version; if it is there already — the owner's store submission (listed), or a re-run
# of the release — and signed, the signed file is downloaded, checked against AMO's hash and used
# instead. Only when AMO does not know the version is it uploaded.
#
# AMO_BASE_URL (default https://addons.mozilla.org/api/v5/) exists for the test,
# scripts/tests/firefox-amo.sh, which stands in stub `curl` and `npx`.
set -euo pipefail

# Pinned: a new web-ext can change the validator's verdict or the upload. Raise it deliberately.
WEB_EXT_VERSION="10.7.0"
AMO_BASE_URL="${AMO_BASE_URL:-https://addons.mozilla.org/api/v5/}"
# Unlisted versions are signed automatically, usually within minutes.
SIGN_TIMEOUT_MS=900000

usage() {
    echo "usage: $0 lint <source-dir> | sign <source-dir> <out.xpi>" >&2
    exit 2
}

web_ext() {
    npx --yes "web-ext@${WEB_EXT_VERSION}" "$@" --no-config-discovery
}

skip() {
    echo "::warning::Firefox extension not signed: $1 — the release carries the unsigned ZIP only"
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
    sign)
        [[ $# -eq 3 ]] || usage
        ;;
    *) usage ;;
esac

source_dir="$2"
out="$3"
[[ -f "$source_dir/manifest.json" ]] || { echo "no manifest.json in $source_dir" >&2; exit 2; }
id="$(manifest_field "$source_dir" browser_specific_settings.gecko.id)"
version="$(manifest_field "$source_dir" version)"
[[ -n "$id" && -n "$version" ]] || { echo "$source_dir/manifest.json names no gecko id or version" >&2; exit 2; }
rm -f "$out"

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
        # channel, file status, file URL and file hash, separated by US (0x1f): unlike a tab it is
        # not whitespace to `read`, so an empty field stays a field.
        IFS=$'\x1f' read -r channel file_status file_url file_hash < <(node -e '
const version = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"))
const file = version.file ?? {}
console.log([version.channel, file.status, file.url, file.hash].map((v) => v ?? "").join("\x1f"))
' "$work/version.json")
        [[ "$file_status" == "public" && -n "$file_url" ]] \
            || skip "AMO has ${version} (${channel:-unknown} channel) but its file is '${file_status:-none}', not signed"
        # The token goes only to AMO's own host, not to wherever a URL in a response points.
        [[ "$file_url" == "${AMO_BASE_URL%%/api/*}/"* ]] || skip "AMO names a download outside ${AMO_BASE_URL%%/api/*}"
        echo "==> ${version} is on AMO already (${channel} channel, signed); downloading it"
        status="$(amo_get "$file_url" "$work/signed.xpi")"
        [[ "$status" == "200" ]] || skip "the download of the signed ${version} answered ${status}"
        expected="${file_hash#sha256:}"
        actual="$(sha256sum "$work/signed.xpi" | cut -d' ' -f1)"
        [[ "$file_hash" == sha256:* && "$actual" == "$expected" ]] \
            || skip "the downloaded ${version} does not match AMO's hash ${file_hash:-none}"
        mv "$work/signed.xpi" "$out"
        ;;
    404)
        echo "==> AMO does not know ${version}; web-ext ${WEB_EXT_VERSION} sign --channel unlisted"
        mkdir -p "$work/signed"
        WEB_EXT_API_KEY="$AMO_JWT_ISSUER" WEB_EXT_API_SECRET="$AMO_JWT_SECRET" \
            web_ext sign --channel unlisted --source-dir "$source_dir" \
            --artifacts-dir "$work/signed" --timeout "$SIGN_TIMEOUT_MS" \
            || skip "web-ext sign failed (above)"
        shopt -s nullglob
        signed=("$work"/signed/*.xpi)
        [[ ${#signed[@]} -eq 1 ]] || skip "web-ext sign left ${#signed[@]} .xpi files, expected one"
        mv "${signed[0]}" "$out"
        ;;
    401 | 403) skip "AMO refused the credentials or this add-on (HTTP ${status})" ;;
    000) skip "AMO did not answer ($(tr '\n' ' ' < "$work/curl.err"))" ;;
    *) skip "AMO answered HTTP ${status} to the version query" ;;
esac

echo "==> signed: $out ($(sha256sum "$out" | cut -d' ' -f1))"
