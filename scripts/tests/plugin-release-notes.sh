#!/usr/bin/env bash
#
# scripts/plugin-release-notes.sh against scratch plugins (RD-1140-03): the section of a version
# in a plugin's CHANGES.md becomes its notes, as plain text, and nothing else does; --check
# refuses a missing file, a version without its section, and a section that is too long or names
# a job, a path, a Rust identifier or other plugins.
#
#   scripts/tests/plugin-release-notes.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# Characters, not bytes: the ellipsis is three bytes.
export LC_ALL=C.UTF-8

# A scratch checkout: the script, a stand-in for build-plugins.sh's lists of bundled plugins and
# of examples, and four plugins.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts"
cp "$ROOT/scripts/plugin-release-notes.sh" "$TREE/scripts/"
cat > "$TREE/scripts/build-plugins.sh" <<'STUB'
#!/usr/bin/env bash
if [[ "$1" == --list-examples ]]; then echo example-echo; else printf '%s\n' alpha bravo-jobs charlie; fi
STUB
chmod +x "$TREE/scripts/build-plugins.sh"
plugin() {
    mkdir -p "$TREE/plugins/$1"
    printf 'manifest_version = 3\nname = "%s"\nversion = "%s"\n' "$1" "$2" > "$TREE/plugins/$1/manifest.toml"
}
plugin alpha 0.2.2
plugin bravo-jobs 0.1.1
plugin charlie 1.0.0
plugin example-echo 0.3.0
printf '# Changes\n\n## 0.3.0\n\nThe reference plugin for authors.\n' > "$TREE/plugins/example-echo/CHANGES.md"
cat > "$TREE/plugins/alpha/CHANGES.md" <<'EOF'
# Changes

## 0.2.2

Links to folders are **recognised** again, see
[the handbook](https://example.test/handbook).

## 0.2.1

Maintenance release: internal changes only, no change in behaviour.
EOF
cat > "$TREE/plugins/bravo-jobs/CHANGES.md" <<'EOF'
# Changes

## 0.1.1

A transfer the service still prepares is no longer reported as failed.
EOF
cat > "$TREE/plugins/charlie/CHANGES.md" <<'EOF'
# Changes

## 1.0.0

The first release.
EOF
notes() { "$TREE/scripts/plugin-release-notes.sh" "$@"; }

expect "the section, plain text, wrapped lines joined" \
    "Links to folders are recognised again, see the handbook." "$(notes alpha 0.2.2)"
expect "an older section" \
    "Maintenance release: internal changes only, no change in behaviour." "$(notes alpha 0.2.1)"
expect "a version without a section has no notes" "" "$(notes alpha 0.2.3)"
expect "a longer version is another version" "" "$(notes alpha 0.2.21)"
expect "a plugin without the file has no notes" "" "$(notes delta 0.1.0)"
expect "another file names the source" \
    "A transfer the service still prepares is no longer reported as failed." \
    "$(notes alpha 0.1.1 "$TREE/plugins/bravo-jobs/CHANGES.md")"

printf '# Changes\n\n## 1.0.0\n\n%s.\n' "$(printf '%2500s' '' | tr ' ' x)" > "$SCRATCH/LONG.md"
long="$(notes long 1.0.0 "$SCRATCH/LONG.md")"
expect "an overlong section is cut to the index limit" "2000" "${#long}"
expect "and says so" "…" "${long: -1}"

run_status notes alpha
expect_status "a missing version is a usage error" 2

# --check: the four plugins as written pass.
check() { "$TREE/scripts/plugin-release-notes.sh" --check "$@"; }
run_status check
expect_status "every version has short notes: exit 0" 0

# The examples are checked too, though never bundled: they are what an author copies.
mv "$TREE/plugins/example-echo/CHANGES.md" "$SCRATCH/example.md"
run_status check
expect_status "an example without CHANGES.md: exit 1" 1
expect_output "names the example" 'plugins/example-echo/CHANGES.md: missing; start it with "## 0.3.0"'
mv "$SCRATCH/example.md" "$TREE/plugins/example-echo/CHANGES.md"

# A raised version without its section, and a plugin without the file.
plugin charlie 1.0.1
run_status check
expect_status "a raised version without its section: exit 1" 1
expect_output "names the section to write" 'plugins/charlie/CHANGES.md: no section "## 1.0.1"'
plugin charlie 1.0.0
mv "$TREE/plugins/charlie/CHANGES.md" "$SCRATCH/charlie.md"
run_status check charlie
expect_status "a plugin without CHANGES.md: exit 1" 1
expect_output "says how to start it" 'plugins/charlie/CHANGES.md: missing; start it with "## 1.0.0"'
mv "$SCRATCH/charlie.md" "$TREE/plugins/charlie/CHANGES.md"

# One forbidden section at a time, as the section of charlie 1.0.0; the line it has to name.
refused() {
    local name="$1" text="$2" needle="$3"
    printf '# Changes\n\n## 1.0.0\n\n%s\n' "$text" > "$TREE/plugins/charlie/CHANGES.md"
    run_status check charlie
    expect_status "$name: exit 1" 1
    expect_output "$name: named" "plugins/charlie/CHANGES.md:3: 1.0.0 $needle"
}
refused "too long" "$(printf 'Faster. %.0s' {1..40})" "is 319 characters, at most 300"
refused "a job number" "Downloads resume again (RD-1140-03)." "names a job"
refused "a path" "The fix is in src/resolver/api.rs now." "names a path"
refused "a directory of the repository" "Read plugins/charlie for details." "names a path"
# shellcheck disable=SC2016  # the backticks are the text
refused "a code span" 'The `Retry-After` header counts.' "names a Rust identifier"
refused "a snake_case name" "The retry_after value counts." "names a Rust identifier"
refused "a path of a Rust item" "Uses FreeFlow::run now." "names a Rust identifier"
refused "a list of other plugins" "Shared with alpha and bravo-jobs now." "lists other plugins (alpha, bravo-jobs)"
refused "German" "Downloads laufen schneller über das Netz." "is not English"
refused "no sentence" "Faster downloads" "does not end a sentence"
refused "maintenance in other words" "Maintenance release only." "says maintenance in other words"

# What passes: one other plugin, a slash that is no path, a version in the text.
printf '# Changes\n\n## 1.0.0\n\n%s\n' \
    "Works with alpha links; a 429/5xx answer without a stated wait pauses before HTTP/2 retries in 1.0.0." \
    > "$TREE/plugins/charlie/CHANGES.md"
run_status check charlie
expect_status "one other plugin, a slash and a version pass" 0

printf '# Changes\n\n## 1.0.0\n\nOne.\n\n## 1.0.0\n\nTwo.\n\n## next\n\nThree.\n' > "$TREE/plugins/charlie/CHANGES.md"
run_status check charlie
expect_status "a second section and a heading that is no version: exit 1" 1
expect_output "the second section" "plugins/charlie/CHANGES.md:7: 1.0.0 has a second section"
expect_output "the heading" "plugins/charlie/CHANGES.md:11: next is not a version"

# Notes are no source of the component: writing a section changes no source hash, so it asks for
# no version of its own (--list-unbumped, the stamps, CI's component cache).
# shellcheck source=../lib/plugin-stamp.sh
source "$ROOT/scripts/lib/plugin-stamp.sh"
mkdir -p "$TREE/crates/rd-plugin-api/wit" "$TREE/plugins/alpha/src"
printf 'package demo;\n' > "$TREE/crates/rd-plugin-api/wit/demo.wit"
printf '[package]\nname = "alpha"\n' > "$TREE/plugins/alpha/Cargo.toml"
printf 'fn main() {}\n' > "$TREE/plugins/alpha/src/lib.rs"
hash_before="$(cd "$TREE" && source_hash alpha)"
printf '\n## 0.2.3\n\nAnother sentence.\n' >> "$TREE/plugins/alpha/CHANGES.md"
expect "a new section leaves the source hash alone" "$hash_before" "$(cd "$TREE" && source_hash alpha)"

# The real plugins: every bundled one and every example passes.
run_status "$ROOT/scripts/plugin-release-notes.sh" --check
expect_status "every bundled plugin and example has short notes for the version it carries" 0

finish_tests plugin-release-notes
