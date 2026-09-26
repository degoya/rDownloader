#!/usr/bin/env bash
#
# The private markers of scripts/export-wiki.sh against a scratch wiki (RD-140-22): a private
# page and its sidebar line stay behind, a private section is cut out, and every way of leaking
# one — a link to the page, a link to a heading inside the section, a marker that does not pair
# up — refuses the export before anything reaches the clone.
#
# Local only: the "public" wiki is a bare repository in the scratch directory, gitleaks a stub.
#
#   scripts/tests/export-wiki.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export TMPDIR="$SCRATCH"
export RD_WIKI_SRC="$SCRATCH/wiki"
export RD_PUBLIC_WIKI_DIR="$SCRATCH/public-wiki"
export RD_PUBLIC_WIKI_REMOTE="$SCRATCH/public-wiki.git"
export GITLEAKS="$SCRATCH/gitleaks"
# The export takes its author from `git config` of this checkout; a machine without one still runs.
export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=user.name GIT_CONFIG_VALUE_0=test
export GIT_CONFIG_KEY_1=user.email GIT_CONFIG_VALUE_1=test@example.invalid
printf '#!/bin/sh\nexit 0\n' > "$GITLEAKS"
chmod +x "$GITLEAKS"
git init -q --bare -b master "$RD_PUBLIC_WIKI_REMOTE"

# A wiki with one of everything: a public page with a private section, a private page, and a
# sidebar that lists both pages.
write_wiki() {
    rm -rf "$RD_WIKI_SRC"
    mkdir -p "$RD_WIKI_SRC/using"
    cat > "$RD_WIKI_SRC/home.md" <<'EOF'
# Home

See [downloads](using/downloads).
EOF
    cat > "$RD_WIKI_SRC/_sidebar.md" <<'EOF'
- [Home](home)
- [Downloads](using/downloads)
- [Maintainer notes](using/maintainer)
EOF
    cat > "$RD_WIKI_SRC/using/downloads.md" <<'EOF'
# Downloads

Public text.

<!-- private -->
## Release checklist

Only for the maintainer.
<!-- /private -->

## Queue

More public text.
EOF
    cat > "$RD_WIKI_SRC/using/maintainer.md" <<'EOF'
<!-- private page -->
# Maintainer notes

Secret process.
EOF
}
commit_wiki() {
    git -C "$RD_WIKI_SRC" init -q -b main 2> /dev/null || true
    git -C "$RD_WIKI_SRC" add -A
    git -C "$RD_WIKI_SRC" commit -q -m wiki
}
export_wiki() { run_status "$ROOT/scripts/export-wiki.sh" 1.0.0; }
exported() { git -C "$RD_PUBLIC_WIKI_REMOTE" show "master:$1" 2> /dev/null; }

write_wiki
commit_wiki
rm -rf "$RD_PUBLIC_WIKI_DIR"
export_wiki
expect_status "a well-formed handbook is exported" 0
# The export commits into the local clone; the test pushes it into the scratch "remote" itself.
git -C "$RD_PUBLIC_WIKI_DIR" push -q origin master
expect "the private page is left out" "" "$(exported using/maintainer.md)"
expect "and its sidebar line with it" "- [Home](Home)|- [Downloads](downloads)|" \
    "$(exported _Sidebar.md | tr '\n' '|')"
expect "the private section is cut, one blank line where it stood" \
    "# Downloads||Public text.||## Queue||More public text.|" "$(exported using/downloads.md | tr '\n' '|')"
expect_true "no marker text reaches the export" '! git -C "$RD_PUBLIC_WIKI_REMOTE" grep -q "private" master'
expect "the commit names the version" "Handbook for 1.0.0" "$(git -C "$RD_PUBLIC_WIKI_REMOTE" log -1 --format=%s master)"

refused() {
    local name="$1" needle="$2"
    commit_wiki
    local before; before="$(git -C "$RD_PUBLIC_WIKI_DIR" rev-parse HEAD)"
    export_wiki
    expect_status "$name: refused" 1
    expect_output "$name: named" "$needle"
    expect "$name: the clone is untouched" "$before" "$(git -C "$RD_PUBLIC_WIKI_DIR" rev-parse HEAD)"
    write_wiki
}

printf '\nAsk the [maintainer](maintainer).\n' >> "$RD_WIKI_SRC/home.md"
refused "a link to a private page" "link to the private page using/maintainer.md"

printf '\nSee [the checklist](using/downloads#release-checklist).\n' >> "$RD_WIKI_SRC/home.md"
refused "a link into a private section" "link into a private section"

printf '\n<!-- private -->\nnever closed\n' >> "$RD_WIKI_SRC/home.md"
refused "an unclosed section" "Home.md:5: private section never closed"

printf '\n<!-- private -->\n<!-- private -->\nx\n<!-- /private -->\n' >> "$RD_WIKI_SRC/home.md"
refused "a nested section" "private section opened inside the one from line 5"

printf '\n<!-- /private -->\n' >> "$RD_WIKI_SRC/home.md"
refused "a close without an open" "without an open private section"

printf '\n<!-- private page -->\n' >> "$RD_WIKI_SRC/home.md"
refused "a page marker below the first line" "counts only as the first line"

printf '\nWrite `<!-- private -->` to hide a section.\n' >> "$RD_WIKI_SRC/home.md"
refused "marker text inside a sentence" "marker text outside a marker line would be published"

finish_tests export-wiki
