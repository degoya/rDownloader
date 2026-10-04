# shellcheck shell=bash
# shellcheck disable=SC2154  # `work` is the sourcing script's
#
# The HTTP and JSON helpers of the browser-extension store scripts, scripts/chrome-webstore.sh and
# scripts/edge-addons.sh (RD-191-09: kept twice until then). Sourced; the caller sets `work`, a
# scratch directory holding the `auth` header file curl sends, and receives `curl.err` and
# `headers` there.
#
#   json_field <file> <dotted.path>                 # the field, empty when absent or no JSON
#   store_reason <file> <message path> [<code path>]  # the store's own words for a refusal
#   api <method> <url> <response file> [curl args]  # prints the HTTP status, 000 for no answer

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

# The store's own words for a refused call or a failed operation, for the warning: the message at
# path $2 of response $1, with the code at path $3 when given, else what curl said.
store_reason() {
    local message code=""
    message="$(json_field "$1" "$2")"
    [[ -z "${3:-}" ]] || code="$(json_field "$1" "$3")"
    [[ -n "$message" ]] || message="$(tr '\n' ' ' < "$work/curl.err")"
    echo "${message:-no reason given}${code:+ (${code})}"
}

# $1 method, $2 URL, $3 response file, the rest extra curl arguments; prints the HTTP status, 000
# when there was no answer. The credentials come from the header file; the response headers land
# in $work/headers.
api() {
    local method="$1" url="$2" out="$3" status
    shift 3
    status="$(curl -sS -o "$out" -D "$work/headers" -w '%{http_code}' -X "$method" -H @"$work/auth" \
        "$@" "$url" 2> "$work/curl.err")" || status="000"
    echo "$status"
}
