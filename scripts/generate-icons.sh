#!/usr/bin/env sh
# Regenerates the browser-extension PNGs and the Windows .ico from the
# single source of truth, web/public/favicon.svg.
# Rasterizes with sharp (librsvg) via npx — ImageMagick's builtin SVG
# renderer drops the stroke elements. ImageMagick only assembles the .ico.
set -eu

cd "$(dirname "$0")/.."
SVG=web/public/favicon.svg
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

render() {
    npx --yes sharp-cli -i "$SVG" -o "$2" --density 1152 resize "$1" "$1" >/dev/null
}

for size in 16 32 48 128; do
    render "$size" "extension/icons/icon${size}.png"
done

# Progressive web app icons (RD-090-08). A manifest needs raster icons: the install
# prompt and the home-screen entry are rendered by the platform, not the browser.
mkdir -p web/public/icons
for size in 192 512; do
    render "$size" "web/public/icons/icon-${size}.png"
done

for size in 16 24 32 48 64 256; do
    render "$size" "$TMP/ico-${size}.png"
done
magick "$TMP"/ico-16.png "$TMP"/ico-24.png "$TMP"/ico-32.png \
    "$TMP"/ico-48.png "$TMP"/ico-64.png "$TMP"/ico-256.png \
    resources/rdownloader.ico

echo "Icons regenerated from $SVG"
