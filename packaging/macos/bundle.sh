#!/bin/sh
# On a Mac: builds Atelier.app around the given binary and packs it into a
# .dmg (with a link to Applications). The app is signed ad hoc only: there
# is no Apple Developer certificate, so macOS asks for confirmation (right
# click > Open) the first time.
# Usage: packaging/macos/bundle.sh path/to/atelier VERSION out.dmg
set -e
binary=$1
version=$2
dmg=$3
cd "$(dirname "$0")/../.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/Atelier/Atelier.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/atelier"
sed "s/VERSION/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# Icon: every size of an .iconset, from the 1024 px PNG.
set_dir="$work/AppIcon.iconset"
mkdir "$set_dir"
for size in 16 32 128 256 512; do
    sips -z $size $size assets/icon-1024.png --out "$set_dir/icon_${size}x${size}.png" > /dev/null
    double=$((size * 2))
    sips -z $double $double assets/icon-1024.png --out "$set_dir/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/AppIcon.icns"

codesign --force --deep --sign - "$app"
ln -s /Applications "$work/Atelier/Applications"
hdiutil create -volname Atelier -srcfolder "$work/Atelier" -ov -format UDZO "$dmg"
