#!/usr/bin/env bash
# Decides whether the {box} archives on the latest release are out of date.
#
# The latest release is the one GitHub marks as Latest (never a draft or a
# pre-release). Its box-source.txt records the commit its archives were built
# from. When that differs from the commit the {box} source points at now, the
# archives are stale.
#
# Only a release that already has a box-source.txt is refreshed, unless FORCE
# is "true". Writes tag, repo, ref, sha and stale to $GITHUB_OUTPUT, or to the
# terminal when that is not set. Needs the gh CLI and GITHUB_REPOSITORY.
#
# Usage: scripts/plan-box-refresh.sh

set -euo pipefail

out="${GITHUB_OUTPUT:-/dev/stdout}"
emit() {
    printf '%s=%s\n' "$1" "$2" >> "$out"
}

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}"

tag="$(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .tag_name 2>/dev/null || true)"
if [ -z "$tag" ]; then
    echo "There is no release yet, so there is nothing to refresh."
    emit stale false
    exit 0
fi

resolved="$("$(dirname "$0")/resolve-box.sh")"
repo="$(printf '%s\n' "$resolved" | sed -n 's/^repo=//p')"
ref="$(printf '%s\n' "$resolved" | sed -n 's/^ref=//p')"
sha="$(printf '%s\n' "$resolved" | sed -n 's/^sha=//p')"

built="$(gh release download "$tag" --repo "$GITHUB_REPOSITORY" --pattern box-source.txt --output - 2>/dev/null \
    | sed -n 's/^sha=//p' || true)"

# A release that shipped without {box} stays that way, so turning this on
# does not retrofit v0.1.1 or any other old release. FORCE=true does it on
# purpose, for a release whose first {box} build failed.
if [ -z "$built" ] && [ "${FORCE:-}" != "true" ]; then
    echo "$tag was released without {box}, so it is left alone. Run this by hand with force to add it."
    emit stale false
    exit 0
fi

if [ "$built" = "$sha" ] && [ "${FORCE:-}" != "true" ]; then
    stale=false
    echo "$tag already has {box} from $repo@$ref ($sha), nothing to do."
else
    stale=true
    echo "$tag has {box} from '${built:-nothing}', and $repo@$ref is at $sha. Rebuilding."
fi

emit tag "$tag"
emit repo "$repo"
emit ref "$ref"
emit sha "$sha"
emit stale "$stale"
