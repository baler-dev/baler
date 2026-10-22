use anyhow::{bail, Context, Result};
use std::collections::{btree_map::Entry, BTreeMap};
use std::path::Path;

use crate::baler_toml::{BalerToml, ModuleDep, PackageDep};
use crate::lockfile::{self, BalerLock, LockGroup, LockedModule, PinnedPackage};
use crate::ops::resolve;
use crate::ops::module_graph::ModulePin;
use crate::ops::resolve::ResolvedPlan;
#[cfg(feature = "network")]
use crate::ops::install::GitHubFetcher;
#[cfg(feature = "network")]
use crate::ops::module_graph::{self, PinnedFetcher};

/// How much of an existing lock `baler lock` may move.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateScope {
    /// Keep every pin that still fits `baler.toml`. Only what the manifest
    /// changed is added, dropped or re-resolved.
    #[default]
    Keep,
    /// Ignore every existing pin and resolve everything again
    /// (`baler lock --update`).
    All,
    /// Resolve only these packages again; every other pin stays
    /// (`baler lock --update <pkg>`).
    Packages(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct LockOptions {
    pub update: UpdateScope,
    /// Record the detected R version in the lock. Only for a lock that is
    /// being created or updated.
    pub with_r_version: bool,
    /// Delete `baler.lock` instead of writing one.
    pub remove: bool,
    /// Never write. Fail when the lock is missing or doesn't match
    /// `baler.toml`.
    pub check: bool,
}

/// Resolve `path`'s R package dependencies to exact versions and repos,
/// then write `baler.lock`. With `update: true`, any existing lock is
/// ignored and everything is re-resolved fresh; otherwise packages the
/// lock already pins are kept at their pinned version (see
/// `resolve_only` / `resolve_all`'s locked-package handling).
///
/// Thin wrapper over `run_with`, kept so existing callers don't change.
pub fn run(path: &str, update: bool, with_r_version: bool, remove: bool) -> Result<()> {
    let opts = LockOptions {
        update: if update { UpdateScope::All } else { UpdateScope::Keep },
        with_r_version,
        remove,
        check: false,
    };
    run_with(path, &opts)
}

/// What `baler lock` does, by situation:
///
/// - No lock yet: resolve everything and write it.
/// - A lock that already matches `baler.toml`: write nothing. The file's
///   bytes and mtime are left alone, so running it twice, or on another
///   machine, never shows up in a diff.
/// - `baler.toml` changed: keep every pin that still fits, add or drop what
///   the manifest changed, re-resolve only a package whose repo or
///   constraint no longer fits its pin, and report what moved.
/// - `update: All`: ignore existing pins and resolve everything again.
/// - `update: Packages`: re-resolve just those packages (and anything they
///   now require that the lock can't satisfy); every other pin stays.
/// - `check: true`: never writes. Fails if the lock is missing or a normal
///   run would change it.
///
/// Runtime packages, `[compiled-code].build_deps`, and `[extras]` packages
/// are each resolved on their own, then merged into one entry per
/// package tagged with the groups that need it. A package the groups
/// resolve to different versions is an error, not a silent pick.
///
/// The R version in the lock is provenance only. It is never enforced (the
/// real constraint is `baler.toml`'s `module.r_version`, checked on every
/// run), and it describes whichever machine last wrote it, so a plain run
/// leaves it exactly as it is. It changes only when written deliberately:
///
/// - `with_r_version` records the detected version, and is refused on an
///   existing lock unless the lock is being updated too.
/// - A full `update` refreshes it, but only if the old lock recorded one.
///
/// `remove: true` safely deletes `baler.lock` instead of writing one, and
/// returns before any resolution happens. This is always safe: a
/// missing lock is not an error state anywhere in baler. `ops::install`
/// treats "no lock" as "resolve fresh", exactly as if `baler lock` had
/// never been run, and `ops::bundle` simply ships no lock.
pub fn run_with(path: &str, opts: &LockOptions) -> Result<()> {
    let project_root = Path::new(path);

    if opts.remove {
        let lock_path = project_root.join(lockfile::LOCK_FILE_NAME);
        if lock_path.exists() {
            std::fs::remove_file(&lock_path)
                .with_context(|| format!("Failed to remove {}", lock_path.display()))?;
            println!("Removed {}", lockfile::LOCK_FILE_NAME);
        } else {
            println!("No {} present, nothing to remove.", lockfile::LOCK_FILE_NAME);
        }
        return Ok(());
    }

    if opts.check && opts.update != UpdateScope::Keep {
        bail!("--check never writes the lock, so it can't be combined with --update.");
    }
    if opts.check && opts.with_r_version {
        bail!("--check never writes the lock, so it can't be combined with --with-rver.");
    }

    let toml = BalerToml::from_dir(project_root)?;

    let r_spec = toml.project.r_version_spec()?;
    crate::version::check_r_version(&r_spec)?;

    let existing = lockfile::read(project_root)?;

    if opts.with_r_version && existing.is_some() && opts.update == UpdateScope::Keep {
        bail!(
            "--with-rver records which R wrote the lock, and {} already exists. \
             To record it, update the lock too: `baler lock --update --with-rver`.",
            lockfile::LOCK_FILE_NAME
        );
    }
    if opts.check && existing.is_none() {
        bail!(
            "No {} found. Run `baler lock` to create it.",
            lockfile::LOCK_FILE_NAME
        );
    }
    if let UpdateScope::Packages(names) = &opts.update {
        let Some(lock) = existing.as_ref() else {
            bail!(
                "No {} to update. Run `baler lock` to create it first.",
                lockfile::LOCK_FILE_NAME
            );
        };
        for name in names {
            if !lock.packages.iter().any(|p| &p.name == name)
                && !lock.modules.iter().any(|m| &m.name == name)
            {
                bail!(
                    "'{}' is not in {}, so there is nothing to update. \
                     If it is a new dependency, run `baler lock` to add it.",
                    name,
                    lockfile::LOCK_FILE_NAME
                );
            }
        }
    }

    // The lock that seeds resolution: every pin for a plain run, none for a
    // full update, and every pin except the named ones for a partial one.
    let seed: Option<BalerLock> = match (&opts.update, &existing) {
        (UpdateScope::All, _) | (_, None) => None,
        (UpdateScope::Keep, Some(lock)) => Some(lock.clone()),
        (UpdateScope::Packages(names), Some(lock)) => Some(lock.without(names)),
    };

    let runtime_deps = (!toml.dependencies.packages.is_empty())
        .then(|| toml.dependencies.packages.clone());
    let module_deps = toml.dependencies.baler.clone();
    let build_deps = toml.compiled_code.as_ref()
        .and_then(|c| c.build_deps.clone())
        .filter(|deps| !deps.is_empty());
    let extras_deps = toml.extras.as_ref()
        .map(|e| e.packages.clone())
        .filter(|deps| !deps.is_empty());

    let mut pinned: BTreeMap<String, PinnedPackage> = BTreeMap::new();
    let mut module_pins: Vec<ModulePin> = Vec::new();
    let groups = [
        (LockGroup::Runtime, runtime_deps),
        (LockGroup::Build, build_deps),
        (LockGroup::Extras, extras_deps),
    ];
    for (group, package_deps) in groups {
        if group != LockGroup::Runtime && package_deps.is_none() {
            continue;
        }
        let modules = if group == LockGroup::Runtime { &module_deps } else { &None };
        let locked_modules: &[LockedModule] =
            seed.as_ref().map(|l| l.modules.as_slice()).unwrap_or(&[]);
        let (plan, pins) = plan_for_group(&toml, group, &package_deps, modules, locked_modules)?;
        module_pins.extend(pins);
        let group_pins = resolve::resolve_only(&plan, seed.as_ref())?;
        for (name, (version, repo)) in group_pins {
            match pinned.entry(name) {
                Entry::Vacant(slot) => {
                    slot.insert(PinnedPackage { version, repo, groups: vec![group] });
                }
                Entry::Occupied(mut slot) => {
                    let name = slot.key().clone();
                    let pin = slot.get_mut();
                    if pin.version != version || pin.repo != repo {
                        bail!(
                            "'{}' resolves to {} ({}) for one dependency group and {} ({}) \
                             for another. Align the version constraints in baler.toml so \
                             one version satisfies both.",
                            name, pin.version, pin.repo, version, repo
                        );
                    }
                    pin.groups.push(group);
                }
            }
        }
    }

    // Detect R only when its answer ends up in the lock.
    let r_version: Option<String> = if opts.with_r_version {
        Some(crate::paths::detect_r_version()?.to_string())
    } else {
        match (&opts.update, &existing) {
            (UpdateScope::All, Some(old)) if old.r_version.is_some() => {
                Some(crate::paths::detect_r_version()?.to_string())
            }
            (UpdateScope::All, _) => None,
            (_, Some(old)) => old.r_version.clone(),
            (_, None) => None,
        }
    };

    // Without the network feature modules are not walked, so what the lock
    // already pins is kept as it is.
    let locked_modules: Vec<LockedModule> = if cfg!(feature = "network") {
        module_pins.iter().map(locked_module).collect()
    } else {
        seed.as_ref().map(|l| l.modules.clone()).unwrap_or_default()
    };
    let new_lock = lockfile::build_grouped(&pinned, r_version.as_deref()).with_modules(locked_modules);

    match &existing {
        Some(old) if old.same_content(&new_lock) => {
            println!(
                "{} is up to date ({} packages)",
                lockfile::LOCK_FILE_NAME,
                new_lock.packages.len()
            );
            return Ok(());
        }
        Some(old) if opts.check => {
            bail!(
                "{} is out of date with baler.toml:\n  {}\nRun `baler lock` to update it.",
                lockfile::LOCK_FILE_NAME,
                old.changes(&new_lock).join("\n  ")
            );
        }
        _ => {}
    }
    if opts.check {
        bail!("No {} found. Run `baler lock` to create it.", lockfile::LOCK_FILE_NAME);
    }

    lockfile::write_lock(project_root, &new_lock)?;
    match &existing {
        None => println!(
            "Wrote {} ({} packages)",
            lockfile::LOCK_FILE_NAME,
            new_lock.packages.len()
        ),
        Some(old) => {
            println!(
                "Updated {} ({} packages):",
                lockfile::LOCK_FILE_NAME,
                new_lock.packages.len()
            );
            for line in old.changes(&new_lock) {
                println!("  {}", line);
            }
        }
    }

    Ok(())
}

fn locked_module(pin: &ModulePin) -> LockedModule {
    LockedModule::new(&pin.name, &pin.version, &pin.source, pin.rev.clone())
}

/// The plan for one dependency group. The runtime group also walks the
/// modules this module depends on, so the R packages those modules need are
/// locked too, and each module is pinned to the commit it was fetched at.
/// A module the lock already pins is fetched at that commit, so a plain
/// `baler lock` never moves it. A module dependency that declares no source
/// cannot be fetched, so it is skipped with a warning instead of stopping the
/// lock.
#[cfg(feature = "network")]
fn plan_for_group(
    toml: &BalerToml,
    group: LockGroup,
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    modules: &Option<BTreeMap<String, ModuleDep>>,
    locked: &[LockedModule],
) -> Result<(ResolvedPlan, Vec<ModulePin>)> {
    let has_modules = modules.as_ref().is_some_and(|found| !found.is_empty());
    if group != LockGroup::Runtime || !has_modules {
        return Ok((resolve::resolve(package_deps, modules)?, Vec::new()));
    }
    let fetcher = PinnedFetcher { inner: &GitHubFetcher, locked };
    let (plan, pins) = module_graph::resolve_dependencies_pinned(
        &toml.dependencies.packages,
        modules.as_ref(),
        &fetcher,
        true,
    )?;
    for pin in &pins {
        match &pin.rev {
            Some(rev) => println!("  module {} {} (pinned to {rev})", pin.name, pin.version),
            None => println!("  module {} {} (the source gave no commit to pin)", pin.name, pin.version),
        }
    }
    Ok((plan, pins))
}

/// Without the `network` feature there is nothing to fetch a module with, so
/// module dependencies are not walked.
#[cfg(not(feature = "network"))]
fn plan_for_group(
    _toml: &BalerToml,
    _group: LockGroup,
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    modules: &Option<BTreeMap<String, ModuleDep>>,
    _locked: &[LockedModule],
) -> Result<(ResolvedPlan, Vec<ModulePin>)> {
    Ok((resolve::resolve(package_deps, modules)?, Vec::new()))
}
