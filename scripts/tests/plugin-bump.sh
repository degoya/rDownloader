#!/usr/bin/env bash
#
# scripts/plugin-bump.sh against a scratch tree (RD-1280-17): `--cascade` raises exactly the
# plugins build-plugins.sh --list-unbumped names, by one patch number, with the maintenance
# section on top of their CHANGES.md, after which --list-unbumped names none; named plugins are
# raised with `--note`; `--dry-run` writes nothing; a note the release-notes rules refuse fails.
#
#   scripts/tests/plugin-bump.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

MAINTENANCE="Maintenance release: internal changes only, no change in behaviour."

# A scratch checkout: the two scripts and a stand-in for build-plugins.sh. Its --list-unbumped
# names a plugin from `signed` (`<name> <version>`, what the signed packages carry) while the
# manifest still declares that version — the real comparison's outcome for a changed component.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts"
cp "$ROOT/scripts/plugin-bump.sh" "$ROOT/scripts/plugin-release-notes.sh" "$TREE/scripts/"
cat > "$TREE/scripts/build-plugins.sh" <<'STUB'
#!/usr/bin/env bash
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "$1" in
    --list-examples) ;;
    --list-packageable) printf '%s\n' alpha bravo charlie delta ;;
    --list-unbumped)
        while read -r name version; do
            declared="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/plugins/$name/manifest.toml" | head -1)"
            if [[ "$declared" == "$version" ]]; then
                echo "$name $version component.wasm dist/plugins/$name-$version.rdplug"
            fi
        done < "$ROOT/signed"
        ;;
esac
STUB
chmod +x "$TREE/scripts/build-plugins.sh"
plugin() {
    mkdir -p "$TREE/plugins/$1"
    printf 'manifest_version = 3\napi_version = "0.11.0"\nname = "%s"\nversion = "%s"\nmin_app_version = "1.20.0"\n\n[extra]\nversion = "9.9.9"\n' \
        "$1" "$2" > "$TREE/plugins/$1/manifest.toml"
    printf '# Changes\n\n## %s\n\nThe first release.\n' "$2" > "$TREE/plugins/$1/CHANGES.md"
}
plugin alpha 0.1.13
plugin bravo 0.9.9
plugin charlie 1.2.0
plugin delta 0.2.1
printf 'alpha 0.1.13\nbravo 0.9.9\n' > "$TREE/signed"
version_of() { sed -n 's/^version = "\(.*\)"$/\1/p' "$TREE/plugins/$1/manifest.toml" | head -1; }
bump() { "$TREE/scripts/plugin-bump.sh" "$@"; }

run_status bump
expect_status "neither --cascade nor a plugin is a usage error" 2
run_status bump --cascade charlie
expect_status "both at once is a usage error" 2

# --dry-run: the plan, nothing written.
before="$(cat "$TREE/plugins/alpha/manifest.toml" "$TREE/plugins/alpha/CHANGES.md")"
run_status bump --cascade --dry-run
expect_status "--cascade --dry-run passes" 0
expect "and lists the unbumped plugins" $'alpha 0.1.13 -> 0.1.14\nbravo 0.9.9 -> 0.9.10' "$output"
expect "writing nothing" "$before" "$(cat "$TREE/plugins/alpha/manifest.toml" "$TREE/plugins/alpha/CHANGES.md")"

# --cascade: exactly those two, by one patch number, each with the maintenance section on top.
run_status bump --cascade
expect_status "--cascade passes, the notes check included" 0
expect "alpha is raised" "0.1.14" "$(version_of alpha)"
expect "bravo's patch number counts past nine" "0.9.10" "$(version_of bravo)"
expect "charlie, not unbumped, stays" "1.2.0" "$(version_of charlie)"
expect "the first version line only" $'version = "0.1.14"\nversion = "9.9.9"' \
    "$(grep '^version = ' "$TREE/plugins/alpha/manifest.toml")"
expect "the other version keys stay" \
    $'manifest_version = 3\napi_version = "0.11.0"\nmin_app_version = "1.20.0"' \
    "$(grep -E '^(manifest_version|api_version|min_app_version) ' "$TREE/plugins/alpha/manifest.toml")"
expect "the maintenance section on top" \
    "$(printf '# Changes\n\n## 0.1.14\n\n%s\n\n## 0.1.13\n\nThe first release.' "$MAINTENANCE")" \
    "$(cat "$TREE/plugins/alpha/CHANGES.md")"
expect "--list-unbumped names none afterwards" "" "$("$TREE/scripts/build-plugins.sh" --list-unbumped)"
run_status bump --cascade
expect_status "a second --cascade finds nothing" 0
expect_output "and says so" "nothing to raise"
expect "nothing raised twice" "0.1.14" "$(version_of alpha)"

# Named plugins, with a note of their own; a plugin without CHANGES.md gets the file.
rm "$TREE/plugins/delta/CHANGES.md"
run_status bump charlie delta --note "Links to folders are recognised again."
expect_status "named plugins with a note pass" 0
expect "charlie is raised" "1.2.1" "$(version_of charlie)"
expect "with the note on top" \
    "$(printf '# Changes\n\n## 1.2.1\n\nLinks to folders are recognised again.\n\n## 1.2.0\n\nThe first release.')" \
    "$(cat "$TREE/plugins/charlie/CHANGES.md")"
expect "a missing CHANGES.md is started" \
    "$(printf '# Changes\n\n## 0.2.2\n\nLinks to folders are recognised again.')" \
    "$(cat "$TREE/plugins/delta/CHANGES.md")"

# Refusals before any write: an unknown plugin, a version not X.Y.Z, an existing section.
run_status bump charlie echo
expect_status "an unknown plugin is refused" 1
expect "and nothing is written, not even for the known one" "1.2.1" "$(version_of charlie)"
sed -i 's/^version = "0.2.2"$/version = "0.3.0-beta.1"/' "$TREE/plugins/delta/manifest.toml"
run_status bump delta
expect_status "a pre-release version is refused" 1
expect_output "and named" "is not X.Y.Z"
printf '# Changes\n\n## 1.2.2\n\nAlready written.\n\n' > "$SCRATCH/head"
cat "$SCRATCH/head" "$TREE/plugins/charlie/CHANGES.md" > "$SCRATCH/changes"
cp "$SCRATCH/changes" "$TREE/plugins/charlie/CHANGES.md"
run_status bump charlie
expect_status "an existing section of the new version is refused" 1
expect "and the manifest keeps its version" "1.2.1" "$(version_of charlie)"

# The release-notes rules hold a note: one naming a job fails, after the write.
run_status bump alpha --note "Fixes RD-1234."
expect_status "a note the rules refuse fails" 1
expect_output "the release-notes check names it" "names a job"

finish_tests plugin-bump
