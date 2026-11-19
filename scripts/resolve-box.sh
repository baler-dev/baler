#!/usr/bin/env bash
# Works out which {box} source to build: the repository, the ref and the exact
# commit the ref points at. Prints repo=, ref= and sha= lines, which is also
# the format of box-source.txt on a release.
#
# Usage: scripts/resolve-box.sh [repo] [ref]
#
# A missing or empty argument falls back to the BOX_REPO or BOX_REF environment
# variable (CI sets them from the repository variables of the same names), and
# then to joshuamarie/box and feature/baler-support. The ref can be a branch, a
# tag or a full commit hash. To follow another repository, such as klmr/box,
# set the two variables. Nothing here needs editing.

set -euo pipefail

fail() {
    echo "error: $*" >&2
    exit 1
}

repo="${1:-${BOX_REPO:-joshuamarie/box}}"
ref="${2:-${BOX_REF:-feature/baler-support}}"

[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail "'$repo' is not an owner/name repository"
[[ "$ref" =~ ^[A-Za-z0-9_./-]+$ ]] || fail "'$ref' is not a branch, tag or commit name"

if [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
    sha="$ref"
else
    lines="$(git ls-remote "https://github.com/$repo.git" "refs/heads/$ref" "refs/tags/$ref" "refs/tags/$ref^{}")" \
        || fail "could not reach https://github.com/$repo.git"
    # An annotated tag lists the tag object and, with ^{}, the commit under it.
    sha="$(printf '%s\n' "$lines" | awk '$2 ~ /\^\{\}$/ { print $1; exit }')"
    [ -n "$sha" ] || sha="$(printf '%s\n' "$lines" | awk 'NF { print $1; exit }')"
    [ -n "$sha" ] || fail "'$ref' is not a branch or tag of $repo"
fi

printf 'repo=%s\nref=%s\nsha=%s\n' "$repo" "$ref" "$sha"
