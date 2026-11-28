#!/usr/bin/env bash
# Regenerates crates/app/assets/app-icon/{icon.icns,icon-512.png,icon.ico} from icon.svg (macOS
# only; the Windows .ico needs ImageMagick: brew install imagemagick).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/crates/app/assets/app-icon"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
set_dir="$work/icon.iconset"; mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
  swift "$root/script/render-icon.swift" "$dir/icon.svg" "$set_dir/icon_${size}x${size}.png" "$size"
  swift "$root/script/render-icon.swift" "$dir/icon.svg" "$set_dir/icon_${size}x${size}@2x.png" "$((size * 2))"
done
iconutil -c icns "$set_dir" -o "$dir/icon.icns"
cp "$set_dir/icon_256x256@2x.png" "$dir/icon-512.png"   # Linux .desktop / window icon
# Windows (the exe's icon, the taskbar, the installer): the rounded square without the margin of
# Apple's icon grid, a little padding around it, at the sizes Windows asks for.
swift "$root/script/render-icon.swift" "$dir/icon.svg" "$work/big.png" 1024
magick "$work/big.png" -trim +repage -background none -gravity center -extent 864x864 "$work/pad.png"
sizes=()
for size in 16 20 24 32 40 48 64 128 256; do
  magick "$work/pad.png" -filter Lanczos -resize "${size}x${size}" "PNG32:$work/win-$size.png"
  sizes+=("$work/win-$size.png")
done
magick "${sizes[@]}" "$dir/icon.ico"
echo "$dir/icon.icns"
echo "$dir/icon.ico"
