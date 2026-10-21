use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

use crate::baler_toml::{BalerToml, ModuleDep, ModuleSource};
use crate::formats::tar;
use crate::lockfile::{self, BalerLock};
use crate::ops::external_tools::check_external_tools;
use crate::ops::module_graph::plan_module_installs;
use crate::ops::resolve;
use crate::paths::resolve_install_dir;

use super::github::{download_and_install, install_from_github, GitHubFetcher};
use super::native::build_native_if_present;

/// Install a module from a `.tar.gz`, enforcing a lock if there is one.
///
/// The lock is `lock` when the caller has one (a local project directory),
/// otherwise whatever `baler.lock` the archive shipped. It is only read:
/// an install never writes or regenerates `baler.lock`, and fails with a
/// pointer to `baler lock` when the lock doesn't cover `baler.toml`. A dry
/// run (`install_deps` false) only prints what would be installed.
///
/// `extras` also installs this module's own `[extras]` packages. Only the
/// module being installed contributes extras: the modules it depends on
/// are never walked here, so their extras are never installed.
pub(super) fn install_from_tar(tar_path: &PathBuf, install_deps: bool, lock: Option<&BalerLock>, extras: bool) -> Result<()> {
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
    let package_deps = (!toml.dependencies.packages.is_empty())
        .then(|| toml.dependencies.packages.clone());
    let module_deps = toml.dependencies.baler.clone();

    // An `[extras]` table holding only `external` tools has nothing for
    // baler to install, so it counts as no extras at all.
    let wanted_extras = toml.extras.as_ref().filter(|extra| {
        extras
            && (!extra.packages.is_empty()
                || extra.baler.as_ref().is_some_and(|modules| !modules.is_empty()))
    });
    if extras && wanted_extras.is_none() {
        println!("No [extras] packages or modules declared in '{name}', nothing extra to install.");
    }

    let plan = match (&effective_lock, wanted_extras) {
        (Some(locked), Some(extra)) => resolve::resolve_locked_with_extras(
            &package_deps,
            &module_deps,
            &extra.packages,
            &extra.baler,
            locked,
        )?,
        (Some(locked), None) => resolve::resolve_locked(&package_deps, &module_deps, locked)?,
        (None, Some(extra)) => resolve::resolve_with_extras(
            &package_deps,
            &module_deps,
            &extra.packages,
            &extra.baler,
        )?,
        (None, None) => resolve::resolve(&package_deps, &module_deps)?,
    };
    if wanted_extras.is_some() {
        println!("Including [extras].");
    }
    println!("Dependencies:");
    resolve::print_plan(&plan);
    resolve::execute_plan(&plan, !install_deps, effective_lock.as_ref())?;

    let mut modules_wanted: BTreeMap<String, ModuleDep> = module_deps.clone().unwrap_or_default();
    if let Some(extra) = wanted_extras {
        modules_wanted.extend(extra.baler.clone().unwrap_or_default());
    }
    install_declared_modules(&modules_wanted, install_deps)?;

    build_native_if_present(&module_path, &name, install_deps, &toml)?;

    Ok(())
}

/// Installs the module dependencies that are not installed yet, each from
/// the source its dependency entry declares. A dry run (no `--install-deps`)
/// installs nothing and does not touch the network, and without the
/// `network` feature there is nothing to fetch a module with.
///
/// Each module installs like any other, so its own R packages and module
/// dependencies follow. The module being installed is already in place by
/// now, so a dependency cycle ends instead of looping.
fn install_declared_modules(
    modules: &BTreeMap<String, ModuleDep>,
    install_deps: bool,
) -> Result<()> {
    if modules.is_empty() || !install_deps || !cfg!(feature = "network") {
        return Ok(());
    }
    let planned = plan_module_installs(modules, &GitHubFetcher, &resolve::already_installed_module)?;
    for module in planned {
        println!("  Installing module {} {} from {}...", module.name, module.version, module.source);
        match &module.source {
            ModuleSource::Git { url, git_ref, module_dir } => {
                install_from_github(url, module_dir.as_deref(), git_ref.as_deref(), true, false)
            }
            ModuleSource::Url(url) => download_and_install(url, true, false),
        }
        .with_context(|| format!("Failed to install module '{}'", module.name))?;
    }
    Ok(())
}

pub(super) fn install_from_dir(project_root: &PathBuf, install_deps: bool, extras: bool) -> Result<()> {
    if !project_root.join("baler.toml").exists() {
        bail!(
            "No baler.toml found in {}. Is this a baler module project?",
            project_root.display()
        );
    }

    // Only a --path install reads straight from a project directory
    // that's still being worked on, so [extras.external] (dev-only
    // tooling: docs generators, linters) is worth surfacing here. A
    // --git/--url/registry install fetches something closer to a
    // finished release than a project someone's actively developing,
    // so those paths skip this.
    let toml = BalerToml::from_dir(project_root)?;
    check_external_tools(toml.extras.as_ref());

    let lock = lockfile::read(project_root)
        .with_context(|| format!("Failed to read baler.lock in {}", project_root.display()))?;

    let tmp = TempDir::new().context("Failed to create temp directory")?;
    let output_path = tmp.path().join("module.tar.gz");

    crate::ops::bundle::bundle_to(project_root, &output_path)
        .context("Failed to bundle project")?;

    install_from_tar(&output_path, install_deps, lock.as_ref(), extras)
}

/// Not implemented yet, there's no module registry protocol defined.
/// This exists so the CLI surface (--repo/--version flags, arg
/// threading, mutual exclusivity with --git/--url/local sources) is
/// already wired and tested; once a real registry protocol exists,
/// only this function's body changes.
pub(super) fn install_from_registry(name: &str, repo: &str, version: Option<&str>, _install_deps: bool, _extras: bool) -> Result<()> {
    let wanted = match version {
        Some(v) => format!("{name}@{v}"),
        None => name.to_owned(),
    };
    bail!(
        "Module registries aren't implemented yet, wanted to install '{wanted}' from '{repo}'.\n\
         For now, install directly: a local path, --git <url>, or --url <tarball>."
    )
}
