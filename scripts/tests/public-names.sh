#!/usr/bin/env bash
#
# The public tree names JDownloader, `JD` as a word or a path from its source tree only where
# scripts/public-names-allow.txt allows it (RD-1250-11): the guard scripts/lib/public-names.py over this checkout first, then against
# hand-written cases in a scratch repository — a listed mention passes, an unlisted one and a
# listed file's other line fail, an excluded path is not read, `## [Unreleased]` and a draft
# section of the release notes fail while released sections pass, and an entry no mention needs
# any more fails too.
#
# Pure python3, git and bash: it runs in a second. A names file takes the place of the built-in terms: the
# release pages (RD-1480-23) are held that way.
#
#   scripts/tests/public-names.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/lib/public-names.py"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

run_status python3 "$GUARD" "$ROOT" "$ROOT/scripts/public-exclude.txt" "$ROOT/scripts/public-names-allow.txt"
expect_status "this checkout names it only where the allowlist says" 0
[[ "$status" -eq 0 ]] || printf '%s\n' "$output"

# The release pages (RD-1480-23): the list is left out of the public repository, where there is
# nothing to read and the case is skipped.
NAMES="$ROOT/scripts/release-site-names.txt"
if [[ -f "$NAMES" ]]; then
    run_status python3 "$GUARD" "$ROOT" "$ROOT/scripts/public-exclude.txt" \
        "$ROOT/scripts/release-site-names-allow.txt" "$NAMES"
    expect_status "this checkout names no release page outside its allowlist" 0
    [[ "$status" -eq 0 ]] || printf '%s\n' "$output"
    run_status python3 - "$ROOT/scripts/public-exclude.txt" <<'PY'
import sys

entries = open(sys.argv[1], encoding="utf-8").read().splitlines()
wanted = ["scripts/release-site-names.txt", "scripts/release-site-names-allow.txt"]
sys.exit(0 if all(name in entries for name in wanted) else 1)
PY
    expect_status "the list of release pages is left out of the public repository" 0
fi

TREE="$SCRATCH/tree"
mkdir -p "$TREE/docs" "$TREE/src"
git -C "$TREE" init -q
cat > "$SCRATCH/exclude.txt" <<'LIST'
docs/
LIST
cat > "$TREE/CHANGELOG.md" <<'TEXT'
# Changelog

## [Unreleased]

- Something new.

## [1.0.0] - 2026-01-01

- Grouping after JDownloader.
TEXT
cat > "$TREE/RELEASE-NOTES.md" <<'TEXT'
# Release notes

## 1.0.0

- Grouping as in JDownloader.
TEXT
printf 'format = "JDownloader .crawljob"\n' > "$TREE/src/format.rs"
printf 'Internal notes on JDownloader.\n' > "$TREE/docs/notes.md"
cat > "$SCRATCH/allow.txt" <<'LIST'
# A comment.
CHANGELOG.md | @released | history
RELEASE-NOTES.md | @released | history
src/*.rs | JDownloader \.crawljob | the format's name
LIST

# guard: runs the guard over the scratch tree, its files staged.
guard() {
    git -C "$TREE" add -A
    run_status python3 "$GUARD" "$TREE" "$SCRATCH/exclude.txt" "$SCRATCH/allow.txt"
}

guard
expect_status "listed mentions, released history and an excluded path pass" 0

printf '// Grouping after JDownloader.\n' >> "$TREE/src/format.rs"
guard
expect_status "a listed file's other line fails" 1
expect_output "and names file and line" "src/format.rs:2: names jdownloader outside the allowlist"
printf 'format = "JDownloader .crawljob"\n' > "$TREE/src/format.rs"

printf '// Waits as JD does.\n' > "$TREE/src/wait.rs"
guard
expect_status "the abbreviation as a word fails" 1
expect_output "and is named" "src/wait.rs:1: names JD outside the allowlist"
printf 'let hash = "3fJDa9"; // a jd_mirror id, jdx\n' > "$TREE/src/wait.rs"
guard
expect_status "jd inside a word or in lower case is not the abbreviation" 0
rm "$TREE/src/wait.rs"

printf '// Read off svn_trunk/src/jd/plugins/hoster/FooCom.java.\n' > "$TREE/src/wait.rs"
guard
expect_status "a path into the source mirror fails" 1
expect_output "and is named" "src/wait.rs:1: names a source-mirror path outside the allowlist"
printf '// The page loads app.javascript; FooCom.javaish is no source file.\n' > "$TREE/src/wait.rs"
guard
expect_status "a word that only starts with java is not a source file" 0
rm "$TREE/src/wait.rs"

printf 'Works like JDownloader.\n' > "$TREE/README.md"
guard
expect_status "an unlisted file fails" 1
expect_output "and is named" "README.md:1: names jdownloader outside the allowlist"
rm "$TREE/README.md"

sed -i 's/- Something new\./- After jdownloader./' "$TREE/CHANGELOG.md"
guard
expect_status "a mention under [Unreleased] fails" 1
expect_output "and is named" "CHANGELOG.md:5: names jdownloader outside the allowlist"
sed -i 's/- After jdownloader\./- Something new./' "$TREE/CHANGELOG.md"

cat > "$TREE/RELEASE-NOTES.md" <<'TEXT'
# Release notes

## 1.1.0

<!-- draft: finished at the release -->

- As in JDownloader.

## 1.0.0

- Grouping as in JDownloader.
TEXT
guard
expect_status "a draft section of the release notes fails" 1
expect_output "and is named" "RELEASE-NOTES.md:7: names jdownloader outside the allowlist"
sed -i '/<!-- draft/d' "$TREE/RELEASE-NOTES.md"
guard
expect_status "a section CHANGELOG.md does not date fails too" 1
expect_output "and is named" "RELEASE-NOTES.md:6: names jdownloader outside the allowlist"
printf '# Release notes\n\n## 1.0.0\n\n- Grouping as in JDownloader.\n' > "$TREE/RELEASE-NOTES.md"

printf 'docs/notes.md | JDownloader | a file the export leaves out\n' >> "$SCRATCH/allow.txt"
guard
expect_status "an entry no mention needs fails" 1
expect_output "and is named" "allow.txt:5: no mention needs this entry any more"
sed -i '$d' "$SCRATCH/allow.txt"

printf 'src/*.rs | JDownloader\n' >> "$SCRATCH/allow.txt"
guard
expect_status "an entry without a reason fails" 1
expect_output "and is named" "allow.txt:5: not '<path> | <expression> | <reason>'"
sed -i '$d' "$SCRATCH/allow.txt"

guard
expect_status "back to the passing tree" 0

# A names file: its terms instead of the built-in ones, any case, in hand-written names.
printf '# Invented.\n\nsome[.-]releases\n' > "$SCRATCH/names.txt"
printf 'CHANGELOG.md | @released | history\n' > "$SCRATCH/names-allow.txt"
# names_guard: runs the guard over the scratch tree with the names file.
names_guard() {
    git -C "$TREE" add -A
    run_status python3 "$GUARD" "$TREE" "$SCRATCH/exclude.txt" "$SCRATCH/names-allow.txt" "$SCRATCH/names.txt"
}
sed -i 's/^- Grouping after JDownloader\.$/- Some-Releases pages./' "$TREE/CHANGELOG.md"
names_guard
expect_status "a names file passes the released history and reads only its own names" 0
printf 'let page = "https://some.releases/x";\n' > "$TREE/src/page.rs"
names_guard
expect_status "a listed name outside the allowlist fails" 1
expect_output "and is named" "src/page.rs:1: names some[.-]releases outside the allowlist"
printf 'let page = "https://releases.example/x";\n' > "$TREE/src/page.rs"
names_guard
expect_status "an example host passes" 0
printf '# Nothing.\n' > "$SCRATCH/names.txt"
names_guard
expect_status "an empty names file fails instead of passing everything" 1

finish_tests public-names
