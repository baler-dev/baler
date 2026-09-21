use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::baler_toml::BalerToml;
use crate::formats::tar;

pub fn run(path: &str, binary: bool, keep_source: bool) -> Result<()> {
    if keep_source && !binary {
        bail!("--keep-source only applies together with --binary.");
    }

    let project_root = PathBuf::from(path);

    if !project_root.exists() {
        bail!("Path does not exist: {}", project_root.display());
    }
    if !project_root.is_dir() {
        bail!("Path is not a directory: {}", project_root.display());
    }

    let toml = BalerToml::from_dir(&project_root)?;
    let src_path = toml.resolve_src_dir(&project_root)?;
    let meta = &toml.project;

    if binary {
        // Compiled here for the side effect, the .lib/ artifacts it
        // produces. There used to be a return value captured for
        // manifest.json's native artifact metadata; nothing ever read
        // that back, so it is no longer captured.
        crate::ops::compile::run(&project_root, crate::ops::compile::CompileMode::Normal)?;
    }

    // .lib/ is now dot-prefixed, so the archive writer's own
    // hidden-file filter excludes it from a plain bundle automatically
    // — no explicit exclusion needed there anymore. --binary needs the
    // opposite: force it back in despite the dot, since shipping it is
    // the whole point. --binary without --keep-source additionally
    // excludes native source; a mismatched/missing tag on install then
    // has nothing to fall back to and must error clearly (not yet
    // implemented on the install side — see the TODO on install.rs).
    let (exclude, force_include): (Vec<PathBuf>, Vec<PathBuf>) = if binary {
        let lib_dirs: Vec<PathBuf> = toml.resolve_native_dirs(&project_root)
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
