#!/usr/bin/env bash
#
# Closes the token account's own open winget-pkgs pull requests for an older rDownloader version,
# after the release workflow's `package-managers` job submitted a new one (RD-1280-03). komac opens
# one pull request per version and closes none; up to 1.27 eleven stood open (1.16.1 … 1.26.0),
# and winget-pkgs' moderators only ever work on the newest.
#
# Closed is a pull request that is open, authored by the account `gh` is signed in as (the
# WINGET_TOKEN's), titled exactly `New version: degoya.rDownloader version X`, with X older than
# the version just submitted — never one of another author, never a merged one, never one for the
# same or a newer version (a re-run's duplicate stays for a human). Each gets the comment
# "Superseded by #<new>". The new pull request's number comes from komac's log, else from the
# open pull request titled with the submitted version; without one nothing is closed.
#
# The release is out when this runs: every failure is a `::warning::` and exit 0, never a red job.
# `gh` reads GH_TOKEN; nothing here prints it.
#
# Usage:
#   scripts/winget-supersede.sh <version> [<komac log>]
#
set -euo pipefail

REPO="${RD_WINGET_REPO:-microsoft/winget-pkgs}"
IDENTIFIER="degoya.rDownloader"
TITLE_PREFIX="New version: ${IDENTIFIER} version "

[[ $# -ge 1 && $# -le 2 && "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || {
    echo "usage: scripts/winget-supersede.sh <version> [<komac log>]" >&2
    exit 2
}
version="$1"
log="${2:-}"

warn() { echo "::warning::$*"; }

# Whether version $1 is older than $2: by their X.Y.Z, and on equal numbers a pre-release is
# older than the release (sort -V alone puts 1.8.0 before 1.8.0-beta.2).
older() {
    local a="${1%%-*}" b="${2%%-*}"
    if [[ "$a" == "$b" ]]; then
        [[ "$1" == *-* && "$2" != *-* ]]
        return
    fi
    [[ "$(printf '%s\n%s\n' "$a" "$b" | sort -V | head -n 1)" == "$a" ]]
}

if ! me="$(gh api user --jq .login)" || [[ -z "$me" ]]; then
    warn "the token's account could not be read; no superseded winget pull request is closed"
    exit 0
fi

# `<number>\t<author>\t<title>` per open pull request of the account.
if ! open="$(gh pr list --repo "$REPO" --author "$me" --state open --limit 200 \
    --json number,author,title --jq '.[] | "\(.number)\t\(.author.login)\t\(.title)"')"; then
    warn "the open pull requests of ${me} in ${REPO} could not be listed; none is closed"
    exit 0
fi

new=""
if [[ -n "$log" && -f "$log" ]]; then
    new="$(grep -oE "${REPO}/pull/[0-9]+" "$log" | tail -n 1 | sed 's#.*/##' || true)"
fi
if [[ -z "$new" ]]; then
    new="$(awk -F '\t' -v me="$me" -v title="${TITLE_PREFIX}${version}" \
        '$2 == me && $3 == title { print $1 }' <<< "$open" | sort -n | tail -n 1)"
fi
if [[ -z "$new" ]]; then
    warn "no open pull request for ${IDENTIFIER} ${version} in ${REPO}; no older one is closed"
    exit 0
fi

closed=0
while IFS=$'\t' read -r number author title; do
    [[ -n "$number" && "$number" != "$new" && "$author" == "$me" ]] || continue
    [[ "$title" == "${TITLE_PREFIX}"* ]] || continue
    old="${title#"${TITLE_PREFIX}"}"
    [[ "$old" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || continue
    older "$old" "$version" || continue
    if gh pr close "$number" --repo "$REPO" --comment "Superseded by #${new}" > /dev/null; then
        echo "closed #${number} (${IDENTIFIER} ${old}), superseded by #${new}"
        closed=$((closed + 1))
    else
        warn "#${number} (${IDENTIFIER} ${old}) could not be closed; close it by hand: Superseded by #${new}"
    fi
done <<< "$open"
echo "${closed} superseded winget pull request(s) closed; #${new} carries ${version}"
