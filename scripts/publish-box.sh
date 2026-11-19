#!/usr/bin/env bash
# Replaces the {box} archives on a release with the ones in a folder.
#
# Only files named box_* and box-source.txt are ever touched, so the release's
# own binaries and installers stay as they are. The new archives go up first,
# then the old ones that are not in the new set are removed, and
# box-source.txt goes up last. A run that stops half way therefore never looks
# up to date, and the next run starts over.
#
# Usage: scripts/publish-box.sh <tag> <folder-with-the-new-files>
# Needs the gh CLI and GITHUB_REPOSITORY.

set -euo pipefail

fail() {
    echo "error: $*" >&2
    exit 1
}

[ $# -eq 2 ] || fail "usage: publish-box.sh <tag> <folder>"
tag="$1"
dir="$2"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}"
[ -d "$dir" ] || fail "'$dir' is not a folder"

shopt -s nullglob
archives=("$dir"/box_*)
[ ${#archives[@]} -gt 0 ] || fail "no box_* archives in '$dir', refusing to publish nothing"
[ -f "$dir/box-source.txt" ] || fail "no box-source.txt in '$dir'"

echo "Uploading ${#archives[@]} archives to $tag"
gh release upload "$tag" "${archives[@]}" --repo "$GITHUB_REPOSITORY" --clobber

keep="$(for file in "$dir"/box_* "$dir"/box-source.txt; do basename "$file"; done)"
gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name' | while read -r name; do
    case "$name" in
        box_* | box-source.txt) ;;
        *) continue ;;
    esac
    if ! printf '%s\n' "$keep" | grep -qxF "$name"; then
        echo "Removing $name"
        gh release delete-asset "$tag" "$name" --repo "$GITHUB_REPOSITORY" --yes
    fi
done

gh release upload "$tag" "$dir/box-source.txt" --repo "$GITHUB_REPOSITORY" --clobber
echo "$tag now has the {box} archives from $(sed -n 's/^sha=//p' "$dir/box-source.txt")"
