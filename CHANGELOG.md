# Changelog

# Development version

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
