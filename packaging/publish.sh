#!/bin/sh
# Publishes a version of Atelier from this computer (no CI): builds the
# Windows .exe and the Flatpak, tags the sources, creates the GitHub release
# with them, where the website's download buttons point, and updates the
# shu-app.com Flatpak repository. The website itself (docs/) is served by GitHub Pages from
# main, so pushing main publishes it.
# Usage: packaging/publish.sh notes.md   (version taken from Cargo.toml)
set -e
cd "$(dirname "$0")/.."
notes=$1
[ -f "$notes" ] || { echo "Usage: $0 notes.md"; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
tag="v$version"
if gh release view "$tag" --repo Shhu/atelier > /dev/null 2>&1; then
    echo "$tag is already published: bump the version in Cargo.toml"
    exit 1
fi
[ -z "$(git status --porcelain)" ] || { echo "Commit your changes first"; exit 1; }

packaging/windows/build.sh
packaging/flatpak/build.sh
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
. packaging/windows/paths.sh
cp "$WIN_TARGET_WSL/release/atelier.exe" "$out/"
cp packaging/flatpak/atelier.flatpak "$out/"

git tag "$tag"
git push origin main "$tag"
gh release create "$tag" "$out/atelier.exe" "$out/atelier.flatpak" \
    --repo Shhu/atelier --title "Atelier $version" --notes-file "$notes"
echo "Published: https://github.com/Shhu/atelier/releases/tag/$tag"
# The shu-app.com Flatpak repository (shared with the author's other apps)
# takes the new bundle; the release stands even if this fails.
shared="$HOME/dev/shu-app/flatpak/publish.sh"
if [ -x "$shared" ]; then
    "$shared" || echo "Flatpak repository not updated: run $shared again" >&2
fi
echo "macOS version too? packaging/macos/publish.sh $tag"
