use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::baler_toml::BalerToml;
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
        }
    }

    // .lib/ is dot-prefixed, so the archive writer's own hidden-file
    // filter excludes it from a plain bundle automatically, no
    // explicit exclusion needed there. `binary = true` needs the
    // opposite: force it back in despite the dot, since shipping it is
    // the whole point. `binary = true` without `keep_source`
    // additionally excludes native source; a mismatched/missing tag
    // on install then has nothing to fall back to and must error
    // clearly (not yet implemented on the install side, see the TODO
    // on install.rs).
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

    tar::bundle(
        &src_path,
        &project_root,
        &output_path,
        &meta.name,
        &meta.version,
        &exclude,
        &force_include,
    )
    .with_context(|| format!("Failed to bundle: {}", src_path.display()))?;

    println!(
        "Bundled '{}' ({}) -> {}",
        meta.name,
        meta.version,
        output_path.display()
    );

    Ok(())
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
