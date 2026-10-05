# shellcheck shell=bash
#
# The "same version, same content" comparison of scripts/build-plugins.sh (RD-120-47): the
# version a plugin declares, and which member of a signed package differs from what the plugin
# would be packaged from now. --list-unbumped and the packaging refusal both ask it.
#
# Expects the working directory at the checkout root.

# The version plugin $1 declares in its own manifest.
manifest_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' "plugins/$1/manifest.toml" | head -1
}

# Which member of the signed package $1 differs from what plugin $2 would be packaged from, with
# component $3: `manifest.toml`, `component.wasm` or `locales`, printed; nothing when they agree.
#
# Byte for byte, member by member. The packager stores all three verbatim (see
# crates/rd-plugin-host/src/packager.rs), so no re-signing is needed and none happens; comparing
# the archive itself would fail on nothing but the signature. The locales count because they
# ship inside the same package and an installation that keeps it keeps its texts too.
package_drift() {
    local package="$1" directory="plugins/$2" component="$3" packaged present language
    if ! unzip -p "$package" manifest.toml 2> /dev/null | cmp -s - "$directory/manifest.toml"; then
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
