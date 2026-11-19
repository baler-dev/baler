#!/usr/bin/env bash
# Builds the {box} R package into an installable binary package for the R and
# platform this runs on, checks that the binary installs and loads from an
# empty library, and writes it to OUT with a .sha256 file next to it.
#
# A binary package is tied to one R minor version (4.5, 4.6, ...) and one
# platform, so both go into the file name:
#   box_<version>_R<minor>_<os>-<arch>.<tar.gz|tgz|zip>
# To install a downloaded file, save it as box_<version>.<ext> first. R reads
# the package name and version from the file name and fails on a longer one.
#
# Usage: scripts/build-box.sh <box-source-dir> <os> <arch> <out-dir>
#   e.g. scripts/build-box.sh box-src linux x86_64 dist

set -euo pipefail

fail() {
    echo "error: $*" >&2
    exit 1
}

[ $# -eq 4 ] || fail "usage: build-box.sh <box-source-dir> <os> <arch> <out-dir>"
src="$1"
os="$2"
arch="$3"
out="$4"

[ -f "$src/DESCRIPTION" ] || fail "no DESCRIPTION in '$src', is that the {box} source?"
command -v Rscript >/dev/null 2>&1 || fail "Rscript is not on the PATH"

src="$(cd "$src" && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/lib" "$work/smoke"

version="$(Rscript -e 'cat(read.dcf(commandArgs(TRUE)[1], "Version"))' "$src/DESCRIPTION")"
rminor="$(Rscript -e 'cat(R.version$major, ".", sub("[.].*", "", R.version$minor), sep = "")')"
[ -n "$version" ] || fail "could not read Version from $src/DESCRIPTION"

# The build compiles inside the source folder, so it works on a copy and the
# checkout stays clean.
cp -R "$src" "$work/pkg"

echo "Building box $version for R $rminor on $os-$arch"
(cd "$work" && R CMD INSTALL --build --library="$work/lib" pkg)

shopt -s nullglob
built=("$work"/box_"$version"*)
[ ${#built[@]} -eq 1 ] || fail "expected one built package for box $version, found ${#built[@]}"
case "${built[0]}" in
    *.tar.gz) ext="tar.gz" ;;
    *.tgz) ext="tgz" ;;
    *.zip) ext="zip" ;;
    *) fail "unexpected package type: ${built[0]}" ;;
esac

final="box_${version}_R${rminor}_${os}-${arch}.${ext}"
cp "${built[0]}" "$out/$final"

# The check is a script of its own, one numbered step at a time with the
# output flushed, so when R crashes (a crash has no error message) the last
# line printed says which step it was.
cat > "$work/check.R" <<'RSCRIPT'
say = function(...) {
    cat(..., "\n", sep = "")
    flush(stdout())
}
lib = commandArgs(TRUE)[1]
.libPaths(c(lib, .libPaths()))
say("R ", R.version.string, " on ", R.version$platform)
say("step 1: load the box namespace")
stopifnot(requireNamespace("box", quietly = TRUE))
here = normalizePath(find.package("box"), winslash = "/")
stopifnot(startsWith(here, normalizePath(lib, winslash = "/")))
say("step 2: box::use() on a package")
box::use(stats[sd])
stopifnot(sd(1:3) == 1)
say("box ", as.character(packageVersion("box")), " loads and box::use() works")
RSCRIPT

# What users get is what gets tested: install the file from the release into
# an empty library, and run the check against it. R takes the package name and
# version from the name of the file it installs, and R 4.5 on Windows fails on
# anything but box_<version>.<ext>, so the installer has to save the download
# under that name. The check does the same.
echo "Checking $final installs and loads"
installable="$work/box_${version}.${ext}"
cp "$out/$final" "$installable"
R CMD INSTALL --library="$work/smoke" "$installable"
if ! Rscript --vanilla "$work/check.R" "$work/smoke"; then
    # Tell a bad binary from a {box} that cannot run here at all, by running
    # the same check on a copy compiled from source on this machine.
    echo
    echo "The check failed on the binary. Running it on a copy built from source."
    mkdir "$work/source-lib"
    cp -R "$src" "$work/pkg-source"
    R CMD INSTALL --library="$work/source-lib" "$work/pkg-source"
    if Rscript --vanilla "$work/check.R" "$work/source-lib"; then
        fail "the binary fails the check but a build from source passes, so the binary is at fault"
    else
        fail "the check fails on a build from source too, so {box} itself does not run here"
    fi
fi

(
    cd "$out"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$final" > "$final.sha256"
    else
        shasum -a 256 "$final" > "$final.sha256"
    fi
)

echo "Wrote $out/$final"
