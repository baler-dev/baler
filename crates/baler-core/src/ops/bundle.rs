use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::baler_toml::{BalerToml, BinaryTarget};
use crate::formats::tar;
use crate::lockfile;
use crate::ops::resolve;

/// Bundle without insisting on a current lock: a stale `baler.lock` is
/// reported as a warning. See `run_with`.
pub fn run(path: &str) -> Result<()> {
    run_with(path, false)
}

/// `binary` and `keep_source` come from `[compiled-code]` in
/// baler.toml, not from flags. `keep_source` without `binary` is
/// rejected when the manifest is parsed (`BalerToml::from_dir`).
///
/// A bundle ships `baler.lock` as it is, and whoever installs the bundle
/// has no project to run `baler lock` in. So a lock that doesn't match
/// `baler.toml` is caught here, on the author's side. The check is offline
/// and only looks at what `baler.toml` declares (see
/// `resolve::lock_problems_for`). With `locked: false` it warns and bundles
/// anyway. With `locked: true`, in the style of `cargo publish --locked`,
/// it fails, and a missing or unreadable lock fails too.
pub fn run_with(path: &str, locked: bool) -> Result<()> {
    let project_root = PathBuf::from(path);

    if !project_root.exists() {
        bail!("Path does not exist: {}", project_root.display());
    }
    if !project_root.is_dir() {
        bail!("Path is not a directory: {}", project_root.display());
    }

    let toml = BalerToml::from_dir(&project_root)?;
    check_lock_before_bundling(&project_root, &toml, locked)?;
    let src_path = toml.resolve_src_dir(&project_root)?;
    let meta = &toml.project;

    let (binary, keep_source) = toml
        .compiled_code
        .as_ref()
        .map(|c| (c.binary, c.keep_source))
        .unwrap_or((false, false));

    if binary {
        // Compiled here for the side effect, the .lib/ artifacts it
        // produces. There used to be a return value captured for
        // manifest.json's native artifact metadata; nothing ever read
        // that back, so only its emptiness is checked.
        let compiled =
            crate::ops::compile::run(&project_root, crate::ops::compile::CompileMode::Normal)?;
        if compiled.is_empty() {
            eprintln!(
                "warning: [compiled-code] sets `binary = true`, but '{}' has no \
                 native code to compile. Bundling without a binary.",
                meta.name
            );
        } else if !keep_source {
            let built_for = crate::ops::compile::host_target()?
                .map_or_else(|| "this machine".to_owned(), |target| target.describe());
            eprintln!(
                "note: '{}' ships a binary for {} so far. Run `baler bundle` on each other \
                 platform in this folder: the binaries already in the bundle are kept.",
                meta.name, built_for
            );
        }
    }

    // .lib/ is dot-prefixed, so the archive writer's own hidden-file
    // filter excludes it from a plain bundle automatically, no
    // explicit exclusion needed there. `binary = true` needs the
    // opposite: force it back in despite the dot, since shipping it is
    // the whole point. `binary = true` without `keep_source`
    // additionally excludes native source; install then has nothing to
    // fall back to when the binary's platform doesn't match, and fails
    // before it replaces anything (see `check_shipped_binaries` in
    // install/archive.rs).
    let (exclude, force_include): (Vec<PathBuf>, Vec<PathBuf>) = if binary {
        let lib_dirs: Vec<PathBuf> = toml
            .resolve_native_dirs(&project_root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|d| d.parent().map(|p| p.join(".lib")))
            .collect();
        let src_exclude = if keep_source {
            Vec::new()
        } else {
            toml.resolve_native_dirs(&project_root).unwrap_or_default()
        };
        (src_exclude, lib_dirs)
    } else {
        (Vec::new(), Vec::new())
    };

    let cwd = std::env::current_dir()
        .context("Failed to get current working directory")?;

    let output_path = cwd.join(format!("{}_{}.tar.gz", meta.name, meta.version));

    // Written beside its destination first, so an earlier bundle of this
    // module is still there to take the other platforms' binaries from.
    let fresh_path = output_path.with_extension("gz.new");
    tar::bundle(
        &src_path,
        &project_root,
        &fresh_path,
        &meta.name,
        &meta.version,
        &exclude,
        &force_include,
    )
    .with_context(|| format!("Failed to bundle: {}", src_path.display()))?;
    place_bundle(&fresh_path, &output_path, binary)?;

    println!(
        "Bundled '{}' ({}) -> {}",
        meta.name,
        meta.version,
        output_path.display()
    );
    if binary {
        for lib in tar::read_shipped_libs(&output_path)? {
            let built_for = lib.targets.iter().map(BinaryTarget::describe).collect::<Vec<_>>().join(", ");
            println!("  binaries in {}: {}", lib.lib_dir.display(), built_for);
        }
    }

    Ok(())
}

/// Moves the bundle just built into place. With `binary = true` and an
/// earlier bundle already there, the earlier one's other-platform binaries
/// come along. An earlier bundle that holds different source, or can't be
/// read, is replaced instead, with a note saying so.
fn place_bundle(fresh_path: &Path, output_path: &Path, binary: bool) -> Result<()> {
    if binary && output_path.exists() {
        let combined_path = output_path.with_extension("gz.combined");
        match tar::add_platforms(fresh_path, output_path, &combined_path) {
            Ok(true) => {
                std::fs::rename(&combined_path, output_path)
                    .with_context(|| format!("Failed to write {}", output_path.display()))?;
                std::fs::remove_file(fresh_path).ok();
                return Ok(());
            }
            Ok(false) => eprintln!(
                "note: {} holds different source, so its binaries are not kept. Replacing it.",
                output_path.display()
            ),
            Err(e) => eprintln!(
                "note: the binaries in {} can't be reused ({:#}). Replacing it.",
                output_path.display(),
                e
            ),
        }
        std::fs::remove_file(&combined_path).ok();
    }
    std::fs::rename(fresh_path, output_path)
        .with_context(|| format!("Failed to write {}", output_path.display()))
}

fn check_lock_before_bundling(project_root: &Path, toml: &BalerToml, locked: bool) -> Result<()> {
    let lock = match lockfile::read(project_root) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            if locked {
                bail!(
                    "--locked needs a {}, and there isn't one. Run `baler lock` first.",
                    lockfile::LOCK_FILE_NAME
                );
            }
            return Ok(());
        }
        Err(e) => {
            if locked {
                return Err(e.context(format!(
                    "--locked needs a readable {}",
                    lockfile::LOCK_FILE_NAME
                )));
            }
            eprintln!(
                "warning: {} can't be read ({:#}). It will be bundled as it is.",
                lockfile::LOCK_FILE_NAME,
                e
            );
            return Ok(());
        }
    };

    let problems = resolve::lock_problems_for(toml, &lock)?;
    if problems.is_empty() {
        return Ok(());
    }

    let list = problems.join("\n  - ");
    if locked {
        bail!(
            "{} is out of date with baler.toml:\n  - {}\nRun `baler lock`, then bundle again.",
            lockfile::LOCK_FILE_NAME,
            list
        );
    }
    eprintln!(
        "warning: {} is out of date with baler.toml:\n  - {}\n\
         It will be bundled as it is, and installing the bundle will fail. \
         Run `baler lock` first, or pass --locked to make this an error.",
        lockfile::LOCK_FILE_NAME,
        list
    );
    Ok(())
}

/// Used by `install` when bundling a GitHub-downloaded module.
pub fn bundle_to(project_root: &Path, output_path: &Path) -> Result<()> {
    let toml = BalerToml::from_dir(project_root)?;
    let src_path = toml.resolve_src_dir(project_root)?;
    let meta = &toml.project;

    tar::bundle(
        &src_path,
        project_root,
        output_path,
        &meta.name,
        &meta.version,
        &[],
        &[],
    )
}
