#!/bin/sh
# Builds atelier.flatpak from the `cargo build --release` binary.
# Prerequisites (once): flatpak, flatpak-builder, and the Flathub remote:
#   flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
set -e
cd "$(dirname "$0")"
(cd ../.. && cargo build --release)
flatpak-builder --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
    --repo=repo build io.github.Shhu.Atelier.yml
flatpak build-bundle repo atelier.flatpak io.github.Shhu.Atelier \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
echo "Package ready: $(pwd)/atelier.flatpak"
