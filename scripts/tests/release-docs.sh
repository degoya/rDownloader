#!/usr/bin/env bash
#
# scripts/release-docs.sh against a scratch repository (RD-1270-15): the CHANGELOG cut with the
# plugin versions raised since the previous tag (grouped by version, wrapped at 100 columns, a
# `### Changed` made where there is none), the release notes' draft comment removed, the next
# minor's draft section, and what is refused; release-notes.sh --check and
# plugin-release-notes.sh --check pass on the result.
#
#   scripts/tests/release-docs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
export LC_ALL=C.UTF-8

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib" "$TREE/plugins/common"
cp "$ROOT/scripts/release-docs.sh" "$ROOT/scripts/release-notes.sh" "$ROOT/scripts/plugin-release-notes.sh" \
    "$TREE/scripts/"
cp "$ROOT/scripts/lib/release-docs.py" "$ROOT/scripts/lib/release-tag.sh" "$TREE/scripts/lib/"
cat > "$TREE/scripts/build-plugins.sh" <<'STUB'
#!/usr/bin/env bash
if [[ "$1" == --list-examples ]]; then echo example-echo; else
    (cd "$(dirname "$0")/../plugins" && ls -d -- */manifest.toml | sed 's,/.*,,' | grep -v '^example-'); fi
STUB
chmod +x "$TREE/scripts/build-plugins.sh"
git -C "$TREE" init -q
gitc() { git -C "$TREE" -c user.name=test -c user.email=test@example.test "$@"; }

# A plugin with its manifest and the CHANGES.md section of its version.
plugin() {
    mkdir -p "$TREE/plugins/$1"
    printf 'manifest_version = 3\nname = "%s"\nversion = "%s"\n\n[metadata]\nversion_note = "x"\n' \
        "$1" "$2" > "$TREE/plugins/$1/manifest.toml"
    printf '# Changes\n\n## %s\n\nMaintenance release: internal changes only, no change in behaviour.\n' \
        "$2" > "$TREE/plugins/$1/CHANGES.md"
}
plugin alpha 0.1.0
plugin bravo-crawler 0.1.0
plugin charlie 0.2.0
plugin delta-unchanged 0.3.0
plugin example-echo 0.7.9
echo 'not a plugin' > "$TREE/plugins/common/lib.rs"
cat > "$TREE/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

## [1.0.0] - 2026-10-01

### Added

- The first release.
EOF
cat > "$TREE/RELEASE-NOTES.md" <<'EOF'
# Release notes

<!--
How a section is written: "## X.Y.Z", newest first.
-->

## 1.0.0

- The first release.
EOF
gitc add -A && gitc commit -qm one && gitc tag v1.0.0

docs() { "$TREE/scripts/release-docs.sh" "$@"; }

# No plugin moved yet: the list is empty.
expect "nothing raised, no list" "" "$(docs --plugins 1.1.0)"

# The wave: four plugins raised, one new, one left; [Unreleased] with Added and Fixed only.
plugin alpha 0.1.1
plugin bravo-crawler 0.1.1
plugin charlie 0.2.1
plugin example-echo 0.7.10
plugin echo-new 0.1.0
cat > "$TREE/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

### Added

- **A thing (RD-1-01).** Text that runs over
  two lines.

- **Another thing (RD-1-02).** Text.

### Fixed

- **A fix (RD-1-03).** Text.

## [1.0.0] - 2026-10-01

### Added

- The first release.
EOF
cat > "$TREE/RELEASE-NOTES.md" <<'EOF'
# Release notes

<!--
How a section is written: "## X.Y.Z", newest first.
-->

## 1.1.0

<!-- draft: the notes of the version in the making -->

- Something new.

## 1.0.0

- The first release.
EOF
gitc add -A && gitc commit -qm wave

expect "the sentence: grouped by version, ascending, names sorted" \
    "The versions this release raises: \`echo-new\` 0.1.0; \`alpha\` and \`bravo-crawler\` 0.1.1; \`charlie\` 0.2.1; \`example-echo\` 0.7.10." \
    "$(docs --plugins v1.1.0)"

run_status docs 1.1.0 --date 2026-10-12
expect_status "the cut: exit 0" 0
expect_output "names the cut" "CHANGELOG.md: [Unreleased] is now [1.1.0] - 2026-10-12"
expect_output "names the draft removed" "RELEASE-NOTES.md: the draft comment of 1.1.0 removed"
expect "the CHANGELOG cut, a Changed section made before Fixed" "# Changelog

## [Unreleased]

## [1.1.0] - 2026-10-12

### Added

- **A thing (RD-1-01).** Text that runs over
  two lines.

- **Another thing (RD-1-02).** Text.

### Changed

- **Plugin versions.** The versions this release raises: \`echo-new\` 0.1.0; \`alpha\` and
  \`bravo-crawler\` 0.1.1; \`charlie\` 0.2.1; \`example-echo\` 0.7.10.

### Fixed

- **A fix (RD-1-03).** Text.

## [1.0.0] - 2026-10-01

### Added

- The first release." "$(cat "$TREE/CHANGELOG.md")"
expect "the release notes without the draft comment" "## 1.1.0

- Something new.

## 1.0.0" "$(sed -n '/^## 1.1.0/,/^## 1.0.0/p' "$TREE/RELEASE-NOTES.md")"

run_status "$TREE/scripts/release-notes.sh" --check --version 1.1.0
expect_status "release-notes.sh --check --version on the result" 0
run_status "$TREE/scripts/plugin-release-notes.sh" --check
expect_status "plugin-release-notes.sh --check on the result" 0

run_status docs 1.1.0
expect_status "a second cut: exit 1" 1
expect_output "names the section cut already" "has a [1.1.0] section already; nothing written"

# The next draft: on top, empty, passing the check; once only; none for a beta.
gitc add -A && gitc commit -qm cut && gitc tag v1.1.0
run_status docs --next-draft 1.1.0
expect_status "the next draft: exit 0" 0
expect_output "names it" "RELEASE-NOTES.md: 1.2.0 started as a draft"
expect "the 1.2.0 draft above 1.1.0" "## 1.2.0

<!-- draft: the notes of the version in the making -->

## 1.1.0" "$(sed -n '/^## 1.2.0/,/^## 1.1.0/p' "$TREE/RELEASE-NOTES.md")"
run_status "$TREE/scripts/release-notes.sh" --check
expect_status "the empty draft passes release-notes.sh --check" 0
gitc add -A && gitc commit -qm draft
run_status docs --next-draft 1.1.0
expect_output "a second time: there already" "1.2.0 has its section already"
expect "a second time: nothing written" "" "$(git -C "$TREE" status --porcelain)"
run_status docs --next-draft 1.1.1
expect_output "a patch release's next draft is the same minor" "1.2.0 has its section already"
run_status docs --next-draft 1.2.0-beta.1
expect_output "a beta adds no draft" "no next draft for the pre-release 1.2.0-beta.1"
expect "a beta wrote nothing" "" "$(git -C "$TREE" status --porcelain)"

# A beta of 1.2.0: its section, the release notes' draft of 1.2.0 finished; Changed exists.
plugin alpha 0.1.2
printf '# Release notes\n\n## 1.2.0\n\n<!--\ndraft: multi-line\n-->\n\n- Better.\n\n## 1.1.0\n\n- Something new.\n' \
    > "$TREE/RELEASE-NOTES.md"
python3 - "$TREE/CHANGELOG.md" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
text = text.replace("## [Unreleased]\n", "## [Unreleased]\n\n### Changed\n\n- **A change (RD-2-01).** Text.\n", 1)
open(path, "w", encoding="utf-8").write(text)
PY
run_status docs 1.2.0-beta.1 --date 2026-10-20
expect_status "a beta's cut: exit 0" 0
expect "the beta section, the list last in Changed" "## [1.2.0-beta.1] - 2026-10-20

### Changed

- **A change (RD-2-01).** Text.

- **Plugin versions.** The versions this release raises: \`alpha\` 0.1.2.

## [1.1.0] - 2026-10-12" "$(sed -n '/^## \[1.2.0-beta.1\]/,/^## \[1.1.0\]/p' "$TREE/CHANGELOG.md")"
expect "a beta finishes its release's notes, a multi-line comment too" "## 1.2.0

- Better.

## 1.1.0" "$(sed -n '/^## 1.2.0/,/^## 1.1.0/p' "$TREE/RELEASE-NOTES.md")"

# The stable release after it: the beta sections named for consolidation; a missing notes section
# named, not refused.
printf '# Release notes\n\n## 1.1.0\n\n- Something new.\n' > "$TREE/RELEASE-NOTES.md"
printf '\n### Fixed\n\n- **Late (RD-2-02).** Text.\n' > "$SCRATCH/late"
sed -i "/^## \[Unreleased\]/r $SCRATCH/late" "$TREE/CHANGELOG.md"
run_status docs 1.2.0 --date 2026-10-22
expect_status "a stable release after its beta: exit 0" 0
expect_output "the beta section to consolidate" "consolidate [1.2.0-beta.1] into [1.2.0] by hand"
expect_output "the notes section missing" "RELEASE-NOTES.md has no section '## 1.2.0'; write it before the chain"

# Refused, nothing written.
run_status docs 1.3.0
expect_status "an empty [Unreleased]: exit 1" 1
expect_output "names it" "[Unreleased] is empty; nothing to release as 1.3.0"
sed -i 's/^## \[Unreleased\]$/## Unreleased/' "$TREE/CHANGELOG.md"
run_status docs 1.3.0
expect_status "no [Unreleased]: exit 1" 1
for args in "" "1.3" "1.3.0-rc.1" "--plugins" "--next-draft 1.3" "1.3.0 --date tomorrow"; do
    # shellcheck disable=SC2086  # the arguments split on purpose
    run_status docs $args
    expect_status "usage or version error: '${args}'" 2
done

# A long list wraps at 100 columns, every name whole.
for n in $(seq 1 30); do plugin "plugin-with-a-long-name-$n" 0.4.0; done
cat > "$TREE/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

### Changed

- **A change (RD-3-01).** Text.

## [1.2.0] - 2026-10-22
EOF
gitc add -A && gitc commit -qm long && gitc tag v1.2.0
for n in $(seq 1 30); do plugin "plugin-with-a-long-name-$n" 0.4.1; done
run_status docs 1.3.0 --date 2026-10-30
expect_status "a long list: exit 0" 0
expect "no line over 100 columns" "" "$(awk 'length > 100' "$TREE/CHANGELOG.md")"
expect "the wrapped point is the sentence" "$(docs --plugins 1.3.0)" \
    "$(sed -n '/^- \*\*Plugin versions/,/^$/p' "$TREE/CHANGELOG.md" | sed 's/^- \*\*Plugin versions\.\*\* //; s/^  //' | tr '\n' ' ' | sed 's/ *$//')"

# No previous tag: no list.
git -C "$TREE" tag -d v1.0.0 v1.1.0 v1.2.0 >/dev/null
expect "no previous release, no list" "" "$(docs --plugins 1.3.0)"

finish_tests release-docs
