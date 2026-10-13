# shellcheck shell=bash
#
# Fills a packaging template's `@NAME@` placeholders (RD-1280-15): the one copy of what
# scripts/package-repo.sh and scripts/package-managers.sh each kept byte for byte.
#
#   source "$ROOT/scripts/lib/render-template.sh"
#   rd_render_template <template> <output> NAME=value...
#
# A placeholder left over after every pair is an error: the template names a value the caller
# does not know, and a package manifest with `@SHA256@` in it would be published as it is.

rd_render_template() {
    local template="$1" output="$2" content pair
    shift 2
    content="$(< "$template")"
    for pair in "$@"; do
        content="${content//@${pair%%=*}@/${pair#*=}}"
    done
    if [[ "$content" =~ @[A-Z0-9_]+@ ]]; then
        echo "error: $template keeps ${BASH_REMATCH[0]} after rendering" >&2
        return 1
    fi
    printf '%s\n' "$content" > "$output"
}
