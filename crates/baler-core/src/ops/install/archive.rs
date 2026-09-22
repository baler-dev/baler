use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tempfile::TempDir;

use crate::formats::tar;
use crate::lockfile::{self, BalerLock};
use crate::ops::resolve;
use crate::paths::resolve_install_dir;

use super::native::build_native_if_present;

/// Returns the full resolved R package set (direct and transitive) that
/// `execute_plan` produced, so a caller installing from a local
/// directory can write it out as `baler.lock` when `--write-lock` is
/// passed. A dry run or a plan with no packages yields an empty map.
pub(super) fn install_from_tar(tar_path: &PathBuf, install_deps: bool, lock: Option<&BalerLock>) -> Result<()> {
    if !tar_path.exists() {
        bail!("File not found: {}", tar_path.display());
    }

    let toml = tar::read_toml(tar_path)
        .with_context(|| format!("Failed to read baler.toml from {}", tar_path.display()))?;

    let r_spec = toml.project.r_version_spec()?;
    crate::version::check_r_version(&r_spec)?;

    let name = toml.project.name.clone();
    let version = toml.project.version.clone();

    let install_dir = resolve_install_dir()?;
    let module_path = install_dir.join(&name);

    std::fs::create_dir_all(&install_dir)
        .context("Failed to create install directory")?;

    if module_path.exists() {
        std::fs::remove_dir_all(&module_path)
            .with_context(|| format!("Failed to remove existing: {}", module_path.display()))?;
    }

    // Also clean up old dist-info if present
    let dist_info = install_dir.join(format!("{}-{}.dist-info", name, version));
    if dist_info.exists() {
        std::fs::remove_dir_all(&dist_info)?;
    }

    tar::unpack(tar_path, &install_dir, &name, &version)
        .with_context(|| format!("Failed to unpack {}", tar_path.display()))?;

    println!("Installed '{}' ({}) -> {}", name, version, module_path.display());

    // A dir/github install passes a lock read fresh from the source
    // project. A standalone .tar.gz has no project directory to read
    // one from, fall back to whatever baler.lock the archive itself
    // shipped, if the module was bundled with one.
    let embedded_lock = tar::read_lock(tar_path)?;
    let effective_lock = lock.cloned().or(embedded_lock);

    // `resolve`/`resolve_locked` take an absent dependency table as
    // `None`, not `Some(<empty map>)`, matching how compile.rs and
    // native.rs already call them.
    let package_deps = (!toml.project.dependencies.packages.is_empty())
        .then(|| toml.project.dependencies.packages.clone());
    let module_deps = toml.project.dependencies.baler.clone();

    let plan = match &effective_lock {
        Some(locked) => resolve::resolve_locked(&package_deps, &module_deps, locked)?,
        None => resolve::resolve(&package_deps, &module_deps)?,
    };
    println!("Dependencies:");
    resolve::print_plan(&plan);
    resolve::execute_plan(&plan, !install_deps, effective_lock.as_ref())?;

    build_native_if_present(&module_path, &name, install_deps, &toml)?;

    Ok(())
}

pub(super) fn install_from_dir(project_root: &PathBuf, install_deps: bool) -> Result<()> {
    if !project_root.join("baler.toml").exists() {
        bail!(
            "No baler.toml found in {}. Is this a baler module project?",
            project_root.display()
        );
    }

    let lock = lockfile::read(project_root)
        .with_context(|| format!("Failed to read baler.lock in {}", project_root.display()))?;

    let tmp = TempDir::new().context("Failed to create temp directory")?;
    let output_path = tmp.path().join("module.tar.gz");

    crate::ops::bundle::bundle_to(project_root, &output_path)
        .context("Failed to bundle project")?;

    install_from_tar(&output_path, install_deps, lock.as_ref())
}

/// Not implemented yet, there's no module registry protocol defined.
/// This exists so the CLI surface (--repo flag, arg threading, mutual
/// exclusivity with gh:/local sources) is already wired and tested; once
/// a real registry protocol exists, only this function's body changes.
pub(super) fn install_from_registry(name: &str, repo: &str, _install_deps: bool) -> Result<()> {
    bail!(
        "Module registries aren't implemented yet, wanted to install '{name}' from '{repo}'.\n\
         For now, install directly: a local path, or gh:user/repo."
    )
}
