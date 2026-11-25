#!/usr/bin/env bash
# Renders every Expand icon file from the SVG sources in design/icons:
#
#   icon.svg               the app icon on Apple's 1024 grid: the .icns and the bundle PNGs (via
#                          `tauri icon`), the .ico's 48/64/256 (cropped to Windows' tighter
#                          margins), the large Square*Logo, docs/images/icon.png, app/public/icon-mac.png
#   icon-small.svg         the same icon redrawn for 16-40 px: the .ico's 16/20/24/32/40 (the PC's
#                          tray loads one of these at the system's small-icon size), the small
#                          Square*Logo and StoreLogo, and app/public/icon.png (the PC's title bars)
#   tray-template.svg      the Mac's menu-bar template at 2x: icons/tray-template@2x.png (36 px)
#   tray-template-1x.svg   the same for 1x screens: icons/tray-template.png (18 px)
#
# Needs ImageMagick 7 with librsvg (`magick -list format | grep RSVG`; on the Mac
# `brew install imagemagick librsvg`) and app/node_modules (`npm ci` in app/) for `tauri icon`.
# Run from anywhere: scripts/render-icons.sh
set -euo pipefail
# `pwd -W`: Windows paths in Git Bash, which magick.exe needs after its "RSVG:" prefix.
ROOT="$(cd "$(dirname "$0")/.." && (pwd -W 2>/dev/null || pwd))"
SRC="$ROOT/design/icons"
OUT="$ROOT/app/src-tauri/icons"
TMP="$(mktemp -d)"
[ -n "$TMP" ] && [ -d "$TMP" ] || { echo "mktemp failed" >&2; exit 1; }
TMP="$(cd "$TMP" && (pwd -W 2>/dev/null || pwd))"
[ -n "$TMP" ] || exit 1
trap 'rm -rf "$TMP"' EXIT

magick -list format | grep RSVG > /dev/null || { echo "ImageMagick has no librsvg (RSVG) delegate" >&2; exit 1; }
[ -x "$ROOT/app/node_modules/.bin/tauri" ] || [ -f "$ROOT/app/node_modules/.bin/tauri.cmd" ] \
  || { echo "run npm ci in app/ first (for tauri icon)" >&2; exit 1; }

# render SVG PX OUT [VIEWBOX]: an 8-bit RGBA PNG, PX square. VIEWBOX crops the source ("x y w h").
render() {
  local svg="$1" px="$2" out="$3" box="${4:-}" src="$1"
  if [ -n "$box" ]; then
    src="$TMP/crop-$(basename "$svg")"
    local w="${box##* }"
    sed -E "s/viewBox=\"[^\"]*\" width=\"[0-9]+\" height=\"[0-9]+\"/viewBox=\"$box\" width=\"$w\" height=\"$w\"/" "$svg" > "$src"
  fi
  local size
  size="$(sed -nE 's/.*<svg[^>]* width="([0-9]+)".*/\1/p' "$src" | head -1)"
  magick -background none -density "$(awk "BEGIN { print 96 * $px / $size }")" "RSVG:$src" \
    -resize "${px}x${px}!" -strip "PNG32:$out"
}
# Windows draws icons nearly edge to edge; Apple's grid leaves 100 of 1024 round the tile. This
# crop keeps the tile's shadow and is square: the tile is 89% of it.
WIN="48 56 928 928"

# The bundle set: icon.icns and the PNGs, from a 1024 render.
render "$SRC/icon.svg" 1024 "$TMP/icon-1024.png"
(cd "$ROOT/app" && npx tauri icon "$TMP/icon-1024.png" -o "$TMP/tauri" > "$TMP/tauri.log" 2>&1) \
  || { cat "$TMP/tauri.log" >&2; exit 1; }
for f in icon.icns icon.png 32x32.png 64x64.png 128x128.png 128x128@2x.png; do
  cp "$TMP/tauri/$f" "$OUT/$f"
done

# icon.ico: the small drawing up to 40 px, the full one, cropped, above.
ico=()
for px in 16 20 24 32 40; do render "$SRC/icon-small.svg" "$px" "$TMP/ico-$px.png"; ico+=("$TMP/ico-$px.png"); done
for px in 48 64 256; do render "$SRC/icon.svg" "$px" "$TMP/ico-$px.png" "$WIN"; ico+=("$TMP/ico-$px.png"); done
magick "${ico[@]}" "$OUT/icon.ico"

# Windows' Square*Logo and StoreLogo.
for px in 30 44; do render "$SRC/icon-small.svg" "$px" "$OUT/Square${px}x${px}Logo.png"; done
render "$SRC/icon-small.svg" 50 "$OUT/StoreLogo.png"
for px in 71 89 107 142 150 284 310; do render "$SRC/icon.svg" "$px" "$OUT/Square${px}x${px}Logo.png" "$WIN"; done

# The Mac's menu-bar template, 1x and 2x.
render "$SRC/tray-template-1x.svg" 18 "$OUT/tray-template.png"
render "$SRC/tray-template.svg" 36 "$OUT/tray-template@2x.png"

# The README's icon and the app's in-window icons.
render "$SRC/icon.svg" 512 "$ROOT/docs/images/icon.png"
render "$SRC/icon-small.svg" 64 "$ROOT/app/public/icon.png"
render "$SRC/icon.svg" 128 "$ROOT/app/public/icon-mac.png"

echo "rendered the icons into $OUT, docs/images and app/public"
