#!/usr/bin/env bash
#
# Raises plugin versions and writes their CHANGES.md sections in one step (RD-1280-17).
#
#   scripts/plugin-bump.sh --cascade [--note "<text>"] [--dry-run]
#   scripts/plugin-bump.sh <plugin>... [--note "<text>"] [--dry-run]
#
# A plugin's version rises with every change to its component, manifest or locales, or an
# installation keeps the package it already has (RD-120-47). A change to the WIT, a shared plugin
# library or a linked crate changes every component at once; until 1.28 each such cascade was
# 72 to 75 manifests and CHANGES.md files edited by hand.
#
# `--cascade` raises exactly the plugins `scripts/build-plugins.sh --list-unbumped` names: built
# components that differ from the signed package of the version they still declare. It needs the
# components built (`build-plugins.sh --components-only`) and the signed packages beside the main
# checkout (`dist/plugins`); with neither it names nothing and raises nothing. Named plugins are
# raised as asked, drift or not — the way to raise one whose behaviour changed, with its `--note`.
#
# The patch number rises by one (0.1.13 becomes 0.1.14) on the first `version = "…"` line of
# `plugins/<plugin>/manifest.toml`, and a `## <new version>` section goes on top of the plugin's
# CHANGES.md: the maintenance sentence, or `--note`'s text — one to three English sentences of
# what a user notices. Afterwards `scripts/plugin-release-notes.sh --check` runs over the raised
# plugins, so a note that breaks its rules fails here; its exit status is this script's.
# `--dry-run` prints `<plugin> <old> -> <new>` per plugin and writes nothing.
#
# A version that is not plain `X.Y.Z`, or a CHANGES.md that already has the new version's
# section, is refused before anything is written.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAINTENANCE="Maintenance release: internal changes only, no change in behaviour."

usage() {
    echo "usage: scripts/plugin-bump.sh (--cascade | <plugin>...) [--note \"<text>\"] [--dry-run]" >&2
    exit 2
}

cascade=0
dry_run=0
note="$MAINTENANCE"
names=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --cascade) cascade=1 ;;
        --dry-run) dry_run=1 ;;
        --note)
            [[ $# -ge 2 && -n "$2" ]] || usage
            note="$2"
            shift
            ;;
        --note=*)
            note="${1#--note=}"
            [[ -n "$note" ]] || usage
            ;;
        -h | --help) usage ;;
        -*) echo "plugin-bump: unknown argument: $1" >&2; usage ;;
        *) names+=("$1") ;;
    esac
    shift
done
if [[ "$cascade" -eq 1 && ${#names[@]} -gt 0 ]] || [[ "$cascade" -eq 0 && ${#names[@]} -eq 0 ]]; then
    usage
fi

if [[ "$cascade" -eq 1 ]]; then
    # `<name> <version> <member> <package>` per line; the name is all this needs.
    unbumped="$("$ROOT/scripts/build-plugins.sh" --list-unbumped)" || {
        echo "plugin-bump: scripts/build-plugins.sh --list-unbumped failed" >&2
        exit 1
    }
    while read -r name _; do
        [[ -n "$name" ]] && names+=("$name")
    done <<< "$unbumped"
    if [[ ${#names[@]} -eq 0 ]]; then
        echo "plugin-bump: no plugin differs from the signed package of its version; nothing to raise" >&2
        exit 0
    fi
fi

# The version plugin $1 declares, from the first `version = "…"` line of its manifest.
declared() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/plugins/$1/manifest.toml" | head -1
}

# Everything is checked before the first file is written, so a refusal leaves the tree as it was.
plan=()
refused=0
for name in "${names[@]}"; do
    manifest="$ROOT/plugins/$name/manifest.toml"
    if [[ ! -f "$manifest" ]]; then
        echo "plugin-bump: plugins/$name: no such plugin" >&2
        refused=1
        continue
    fi
    old="$(declared "$name")"
    if [[ ! "$old" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        echo "plugin-bump: plugins/$name/manifest.toml: version \"$old\" is not X.Y.Z; raise it by hand" >&2
        refused=1
        continue
    fi
    new="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.$((BASH_REMATCH[3] + 1))"
    changes="$ROOT/plugins/$name/CHANGES.md"
    if [[ -f "$changes" ]] && grep -qx "## ${new//./\\.}[[:space:]]*" "$changes"; then
        echo "plugin-bump: plugins/$name/CHANGES.md already has a section \"## $new\"" >&2
        refused=1
        continue
    fi
    plan+=("$name $old $new")
done
[[ "$refused" -eq 0 ]] || exit 1

for line in "${plan[@]}"; do
    read -r name old new <<< "$line"
    echo "$name $old -> $new"
    [[ "$dry_run" -eq 1 ]] && continue
    manifest="$ROOT/plugins/$name/manifest.toml"
    changes="$ROOT/plugins/$name/CHANGES.md"
    # Only the first `version = ` line: `manifest_version`, `api_version` and `min_app_version`
    # never match, and a later table's own `version` key stays as it is.
    awk -v new="$new" '
        !done && /^version = "/ { print "version = \"" new "\""; done = 1; next }
        { print }
    ' "$manifest" > "$manifest.bump"
    mv "$manifest.bump" "$manifest"
    if [[ ! -f "$changes" ]]; then
        printf '# Changes\n\n## %s\n\n%s\n' "$new" "$note" > "$changes"
        continue
    fi
    # On top of the newest section; a file with no section yet gets it at its end.
    NEW="$new" NOTE="$note" awk '
        !done && /^## / { print "## " ENVIRON["NEW"] "\n\n" ENVIRON["NOTE"] "\n"; done = 1 }
        { print }
        END { if (!done) print "\n## " ENVIRON["NEW"] "\n\n" ENVIRON["NOTE"] }
    ' "$changes" > "$changes.bump"
    mv "$changes.bump" "$changes"
done

[[ "$dry_run" -eq 1 ]] && exit 0
raised=()
for line in "${plan[@]}"; do raised+=("${line%% *}"); done
"$ROOT/scripts/plugin-release-notes.sh" --check "${raised[@]}"
