#!/bin/sh
# Adds the macOS version to a published release, when the maintainer decides
# to for that release: runs the macOS build on GitHub Actions (the only part not
# built on this computer, for lack of a Mac), waits for it, and attaches the
# .dmg to the public release. The website's macOS buttons point to the
# newest release that has one.
# Usage: packaging/macos/publish.sh v0.3.0 [--skip-tests]
set -e
tag=$1
skip_tests=false
[ "$2" = "--skip-tests" ] && skip_tests=true
[ -n "$tag" ] || { echo "Usage: $0 vX.Y.Z"; exit 1; }
gh release view "$tag" --repo Shhu/atelier > /dev/null 2>&1 ||
    { echo "$tag is not published yet: run packaging/publish.sh first"; exit 1; }

before=$(gh run list --repo Shhu/atelier --workflow macos.yml --limit 1 --json databaseId --jq '.[0].databaseId // 0')
gh workflow run macos.yml --repo Shhu/atelier -f tag="$tag" -f skip_tests="$skip_tests"
run=$before
while [ "$run" = "$before" ]; do
    sleep 3
    run=$(gh run list --repo Shhu/atelier --workflow macos.yml --limit 1 --json databaseId --jq '.[0].databaseId // 0')
done
gh run watch "$run" --repo Shhu/atelier --exit-status

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
gh run download "$run" --repo Shhu/atelier --name atelier-macos --dir "$out"
gh release upload "$tag" "$out/atelier-macos.dmg" --repo Shhu/atelier --clobber
echo "macOS version added to $tag"
