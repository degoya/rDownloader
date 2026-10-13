# shellcheck shell=bash
#
# The "same version, same content" comparison of scripts/build-plugins.sh (RD-120-47): the
# version a plugin declares, and which member of a signed package differs from what the plugin
# would be packaged from now. --list-unbumped and the packaging refusal both ask it.
#
# Expects from scripts/build-plugins.sh, which sources it: PACKAGES and component_path
# (lib/plugin-stamp.sh), and the working directory at the checkout root.

# The version plugin $1 declares in its own manifest.
manifest_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' "plugins/$1/manifest.toml" | head -1
}

# The manifest `rd-pack plugin package --defaults plugins/metadata.toml` packages for plugin $1
# (RD-1280-18): every shared field the plugin's manifest leaves out, taken from
# plugins/metadata.toml as its line stands there -- the top-level ones above the manifest under
# the marker line, the [metadata] ones directly below its own `[metadata]` line. The same rule as
# `complete_manifest` in crates/rd-plugin-host/src/manifest/complete.rs, whose test compares the
# two over every plugin (`--completed-manifest`). Without plugins/metadata.toml, the manifest as
# it is.
completed_manifest() {
    local manifest="plugins/$1/manifest.toml" defaults="plugins/metadata.toml"
    if [[ ! -f "$defaults" ]]; then
        cat "$manifest"
        return
    fi
    awk -v marker="# Completed from the shared plugin fields by \`rd-pack plugin package\` (RD-1280-18)." '
        function trim(text) { sub(/^[ \t]+/, "", text); sub(/[ \t\r]+$/, "", text); return text }
        function header(text) {
            if (text !~ /^\[[A-Za-z0-9_.-]+\][ \t]*(#.*)?$/) return ""
            sub(/^\[/, "", text); sub(/\].*$/, "", text); return text
        }
        function key(text) {
            if (text !~ /^[A-Za-z0-9_-]+[ \t]*=/) return ""
            sub(/[ \t]*=.*$/, "", text); return text
        }
        FNR == NR {
            line = trim($0)
            if (line == "" || line ~ /^#/) next
            if (line ~ /^\[/) { section = header(line); next }
            if (section == "metadata") { meta_key[++metas] = key(line); meta_line[metas] = line }
            else { root_key[++roots] = key(line); root_line[roots] = line }
            next
        }
        {
            source[++lines] = $0
            line = trim($0)
            # A line opening with `[` that is no header is an element of a multi-line array.
            if (line ~ /^\[/) {
                if (header(line) != "") current = header(line)
                if (current == "metadata") has_metadata = 1
                next
            }
            name = key(line)
            if (name == "") next
            if (current == "") present_root[name] = 1
            if (current == "metadata") present_meta[name] = 1
        }
        END {
            for (i = 1; i <= roots; i++) if (!(root_key[i] in present_root)) missing_root[++adds] = root_line[i]
            for (i = 1; i <= metas; i++) if (!(meta_key[i] in present_meta)) missing_meta[++fills] = meta_line[i]
            if (adds > 0) { print marker; for (i = 1; i <= adds; i++) print missing_root[i] }
            inserted = (fills == 0)
            for (i = 1; i <= lines; i++) {
                print source[i]
                if (!inserted && header(trim(source[i])) == "metadata") {
                    for (j = 1; j <= fills; j++) print missing_meta[j]
                    inserted = 1
                }
            }
            if (!inserted && !has_metadata) {
                print ""; print "[metadata]"
                for (j = 1; j <= fills; j++) print missing_meta[j]
            }
        }
    ' "$defaults" "$manifest"
}

# Which member of the signed package $1 differs from what plugin $2 would be packaged from, with
# component $3: `manifest.toml`, `component.wasm` or `locales`, printed; nothing when they agree.
# The manifest is compared in its completed form (`completed_manifest`), the one packaged.
#
# Byte for byte, member by member. The packager stores all three verbatim (see
# crates/rd-plugin-host/src/packager.rs), so no re-signing is needed and none happens; comparing
# the archive itself would fail on nothing but the signature. The locales count because they
# ship inside the same package and an installation that keeps it keeps its texts too.
package_drift() {
    local package="$1" directory="plugins/$2" component="$3" packaged present language
    if ! cmp -s <(unzip -p "$package" manifest.toml 2> /dev/null) <(completed_manifest "$2"); then
        echo manifest.toml
        return
    fi
    if ! unzip -p "$package" component.wasm 2> /dev/null | cmp -s - "$component"; then
        echo component.wasm
        return
    fi
    packaged="$(unzip -Z1 "$package" 2> /dev/null | sed -n 's|^locales/\(.*\)\.json$|\1|p' | sort || true)"
    present=""
    if [[ -d "$directory/locales" ]]; then
        present="$(find "$directory/locales" -maxdepth 1 -type f -name '*.json' -printf '%f\n' \
            | sed 's/\.json$//' | sort)"
    fi
    if [[ "$packaged" != "$present" ]]; then
        echo locales
        return
    fi
    while read -r language; do
        [[ -n "$language" ]] || continue
        if ! unzip -p "$package" "locales/$language.json" 2> /dev/null \
            | cmp -s - "$directory/locales/$language.json"; then
            echo locales
            return
        fi
    done <<< "$packaged"
}

# The plugins whose built component, manifest or locales differ from a signed package that
# already carries their current version: `<name> <version> <member> <package>`, one per line.
# Without names every plugin; with names only those, which is how check.sh keeps it to the
# change set. A plugin with no package of its version, or no built component, has nothing to
# compare and is not named — the latter is --list-missing's question.
list_unbumped() {
    local -a names=("$@")
    local manifest name version package component member
    if [[ ${#names[@]} -eq 0 ]]; then
        for manifest in plugins/*/manifest.toml; do
            names+=("$(basename "$(dirname "$manifest")")")
        done
    fi
    for name in "${names[@]}"; do
        [[ -f "plugins/$name/manifest.toml" ]] || continue
        version="$(manifest_version "$name")"
        package="$PACKAGES/$name-$version.rdplug"
        component="$(component_path "$name")"
        [[ -n "$version" && -f "$package" && -f "$component" ]] || continue
        member="$(package_drift "$package" "$name" "$component")"
        [[ -z "$member" ]] || echo "$name $version $member $package"
    done
}
