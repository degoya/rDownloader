#!/usr/bin/env bash
#
# Verifies that a built plugin component imports nothing outside the `rdownloader:plugin`
# contract — above all no `wasi:` interface.
#
# Why this is not a `grep` over the binary: the guard this replaces was
# `grep -a -q 'wasi:' "$wasm"`, which searches every byte of the file. It cannot tell an
# import from a string, so any plugin whose data section happens to contain the text — an
# error message, a locale string, a dependency's diagnostic — is rejected with "imports
# wasi:" while importing nothing at all. A module with zero imports and the literal
# "error: wasi: interfaces are not available" in a data segment fails it. The import section
# is a structured part of the component; reading it is the only check that answers the
# question actually being asked.
#
# `wasm-tools` is optional. Without it the byte scan runs instead, loudly, so a machine that
# lacks the tool still catches the obvious case rather than silently checking nothing — the
# same bargain scripts/check.sh makes for nextest and sqlx.
#
# Usage:
#   scripts/check-plugin-imports.sh <component.wasm> [<component.wasm> ...]
#
set -euo pipefail

# Interface namespaces a plugin is allowed to import. Everything the host offers lives under
# `rdownloader:plugin`; anything else means the component reached for a capability the sandbox
# does not grant and the manifest does not declare.
ALLOWED_PREFIX="rdownloader:plugin/"

[[ $# -gt 0 ]] || { echo "usage: scripts/check-plugin-imports.sh <component.wasm>..." >&2; exit 2; }

if ! command -v wasm-tools > /dev/null; then
    echo "!! wasm-tools is not installed — falling back to a byte scan, which cannot tell an" >&2
    echo "   import from a string literal. Install it for the real check:" >&2
    echo "   cargo install wasm-tools --locked" >&2
    status=0
    for component in "$@"; do
        if grep -a -q 'wasi:' "$component"; then
            echo "!! $component contains the text 'wasi:' (byte scan; may be a false positive)" >&2
            status=1
        fi
    done
    exit "$status"
fi

status=0
for component in "$@"; do
    [[ -f "$component" ]] || { echo "!! $component does not exist" >&2; status=1; continue; }

    # `component wit` renders the component's world. Its `import` lines name interfaces; a
    # component that is not a component at all fails here rather than passing unchecked.
    if ! world="$(wasm-tools component wit "$component" 2>&1)"; then
        echo "!! $component is not a readable WebAssembly component:" >&2
        echo "$world" | sed 's/^/   /' >&2
        status=1
        continue
    fi

    # `import foo:bar/baz@1.2.3;` -> `foo:bar/baz@1.2.3`
    imports="$(printf '%s\n' "$world" \
        | sed -n 's/^[[:space:]]*import[[:space:]]\{1,\}\([^;]*\);.*$/\1/p' \
        | sed 's/[[:space:]]*$//')"

    offenders=""
    while IFS= read -r interface; do
        [[ -n "$interface" ]] || continue
        [[ "$interface" == "$ALLOWED_PREFIX"* ]] && continue
        offenders+="   $interface"$'\n'
    done <<< "$imports"

    if [[ -n "$offenders" ]]; then
        echo "!! $component imports outside $ALLOWED_PREFIX:" >&2
        printf '%s' "$offenders" >&2
        status=1
    fi
done

exit "$status"
