# Changelog

# Development version

## What's new

-  `baler config` lets you reads and changes the settings of your machine through a configuration, and kept in `~/.baler/config.toml`. More details: 

    -   `baler config` lists them, `baler config <key>` prints one, `baler config <key> <value>` sets one and `baler config --unset <key>` removes one. Add `--show-origin` to the list to see where each value comes from.
    -   `install-deps` turns `baler install --install-deps` on for every run. `--no-install-deps` turns it off again for one run, and `BALER_INSTALL_DEPS` sets it for a shell.
    -   `lib` sets where modules are installed, and writes `BALER_LIB` into `~/.Renviron` so that `{box}` finds them.

# v0.2.1

## What's new

-  `baler list` can now be narrowed down.

    -   `baler list <pattern>` shows only the modules whose name matches, with `*` for any run of characters and `?` for any one, ignoring case.

    -   `baler list -k <keyword>` (or `--keyword`, or `--tag`) shows only the modules that list that keyword in their `baler.toml`. `-k stats,plots` or a repeated flag requires all of them.

    -   `baler list --names-only` prints just the names, one for each line, for scripts. It prints nothing when no module matches.

    -   They can be combined. An empty pattern or keyword is an error. Plain `baler list` still prints every module in the same table.

# v0.2.0

## What's new

-  `baler list` shows the modules installed in the install directory (name, version, license, R requirement), and `baler show <name>` shows everything one module declared, where its files are, and which license file and README were kept. 

    -   These new commands will read the `.dist-info` folder that `baler install` writes, so a directory not installed by `{baler}` is not listed.

-  `baler init --license <id>` writes the license into `baler.toml` instead of the `Unknown` placeholder. The value has to be a valid SPDX identifier or expression, and an invalid one fails before anything is written.

-  `baler bundle` now ships the project's license files (`LICENSE`, `LICENCE` and `COPYING`, with or without `.md` or `.txt`) and its README, and `baler install` keeps them in the module's `.dist-info` folder.

    -   The README is the file `readme` points at, or else a `README` at the project root.

    -   It also warns, but never fails, when `license` is empty or `Unknown`, when it is not valid SPDX, when there is no license file at the project root, or when `readme` names a file that is missing or outside the project.

### {box} package installation

-  Releases now ship the patched `{box}` already built (if its build fails, the release goes out without it), for Linux (x86_64 and aarch64), macOS (Apple silicon and Intel) and Windows, for the current and the previous R version, so installing it no longer needs a compiler. The files on the latest release are rebuilt when the fork changes. The installation guide has the steps.

-  `baler box install` installs the patched `{box}` that `{baler}` modules are loaded with. It picks the pre-built file on the latest release that matches your R version and platform, checks it against the published SHA-256, and installs it into your R library without a compiler and without running `R CMD INSTALL`.

    -   An upstream `{box}` is never replaced unless `--force` is given, and a `{box}` that is already the current baler build is left alone.

    -   `--tag` takes `{box}` from a given release instead of the latest one.

    -   If the release has no build for your R version, the error lists the R versions it has and prints the command to build the fork from source.

-  `baler box status` shows which `{box}` is installed, whether it has `{baler}` support, and which commit it was built from. It reads files only. `--check` makes it exit with an error unless a `{box}` with `{baler}` support is installed, for CI.

-  `install.sh` and `install.ps1` print a hint about `baler box install` when they finish. Setting `BALER_WITH_BOX=1` runs it for you. A failure there never fails the `{baler}` install.

## Fixes

-  `baler remove` now deletes every `.dist-info` folder of the module. Before, it left that folder behind, and an upgrade left the old version's folder, so stale metadata piled up in the install directory. Removing a name also cleans up leftovers from earlier versions when the module itself is already gone.

-  `baler remove` refuses names that are not one plain name, such as `.`, `..` or anything with a `/`. Before, `baler remove . --force` pointed at the install directory itself and deleted every module.

-  Pre-release versions (`0.3.0-dev`) and hyphenated module names (`foo-bar`) are now read correctly from `.dist-info` folder names.

## Compatibility

-  A bundle made by this version holds the license files and README at its top level. `{baler}` 0.1.x does not know them and unpacks them loose into the install directory, so install such bundles with this version or later.

# v0.1.1

## Security

-  The installers (`install.sh` and `install.ps1`) now verify the SHA-256 hash of the downloaded archive against the `.sha256` file published with each release. A mismatch, or a missing or malformed checksum file, stops the install before anything is unpacked, and an existing `baler` is left as it was. The Linux and macOS installer also downloads and checks everything in a temporary directory first, instead of unpacking into the current directory.

## Fixes

-  `baler install --git` now sends `GITHUB_TOKEN` (or `GH_TOKEN`) when one is set, so private repositories work and CI is not held to GitHub's rate limit for anonymous requests. The token goes only to `api.github.com` over HTTPS, never to a host given with `--url`. When GitHub answers 404, 403 or 429 and no token is set, the error now says so and names the variable.

## Build

-  The arm64 macOS release build now runs on `macos-15`. GitHub retired the `macos-14` runner image on November 2, 2026, so that job was cancelled with "configuration 'macos-14-arm64-us-default' is not supported".

## Documentation

-  Brought the docs back in line with the code: the architecture page (crate layout, bundle contents, the `BALER_*` environment variables), the install commands in the guide (`--path`, `--git`), the installer page (checksum verification and version pinning), the `[dependencies]` and `[dependencies.baler]` table names, FORTRAN's status on the native-code page, and links that pointed at `.md` pages that do not exist. The `baler lock` description on the home page now names `--with-rver`.

# v0.1.0

-  This is the migrated version of `{carrier}`, now named `{baler}`. There's no formal name for this packages so name the manufactured packages as `box-bale` modules, not "R packages".

-  Built the very first foundation for managing `{box}` modules as installable, distributable packages.

-  For more details for CLI commands, visit its [official documentation](https://baler-dev.github.io/baler/#commands).

## Migrating from `{carrier}`

`{baler}` still took the idea from `{carrier}`: a `cargo`-like package manager for `{box}` modules, written in Rust. The commands (`init`, `compile`, `bundle`, `install`, `lock`, `remove`), the module layout (`__init__.r` as the entry point, `src` resolution) and how a module is used (`box::use()`) are unchanged. What changed, however, is mostly names, plus there are few behaviours added on the command line interface (in addition for CLI to became stricter than before).

### What was renamed

During the migration, `{carrier}` is transferred its development, including the history, into `{baler}`. This involves the name of the branch, where the patches in `{box}` for `{baler}` support is still pending to be upstream, you are going to install `{box}` package by running

``` r
pak::pak("joshuamarie/box@feature/baler-support")
```

Not

``` r
pak::pak("joshuamarie/box@feature/carrier-module-support")
```

in R.

Here's a side-by-side comparison of what renamed from `{carrier}` to `{baler}` in table format:

| `{carrier}`               | `{baler}`               |
|---------------------------|-------------------------|
| `carrier` command         | `baler`                 |
| `carrier.toml`            | `baler.toml`            |
| `carrier.lock`            | `baler.lock`            |
| `~/.carrier/modules`      | `~/.baler/modules`      |
| `~/.carrier/native-cache` | `~/.baler/native-cache` |
| `[module]`                | `[project]`             |
| `[package_deps]`          | `[dependencies]`        |
| `[module_deps]`           | `[dependencies.baler]`  |
| `[native]`                | `[compiled-code]`       |
| `[test]`                  | `[tool.test]`           |

### Behavior mutation

During the migration, the development begins with changing on how `{baler}` manage the dependencies, fixing the previous inconsistencies in `{carrier}`. In `{baler}`, you don't declare the dependencies source under `source` — it's ambiguous, implicit, and has limited features. Now, the `box-bale` modules dependency name its source with `git` (a GitHub URL, plus `branch`, `tag`, `rev` and `module_dir`) or `url`, instead of `source = "gh:user/repo/..."` from `{carrier}`.

Here's the rest of changes during the development:

- `version` under `[project]` is strictly in semver format, where `{carrier}` has no guard rails on the declaration.
- `r_version` is checked at `baler lock` and `baler install`. `{carrier}` only recorded it.
- `carrier bundle --binary` and `--keep-source` are gone, you have to declare `binary = true` under `[compiled-code]` instead. The native source is now always shipped, and `baler install` checks a shipped binary against the machine and builds from the source when it doesn't fit.
- `baler.lock` covers `build_deps` and `[extras]` packages, as well as external `box-bale` modules that declare a source, and a lock written by `{carrier}` (format 1) is still read.
- `baler install` has explicit `--path`, `--git` and `--url` forms next to the `gh:user/repo` one, and `--extras` with `--install-deps`.

### Moving a module over

Here's what you need to do when migrating `{carrier}` module, assuming you have an existing package made with `{carrier}`, into `{baler}`: 

1.  Rename `carrier.toml` to `baler.toml` and remove `carrier.lock` so that `baler.lock` can take place in.
2.  Rename the tables as listed above, and rewrite each `source = "gh:..."` under the module dependencies with `git = "https://github.com/..."`.
3.  Check that `version` is semver and that `r_version` fits the R you use.
4.  Run `baler lock`, then `baler bundle` again, since an archive made by `{carrier}` isn't a `{baler}` bundle.
5.  Install the modules again with `baler install`. The ones `{carrier}` installed stay in `~/.carrier/modules`, which `{baler}` doesn't read.
