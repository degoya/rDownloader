#!/usr/bin/env bash
#
# Brings the marketing site (rdownloader.net) to a release (RD-130-23).
#
# The site is its own repository (~/projects/rdownloader-website, Nuxt, statically generated).
# Every release fact it shows — version, release date, the tag, the download assets, the
# container image, the handbook links — derives from one file, app/data/release.json, and this
# script sets it: the version, today's date (kept as it is when the file already names this
# version with a date, so a re-run does not move the release date; an empty date is the
# placeholder of a version prepared before its tag and is filled) and the repository, which stays. Then it
# checks that every public wiki page the site links to exists in the exported wiki, runs the
# site's tests, generates it, checks the generated download page links this version's assets,
# and commits "Release <version>" in the site repository.
#
# The site's feature text is not touched: where the release's CHANGELOG section changes what the
# site describes, that is written by hand, like the wiki.
#
# Nothing leaves this machine without --push, and even then only the commit. The script never
# deploys: the owner uploads .output/public/ by hand (the site's docs/deployment-ispconfig-nginx.md),
# and the last line names that directory.
#
# Usage:
#   scripts/update-website.sh 1.3.0           # set, test, generate, commit
#   scripts/update-website.sh 1.3.0 --push    # ... and push the site's branch
#
# Environment:
#   RD_WEBSITE_DIR          the site's checkout (default: ~/projects/rdownloader-website)
#   RD_WEBSITE_BRANCH       the branch a release is committed on (default: main)
#   RD_PUBLIC_WIKI_DIR      the public wiki's clone, as for export-wiki.sh
#                           (default: ~/projects/rDownloader-public.wiki)
#
set -euo pipefail

SITE="${RD_WEBSITE_DIR:-$HOME/projects/rdownloader-website}"
SITE_BRANCH="${RD_WEBSITE_BRANCH:-main}"
WIKI_DIR="${RD_PUBLIC_WIKI_DIR:-$HOME/projects/rDownloader-public.wiki}"
DATA="app/data/release.json"

VERSION=""
DO_PUSH=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --push) DO_PUSH=1; shift ;;
        -h|--help) sed -n '2,31p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *)
            [[ -z "$VERSION" ]] || { echo "unexpected argument: $1" >&2; exit 2; }
            VERSION="$1"; shift ;;
    esac
done
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: scripts/update-website.sh <version> [--push]" >&2
    exit 2
fi

[[ -f "$SITE/$DATA" ]] || { echo "$SITE/$DATA not found: is RD_WEBSITE_DIR the site's checkout?" >&2; exit 1; }
branch="$(git -C "$SITE" symbolic-ref --quiet --short HEAD || true)"
[[ "$branch" == "$SITE_BRANCH" ]] \
    || { echo "the site is on '${branch:-a detached HEAD}', a release goes on '$SITE_BRANCH' (RD_WEBSITE_BRANCH)" >&2; exit 1; }
[[ -z "$(git -C "$SITE" status --porcelain)" ]] \
    || { echo "the site's checkout has uncommitted changes; commit or remove them first" >&2; exit 1; }
[[ -d "$WIKI_DIR" ]] || { echo "the public wiki clone $WIKI_DIR is missing; run export-wiki.sh first" >&2; exit 1; }

echo "==> checking the site's handbook links against $WIKI_DIR"
missing=0
while read -r page; do
    if [[ -z "$(find "$WIKI_DIR" -path "$WIKI_DIR/.git" -prune -o -name "$page.md" -print -quit)" ]]; then
        echo "!! the site links the wiki page '$page', which the public wiki does not have" >&2
        missing=1
    fi
done < <(grep -o "wiki('[^']*')" "$SITE/app/utils/release.ts" | sed "s/^wiki('//; s/')$//")
[[ "$missing" -eq 0 ]] || exit 1

# From here on a failure puts the data file back, so that the next run finds a clean checkout.
restore() { git -C "$SITE" checkout --quiet -- "$DATA"; }
trap 'status=$?; [[ $status -eq 0 ]] || restore; exit $status' EXIT

echo "==> setting $DATA to $VERSION"
python3 - "$SITE/$DATA" "$VERSION" "$(date +%F)" <<'PY'
import json
import sys

path, version, today = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    data = json.load(handle)
if data.get("version") != version or not data.get("date"):
    data["date"] = today
data["version"] = version
with open(path, "w", encoding="utf-8") as handle:
    json.dump(data, handle, indent=2)
    handle.write("\n")
print(f"   version {data['version']}, date {data['date']}, repository {data['repo']}")
PY

cd "$SITE"
[[ -d node_modules ]] || { echo "==> pnpm install"; pnpm install --frozen-lockfile; }
echo "==> the site's tests"
pnpm test
echo "==> generating the site"
pnpm run generate
grep -q "releases/download/v$VERSION/" .output/public/download/index.html \
    || { echo "!! the generated download page does not link the v$VERSION assets" >&2; exit 1; }

if git diff --quiet -- "$DATA"; then
    echo "==> the site already names $VERSION; nothing to commit"
else
    git add -- "$DATA"
    git commit --quiet -m "Release $VERSION"
    echo "==> committed 'Release $VERSION' on $SITE_BRANCH ($(git rev-parse --short HEAD))"
fi
trap - EXIT

if [[ "$DO_PUSH" -eq 1 ]]; then
    echo "==> pushing $SITE_BRANCH"
    git push origin "$SITE_BRANCH"
else
    echo "==> not pushed; publish with: git -C $SITE push origin $SITE_BRANCH"
fi

echo "==> website built in $SITE/.output/public — deploy it by hand (the site's docs/deployment-ispconfig-nginx.md)"
