#!/usr/bin/env bash
#
# Holds the headless line of the capture agent against the *resolved* Linux dependency tree,
# not against the manifest that starts it.
#
# `crates/rd-capture/src/platform_gate/` reads `crates/rd-capture/Cargo.toml` and demands a
# written reason for every dependency an ordinary Linux build reads. That check sees exactly
# one level: what this crate declares. It cannot see a window toolkit that arrives behind an
# innocent name — a new dependency of `rd-core`, a feature that flips on somewhere in the
# workspace, a `[patch]` that redirects a crate onto a fork with GTK underneath it. Those all
# end the same way: a package that draws windows appears in the tree a Linux build compiles,
# and the server that has no display libraries stops building.
#
# So this asks the resolver instead. `cargo tree` computes the tree the way the build does —
# under the same features, the same `[patch]` tables and the same `.cargo/config.toml` — and
# every package in it is held against the list below. Where the offender came from is printed
# with it, because "gtk-sys reaches the Linux build" is only actionable once you know who
# pulled it.
#
# Usage:
#   scripts/check-capture-linux-tree.sh [<cargo-tree-arg> ...]
#
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

PACKAGE="rd-capture"
TARGET="x86_64-unknown-linux-gnu"

# Package families that carry a window, a widget set or a rendering stack, and therefore may
# not appear anywhere in the Linux tree of the capture agent. An entry matches a package of
# the same name and the versioned or `-sys` members of its family: `gtk` covers `gtk-sys`,
# `gtk3` and `gtk4-sys`.
#
# Deliberately *not* on this list: `x11rb`, `wl-clipboard-rs` and the `wayland-*` crates.
# `arboard` pulls them and that is where the line runs — a protocol client is not a toolkit,
# it needs no GTK, and with no display server reachable it returns an error instead of taking
# the agent down. `crates/rd-capture/Cargo.toml` says the same beside the gate.
FORBIDDEN=(
    atk              # GTK accessibility toolkit
    cairo-rs         # GTK's rendering library
    druid            # GUI framework
    egui             # GUI framework
    femtovg          # GPU canvas renderer
    fltk             # GUI framework
    gdk              # GTK's windowing layer, and gdk-pixbuf, gdkx11, gdk4
    gio              # GLib I/O, arrives with GTK
    glib             # GTK's base library
    glium            # OpenGL wrapper
    glutin           # OpenGL context creation, i.e. a window
    gobject          # GLib object system, arrives with GTK
    gpui             # GUI framework
    gtk              # the widget set the headless agent must not link
    iced             # GUI framework
    image            # image decoding; reaches the agent only through arboard's image-data
    javascriptcore   # WebKitGTK's engine bindings
    muda             # menu bar, half of the tray integration
    open             # launches a desktop application; nothing headless opens one
    pango            # GTK's text layout
    skia-safe        # 2D renderer
    slint            # GUI framework
    softbuffer       # framebuffer for a window
    soup3            # WebKitGTK's HTTP library
    tao              # the window toolkit itself
    tray-icon        # the tray integration
    webkit2gtk       # the WebView, and webkit2gtk-sys
    wgpu             # GPU rendering
    winit            # window creation
    wry              # the WebView wrapper
    x11-dl           # dlopened Xlib; the toolkits' way in, unlike the x11rb protocol client
)

matches_forbidden() {
    local name="$1" entry
    for entry in "${FORBIDDEN[@]}"; do
        # The name itself, or the family: `<entry>-sys`, `<entry>3`, `<entry>4-sys`.
        if [[ "$name" == "$entry" || "$name" =~ ^"$entry"(-|[0-9]) ]]; then
            return 0
        fi
    done
    return 1
}

echo "==> cargo tree -p ${PACKAGE} --target ${TARGET}"

# --no-dedupe so a package that appears under several parents is judged wherever it stands;
# --prefix depth so every line is `<depth><name> v<version>` — one package per line, with how
# far from rd-capture it stands. The edge kinds are every kind a Linux build compiles: what
# ships, what builds it and what tests it.
tree_output="$(cargo tree \
    --package "${PACKAGE}" \
    --target "${TARGET}" \
    --edges normal,build,dev \
    --prefix depth \
    --no-dedupe \
    "$@")"

# One line per distinct package, as `<depth> <name>`, carrying the *shallowest* place it
# stands — that is the edge somebody added, rather than whichever copy sorts first.
readarray -t entries < <(printf '%s\n' "${tree_output}" | awk '
    match($0, /^[0-9]+/) {
        depth = substr($0, 1, RLENGTH) + 0
        name = substr($1, RLENGTH + 1)
        if (name != "" && (!(name in shallowest) || depth < shallowest[name])) {
            shallowest[name] = depth
        }
    }
    END { for (name in shallowest) printf "%03d %s\n", shallowest[name], name }')

found=()
for entry in "${entries[@]}"; do
    if matches_forbidden "${entry#* }"; then
        found+=("$entry")
    fi
done

if [[ ${#found[@]} -eq 0 ]]; then
    echo "    ${#entries[@]} packages in the Linux tree, none of them a window stack"
    exit 0
fi

# Shallowest first.
readarray -t offenders < <(printf '%s\n' "${found[@]}" | sort | cut -d' ' -f2)
{
    echo
    echo "!! the Linux build of ${PACKAGE} reaches a window stack:"
    echo "   ${offenders[*]}"
    echo
    # The chain for the shallowest one. That is the edge somebody added; the rest of the list
    # is what came with it, and `cargo tree --invert <name>` prints the path for any of them.
    echo "   ${offenders[0]}, pulled in by:"
    cargo tree \
        --package "${PACKAGE}" \
        --target "${TARGET}" \
        --edges normal,build,dev \
        --invert "${offenders[0]}" \
        "$@" | sed 's/^/     /'
    echo
    echo "   The Linux capture agent is headless: it links no window toolkit, no GTK and no"
    echo "   WebKitGTK. Either the dependency that pulls this belongs behind the"
    echo "   Windows/macOS gate in crates/rd-capture/Cargo.toml, or the feature that turned it"
    echo "   on has to go back off. If the line itself is what moved, it moves in"
    echo "   crates/rd-capture/src/platform_gate/mod.rs and in the list in this script, with a"
    echo "   reason written beside it."
} >&2
exit 1
