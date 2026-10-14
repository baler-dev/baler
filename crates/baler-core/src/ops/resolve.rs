use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::cran::packages::{fetch, fetch_archive_versions, PackageRecord, RVersion};
use crate::lockfile::BalerLock;
use crate::ops::resolve::ResolvedPackage;
use crate::version::VersionSpec;

/// `{name: (exact version, repo)}`.
type Pins = HashMap<String, (Version, String)>;

pub(super) struct ResolvedInstall {
    pub(super) version: RVersion,
    pub(super) repo: String,
}

pub(super) struct RepoResolution {
    pub(super) index: HashMap<String, PackageRecord>,
    pub(super) to_install: HashMap<String, ResolvedInstall>,
}

pub(super) fn topo_order(
    to_install: &HashMap<String, ResolvedInstall>,
    index: &HashMap<String, PackageRecord>,
) -> Vec<String> {
    fn visit(
        pkg: &str,
        to_install: &HashMap<String, ResolvedInstall>,
        index: &HashMap<String, PackageRecord>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if visited.contains(pkg) {
            return;
        }
        visited.insert(pkg.to_owned());

        if to_install.contains_key(pkg) {
            if let Some(record) = index.get(pkg) {
                for (dep, _) in &record.deps {
                    visit(dep, to_install, index, visited, order);
                }
            }
            order.push(pkg.to_owned());
        }
    }

    let mut visited = HashSet::new();
    let mut order = Vec::new();
    for pkg in to_install.keys() {
        visit(pkg, to_install, index, &mut visited, &mut order);
    }
    order
}

/// Resolve every package (direct and transitive) needed to satisfy
/// `packages`, without downloading or installing anything. Shared by
/// `install_packages` (resolve, then install) and `resolve_packages()`
/// (resolve only what `baler lock` calls). Packages are grouped by
/// repo so each PACKAGES.gz is fetched only once per repository.
///
/// If `lock` is `Some`, any requested package it pins is used at that
/// exact version without touching `resolve_install_set` at all. There'll be
/// no constraint solving, no archive fallback, no index-based resolution
/// for that name. A package the lock doesn't mention still resolves
/// fresh, the same way it would with no lock present. This lets a
/// newly added dependency work before the lock is re-written to cover
/// it. A fresh resolution still prefers the lock's pin for any package it
/// reaches, as long as that pin satisfies the constraints, so adding one
/// dependency doesn't move the versions of packages already locked.
///
/// `relock` is the one difference between the two callers:
///
/// - `false` (install): the lock is authoritative. A pin that no longer
///   fits `baler.toml` is an error, and nothing beyond what was requested
///   is added.
/// - `true` (`baler lock`): the lock is a starting point. A directly
///   requested package whose pin no longer fits `baler.toml` (its repo
///   moved, or the constraint tightened past the pinned version) is
///   re-resolved on its own rather than failing, since regenerating the
///   lock is exactly the fix. The dependencies of every pinned package are
///   then walked so the lock keeps its full transitive closure, with each
///   reachable package held at its pinned version. Without that walk a
///   relock would shrink to the directly declared packages.
pub(super) fn resolve_all(
    packages: &BTreeMap<String, ResolvedPackage>,
    lock: Option<&BalerLock>,
    relock: bool,
) -> Result<(BTreeMap<String, RepoResolution>, Pins)> {
    let seed: Pins = match lock {
        Some(l) => l.pins()?,
        None => HashMap::new(),
    };

    let mut by_repo: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (name, pkg) in packages {
        by_repo
            .entry(pkg.repo.clone())
            .or_default()
            .insert(name.clone(), pkg.version_spec.clone());
    }

    // Shared across all repo groups, so a package appearing as a
    // transitive dep under more than one repo isn't independently
    // re-resolved (and potentially silently downgraded) by whichever
    // repo group happens to process it last.
    let mut globally_resolved: Pins = HashMap::new();
    let mut per_repo: BTreeMap<String, RepoResolution> = BTreeMap::new();

    for (repo, pkgs) in &by_repo {
        println!("Fetching package index from {}...", repo);
        let index = fetch(repo)?;

        let mut to_install: HashMap<String, ResolvedInstall> = HashMap::new();
        let mut unlocked: BTreeMap<String, String> = BTreeMap::new();
        let mut locked_roots: Vec<String> = Vec::new();

        for (name, spec) in pkgs {
            let locked = match lock {
                Some(l) => l.locked_version(name)?,
                None => None,
            };

            match locked {
                Some((version, locked_repo)) => {
                    if locked_repo != *repo {
                        if relock {
                            println!(
                                " [relock] '{name}' moved from {locked_repo} to {repo} in \
                                 baler.toml, resolving it again"
                            );
                            unlocked.insert(name.clone(), spec.clone());
                            continue;
                        }
                        bail!(
                            "baler.lock pins '{name}' to repo {locked_repo}, but baler.toml \
                             now points at {repo}. Run `baler lock` to update the lock, or \
                             revert baler.toml's repo for this package."
                        );
                    }

                    let required = VersionSpec::parse(spec)?;
                    if !required.matches(&version) {
                        if relock {
                            println!(
                                " [relock] '{name}' is pinned to {version}, but baler.toml now \
                                 requires '{spec}', resolving it again"
                            );
                            unlocked.insert(name.clone(), spec.clone());
                            continue;
                        }
                        bail!(
                            "baler.lock pins '{name}' to {version}, but baler.toml now \
                             requires '{spec}', so the lock is stale. Run `baler lock` to \
                             update it."
                        );
                    }

                    globally_resolved.insert(name.clone(), (version.clone(), repo.clone()));
                    to_install.insert(name.clone(), ResolvedInstall {
                        version: RVersion::synthetic(version),
                        repo: repo.clone(),
                    });
                    locked_roots.push(name.clone());
                }
                None => {
                    unlocked.insert(name.clone(), spec.clone());
                }
            }
        }

        if !unlocked.is_empty() {
            let resolved =
                resolve_install_set(&unlocked, &index, repo, &mut globally_resolved, &seed)?;
            for (name, r) in &resolved {
                globally_resolved.insert(name.clone(), (r.version.semver().clone(), r.repo.clone()));
            }
            to_install.extend(resolved);
        }

        if relock && !locked_roots.is_empty() {
            let fresh = carry_locked_closure(&locked_roots, &index, &seed, &mut globally_resolved);
            if !fresh.is_empty() {
                let resolved =
                    resolve_install_set(&fresh, &index, repo, &mut globally_resolved, &seed)?;
                for (name, r) in &resolved {
                    globally_resolved
                        .insert(name.clone(), (r.version.semver().clone(), r.repo.clone()));
                }
                to_install.extend(resolved);
            }
        }

        per_repo.insert(repo.clone(), RepoResolution { index, to_install });
    }

    Ok((per_repo, globally_resolved))
}

/// Walk the index's dependency lists from `roots` (packages the lock pins
/// that `baler.toml` still asks for), recording each reachable package
/// the lock also pins into `globally_resolved` at its pinned version.
/// Packages the lock doesn't pin that are reachable anyway come back as
/// `name -> constraint`, for the caller to resolve fresh.
///
/// The lock doesn't record which package depends on which, so the walk
/// uses the dependency lists in the current index. For a package pinned
/// to an older release those lists can differ a little from what that
/// release declared. The practical effect is limited to the edges: a
/// package only the old release needed may be dropped, and one only the
/// newer release needs may be added. Pinned versions never move.
fn carry_locked_closure(
    roots: &[String],
    index: &HashMap<String, PackageRecord>,
    seed: &Pins,
    globally_resolved: &mut Pins,
) -> BTreeMap<String, String> {
    let mut fresh: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();

    while let Some(pkg) = queue.pop_front() {
        if !visited.insert(pkg.clone()) {
            continue;
        }
        let Some(record) = index.get(&pkg) else {
            continue;
        };

        for (dep, dep_spec) in &record.deps {
            if visited.contains(dep) {
                continue;
            }
            if let Some(pin) = seed.get(dep) {
                globally_resolved.entry(dep.clone()).or_insert_with(|| pin.clone());
                queue.push_back(dep.clone());
            } else if globally_resolved.contains_key(dep) {
                queue.push_back(dep.clone());
            } else if index.contains_key(dep) {
                let spec = if VersionSpec::parse(dep_spec).is_ok() {
                    dep_spec.clone()
                } else {
                    "*".to_owned()
                };
                fresh.entry(dep.clone()).or_insert(spec);
            }
        }
    }

    fresh
}

/// Walk the dep graph breadth-first, validating version specs against the
/// index and collecting the full set of packages to install.
///
/// `seed` holds the lock's pins. A package whose pin is from this repo and
/// still satisfies every constraint that reaches it keeps that version
/// instead of jumping to the newest in the index; a pin that no longer
/// fits is resolved fresh.
fn resolve_install_set(
    requested: &BTreeMap<String, String>,
    index: &HashMap<String, PackageRecord>,
    repo_url: &str,
    globally_resolved: &mut Pins,
    seed: &Pins,
) -> Result<HashMap<String, ResolvedInstall>> {
    let direct: HashSet<String> = requested.keys().cloned().collect();

    let mut result: HashMap<String, ()> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut specs: HashMap<String, Vec<VersionSpec>> = HashMap::new();

    for (pkg, spec_str) in requested {
        specs.entry(pkg.clone()).or_default().push(VersionSpec::parse(spec_str)?);
        queue.push_back(pkg.clone());
    }

    while let Some(pkg) = queue.pop_front() {
        if visited.contains(&pkg) {
            continue;
        }
        visited.insert(pkg.clone());

        let record = match index.get(pkg.as_str()) {
            Some(r) => r,
            None => {
                eprintln!(" [warn] transitive dep '{}' not found in index, skipping", pkg);
                continue;
            }
        };

        result.insert(pkg.clone(), ());

        for (dep, dep_spec_str) in &record.deps {
            if let Ok(dep_spec) = VersionSpec::parse(dep_spec_str) {
                specs.entry(dep.clone()).or_default().push(dep_spec);
            }
            if !visited.contains(dep) {
                queue.push_back(dep.clone());
            }
        }
    }

    let mut resolved: HashMap<String, ResolvedInstall> = HashMap::new();

    for pkg in result.keys() {
        let record = &index[pkg];
        let pkg_specs = specs.get(pkg);

        // Already resolved by an earlier repo group in this same run (don't
        // re-resolve independently). Verify it still satisfies what
        // THIS repo's graph requires; reuse it if so, fail loudly if not.
        if let Some((existing_version, existing_repo)) = globally_resolved.get(pkg) {
            if let Some(pkg_specs) = pkg_specs {
                if VersionSpec::resolve(pkg_specs, std::slice::from_ref(existing_version)).is_none() {
                    bail!(
                        "Cross-repo version conflict for '{}': already resolved to {} via {}, \
                         but {} has constraints that version doesn't satisfy: {}",
                        pkg, existing_version, existing_repo, repo_url,
                        pkg_specs.iter().map(|s| format!("{}", s)).collect::<Vec<_>>().join(", ")
                    );
                }
            }

            // A package only reached here transitively (e.g. purrr pulling
            // in rlang) never claims it, since it's already owned by whichever
            // group resolved it as an actual direct dependency, and this
            // group's own to_install has no business installing it a
            // second time under the wrong repo. Only a group where pkg is
            // itself one of `requested` may correct a stale repo tag —
            // that's the one place baler.toml's own declaration for
            // this package lives.
            if direct.contains(pkg) {
                resolved.insert(pkg.clone(), ResolvedInstall {
                    version: RVersion::synthetic(existing_version.clone()),
                    repo: repo_url.to_owned(),
                });
            }
            continue;
        }

        // The lock already pinned this package. Keep the pin when it came
        // from this repo and still satisfies every constraint reaching it.
        if let Some((seed_version, seed_repo)) = seed.get(pkg) {
            let still_fits = seed_repo == repo_url
                && match pkg_specs {
                    Some(s) => VersionSpec::resolve(s, std::slice::from_ref(seed_version)).is_some(),
                    None => true,
                };
            if still_fits {
                resolved.insert(pkg.clone(), ResolvedInstall {
                    version: RVersion::synthetic(seed_version.clone()),
                    repo: repo_url.to_owned(),
                });
                continue;
            }
        }

        let pkg_specs = match pkg_specs {
            Some(s) => s,
            None => {
                resolved.insert(pkg.clone(), ResolvedInstall {
                    version: record.version.clone(),
                    repo: repo_url.to_owned(),
                });
                continue;
            }
        };

        if VersionSpec::resolve(pkg_specs, std::slice::from_ref(record.version.semver())).is_some() {
            resolved.insert(pkg.clone(), ResolvedInstall {
                version: record.version.clone(),
                repo: repo_url.to_owned(),
            });
            continue;
        }

        println!(" [checking] {} — index version doesn't satisfy constraints, searching archive...", pkg);
        let archive_versions = fetch_archive_versions(repo_url, pkg)
            .with_context(|| format!("fetching archive versions for '{}'", pkg))?;
        let archive_semvers: Vec<Version> = archive_versions.iter().map(|v| v.semver().clone()).collect();

        match VersionSpec::resolve(pkg_specs, &archive_semvers) {
            Some(matched_semver) => {
                let matched = archive_versions.iter()
                    .find(|v| v.semver() == matched_semver)
                    .expect("matched_semver came from archive_semvers, which is a 1:1 map of archive_versions");
                resolved.insert(pkg.clone(), ResolvedInstall {
                    version: matched.clone(),
                    repo: repo_url.to_owned(),
                });
            }
            None => {
                bail!(
                    "Version conflict for '{}': no version (including archive) satisfies all constraints.\n\
                     Constraints: {}",
                    pkg,
                    pkg_specs.iter().map(|s| format!("{}", s)).collect::<Vec<_>>().join(", ")
                );
            }
        }
    }

    Ok(resolved)
}
