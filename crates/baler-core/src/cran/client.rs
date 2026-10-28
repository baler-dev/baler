mod download;
mod graph;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use semver::Version;

use crate::cran::packages::RVersion;
use crate::cran::universe::Sources;
use crate::lockfile::BalerLock;
use crate::ops::resolve::ResolvedPackage;

use download::download_and_unpack;
use graph::resolve_all;
pub use graph::Planned;

/// Resolve every package (direct and transitive) to exact versions and
/// repos, without downloading or installing anything — what `baler
/// lock` calls. Only the PACKAGES.gz indices get fetched; no individual
/// package's source or binary is ever transferred, which is what makes
/// this cheap enough to run just to check or refresh a lock.
///
/// Resolves in relock mode (see `resolve_all`): the lock seeds the result
/// rather than being enforced, so a pin that no longer fits `baler.toml`
/// is re-resolved instead of failing, and the lock's transitive closure
/// is carried over.
pub fn resolve_packages(
    packages: &BTreeMap<String, ResolvedPackage>,
    lock: Option<&BalerLock>,
) -> Result<HashMap<String, (Version, String)>> {
    let planned = resolve_all(packages, lock, true)?;
    Ok(planned
        .into_iter()
        .map(|p| (p.name, (p.version.semver().clone(), p.repo)))
        .collect())
}

/// Resolve `packages` against the given sources and return the releases to
/// install, dependencies first. `resolve_packages` and `install_packages`
/// read the network. This is the same resolution with the repositories
/// supplied by the caller, so it can run without one (tests, tools).
pub fn plan_packages(
    packages: &BTreeMap<String, ResolvedPackage>,
    lock: Option<&BalerLock>,
    relock: bool,
    sources: &Sources<'_>,
) -> Result<Vec<Planned>> {
    graph::resolve_all_with(packages, lock, relock, sources)
}

/// Install a set of resolved R packages into `lib_path`, resolving first
/// via `resolve_all`.
///
/// Returns the full resolved set (direct and transitive), minus anything
/// that resolved successfully but then failed to actually download/install
/// as a transitive dep (skipped with a warning below): a set that lists a
/// package that isn't actually there would be worse than one that omits it.
///
/// Resolves in install mode (see `resolve_all`): a lock is enforced, never
/// rewritten. When a lock is in use, a package that can't be fetched is
/// always an error, direct or not. Skipping it would quietly install
/// something other than what the lock records.
pub fn install_packages(
    packages: &BTreeMap<String, ResolvedPackage>,
    lib_path: &Path,
    lock: Option<&BalerLock>,
) -> Result<HashMap<String, (Version, String)>> {
    let planned = resolve_all(packages, lock, false)?;
    let mut globally_resolved: HashMap<String, (Version, String)> = planned
        .iter()
        .map(|p| (p.name.clone(), (p.version.semver().clone(), p.repo.clone())))
        .collect();

    std::fs::create_dir_all(lib_path)
        .with_context(|| format!("Failed to create R lib dir: {}", lib_path.display()))?;

    // `planned` is already in install order, dependencies first.
    for resolved in &planned {
        let pkg = &resolved.name;
        let pkg_dir = lib_path.join(pkg);

        if pkg_dir.is_dir() {
            let desc_path = pkg_dir.join("DESCRIPTION");
            match read_installed_version(&desc_path) {
                Ok(installed_version) => {
                    // Compare versions the way R does. Comparing the text would
                    // reinstall a package whose lock spelled 0.3-1 as 0.3.1.
                    if installed_version.ver() == resolved.version.ver() {
                        println!(" [ok] {} {} (already satisfied)", pkg, installed_version);
                        continue;
                    }
                    println!(" [switching] {} {} → {}...", pkg, installed_version, resolved.version);
                }
                Err(_) => {
                    println!(" [reinstalling] {} (could not read installed version)...", pkg);
                }
            }
        } else {
            println!(" [installing] {} {}...", pkg, resolved.version);
        }

        match download_and_unpack(pkg, resolved.version.as_str(), &resolved.repo, lib_path) {
            Ok(()) => {
                println!(" [done] {} {}", pkg, resolved.version);
            }
            Err(e) => {
                let is_direct = packages.contains_key(pkg.as_str());
                if is_direct || lock.is_some() {
                    return Err(e.context(format!("Failed to install {}", pkg)));
                } else {
                    eprintln!(" [warn] skipping transitive dep {} - {}", pkg, e);
                    globally_resolved.remove(pkg);
                }
            }
        }
    }

    Ok(globally_resolved)
}

/// Read the installed version of a package from its `DESCRIPTION` file.
/// Goes through the same `RVersion::parse` every other CRAN version
/// does, rather than its own ad-hoc normalization — the previous inline
/// `.replace('-', '.')` here produced invalid semver syntax for any
/// package with more than 3 version components (e.g. `RcppEigen`'s
/// `0.3.4.0.2`), so "already satisfied" silently never matched for
/// those packages even right after a successful install.
///
/// `pub(crate)`: reused by `ops/compile.rs`'s local-satisfied check,
/// so it can't drift from what `install_packages` itself considers
/// "already satisfied".
pub(crate) fn read_installed_version(desc_path: &Path) -> Result<RVersion> {
    let content = std::fs::read_to_string(desc_path)
        .with_context(|| format!("Failed to read DESCRIPTION at {}", desc_path.display()))?;

    for line in content.lines() {
        if let Some(ver_str) = line.strip_prefix("Version:") {
            return RVersion::parse(ver_str.trim());
        }
    }

    anyhow::bail!(
        "No Version field found in DESCRIPTION at {}",
        desc_path.display()
    )
}
