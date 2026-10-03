use anyhow::{bail, Context, Result};
use std::collections::{btree_map::Entry, BTreeMap};
use std::path::Path;

use crate::baler_toml::BalerToml;
use crate::lockfile::{self, LockGroup, PinnedPackage};
use crate::ops::resolve;

/// Resolve `path`'s R package dependencies to exact versions and repos,
/// then write `baler.lock`. With `update: true`, any existing lock is
/// ignored and everything is re-resolved fresh; otherwise packages the
/// lock already pins are kept at their pinned version (see
/// `resolve_only` / `resolve_all`'s locked-package handling).
///
/// Runtime packages, `[compiled-code].build_deps`, and `[extras]` packages
/// are each resolved on their own, then merged into one entry per
/// package tagged with the groups that need it. A package the groups
/// resolve to different versions is an error, not a silent pick.
///
/// `with_r_version` controls whether the detected R version is recorded
/// in the lock as provenance (see `lockfile::write`'s doc comment). This is
/// off by default so routine re-locks across contributors on different
/// R installs stay diff-quiet.
///
/// `remove: true` safely deletes `baler.lock` instead of writing one, and
/// returns before any resolution happens. This is always safe: a
/// missing lock is not an error state anywhere in baler. Every
/// caller (`ops::install`, `ops::bundle`) already treats "no lock" as
/// "resolve fresh", exactly as if `baler lock` had never been run.
pub fn run(path: &str, update: bool, with_r_version: bool, remove: bool) -> Result<()> {
    let project_root = Path::new(path);

    if remove {
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

    let toml = BalerToml::from_dir(project_root)?;

    let r_spec = toml.project.r_version_spec()?;
    crate::version::check_r_version(&r_spec)?;
    let detected = crate::paths::detect_r_version()?;

    let existing = if update { None } else { lockfile::read(project_root)? };

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
        let plan = resolve::resolve(&package_deps, modules)?;
        let group_pins = resolve::resolve_only(&plan, existing.as_ref())?;
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

    let r_version = if with_r_version { Some(detected.to_string()) } else { None };
    lockfile::write_grouped(project_root, &pinned, r_version.as_deref())?;
    println!("Wrote {} ({} packages)", lockfile::LOCK_FILE_NAME, pinned.len());

    Ok(())
}
