#!/bin/sh
# Refuses (exit 1) any file, or file in a folder, given that names a path of
# this computer: the Windows and WSL user folders, plus the words listed one
# per line in ~/.config/atelier-release/forbidden (kept out of the
# repository). Text is looked for as bytes and as UTF-16 (Windows strings).
# Usage: packaging/check-private.sh atelier.exe packaging/flatpak/build/files
set -e
[ $# -gt 0 ] || { echo "Usage: $0 file-or-folder..."; exit 1; }
. "$(dirname "$0")/windows/paths.sh"
user=${WIN_HOME##*\\}
patterns=$(mktemp)
trap 'rm -f "$patterns"' EXIT
{
    printf '%s\n' "Users\\$user\\" "Users/$user/" "$HOME/" \
        "$(printf '%s' "$HOME" | tr / '\\')\\"
    list="$HOME/.config/atelier-release/forbidden"
    if [ -f "$list" ]; then grep -v '^[[:space:]]*$' "$list"; fi
} > "$patterns"
for target in "$@"; do
    [ -e "$target" ] || { echo "Missing: $target" >&2; exit 1; }
done
# Each file found with one is named, with its first matches.
if ! find "$@" -type f -exec sh -c '
    patterns=$1; shift; clean=0
    for file; do
        hits=$( { strings -a -n 6 "$file"; strings -a -el -n 6 "$file"; } |
            grep -i -F -f "$patterns" | head -3 | cut -c1-200) || true
        if [ -n "$hits" ]; then
            printf "Private path in %s:\n%s\n" "$file" "$hits" >&2
            clean=1
        fi
    done
    exit $clean' sh "$patterns" {} +; then
    echo "Not published: rebuild without these paths" >&2
    exit 1
fi
echo "No private path in: $*"
