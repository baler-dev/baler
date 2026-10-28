use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::cran::packages::{is_base_package, RVersion};
use crate::cran::universe::{CranUniverse, Sources};
use crate::lockfile::BalerLock;
use crate::ops::resolve::ResolvedPackage;
use crate::resolver::{dependency_order, solve, Node, Preferred, Universe};
use crate::ver::{Ver, VerReq};

/// `{name: (exact version, repo)}`.
type Pins = HashMap<String, (Version, String)>;

/// One package to install: exactly which release, from which repository.
pub struct Planned {
    pub name: String,
    pub version: RVersion,
    pub repo: String,
}

/// A constraint from baler.toml (or the merge of several), already
/// flattened into a string by `resolve_packages_from_specs`.
fn requested_req(package: &str, text: &str) -> Result<VerReq> {
    VerReq::parse(text)
        .with_context(|| format!("Invalid version constraint for '{package}' in baler.toml"))
}

/// The R version a lock pin stands for. A pin that is not an R version at
/// all is an error instead of a guess.
fn pin_ver(name: &str, pin: &Version) -> Result<Ver> {
    Ver::from_semver(pin).with_context(|| {
        format!("baler.lock pins '{name}' to {pin}, which is not an R package version")
    })
}

/// Resolve every package (direct and transitive) needed to satisfy
/// `packages`, without downloading or installing anything, and return them
/// in install order (dependencies first). Shared by `install_packages`
/// (resolve, then install) and `resolve_packages()` (what `baler lock` calls).
///
/// The packages are solved together, as one graph, by `crate::resolver`.
/// Each candidate version is judged by its own dependencies, and a choice
/// that leads to a conflict is undone and the next one tried. Where a
/// package comes from is decided by `CranUniverse`.
///
/// If `lock` is `Some`, the pins in it are kept as long as they still fit:
///
/// - A directly requested package whose pin still fits `baler.toml` is not
///   solved at all when installing (`relock == false`). It is installed at
///   exactly the pinned version, and the solver is told it has no
///   dependencies, since the lock already lists everything it needs. A
///   package the lock doesn't mention is still solved, the same way it
///   would be with no lock, and may not move a pinned package.
/// - A transitive package prefers its pin, when it still satisfies every
///   constraint that reaches it, so adding one dependency doesn't move the
///   versions of packages already locked.
///
/// `relock` is the one difference between the two callers:
///
/// - `false` (install): the lock is authoritative. A pin that no longer
///   fits `baler.toml` is an error, and nothing beyond what was requested
///   is added.
/// - `true` (`baler lock`): the lock is a starting point. Every requested
///   package is solved again, preferring its pin, so a pin that no longer
///   fits (its repo moved, or the constraint tightened past the pinned
///   version) is re-resolved instead of failing, and the lock keeps its
///   full transitive closure. Each pinned package's own dependencies are
///   read, so the closure is the one that release really needs.
///
/// A version a lock pins is recovered as the repository publishes it, so a
/// lock that wrote `0.3.1` for `0.3-1` still downloads the right file.
pub(super) fn resolve_all(
    packages: &BTreeMap<String, ResolvedPackage>,
    lock: Option<&BalerLock>,
    relock: bool,
) -> Result<Vec<Planned>> {
    resolve_all_with(packages, lock, relock, &Sources::network())
}

pub(super) fn resolve_all_with(
    packages: &BTreeMap<String, ResolvedPackage>,
    lock: Option<&BalerLock>,
    relock: bool,
    sources: &Sources<'_>,
) -> Result<Vec<Planned>> {
    let seed: Pins = match lock {
        Some(l) => l.pins()?,
        None => HashMap::new(),
    };

    // Packages that ship with R are on no index and need no installing.
    let wanted: BTreeMap<&String, &ResolvedPackage> = packages
        .iter()
        .filter(|(name, _)| !is_base_package(name))
        .collect();

    // What the lock fixes (install only) and what is left for the solver.
    let mut fixed: BTreeMap<String, Version> = BTreeMap::new();
    let mut roots: BTreeMap<String, String> = BTreeMap::new();

    for (name, pkg) in &wanted {
        let repo = &pkg.repo;
        let spec = &pkg.version_spec;
        let locked = match lock {
            Some(l) => l.locked_version(name)?,
            None => None,
        };
        let Some((version, locked_repo)) = locked else {
            roots.insert((*name).clone(), spec.clone());
            continue;
        };

        if locked_repo != *repo {
            if relock {
                println!(
                    " [relock] '{name}' moved from {locked_repo} to {repo} in \
                     baler.toml, resolving it again"
                );
                roots.insert((*name).clone(), spec.clone());
                continue;
            }
            bail!(
                "baler.lock pins '{name}' to repo {locked_repo}, but baler.toml \
                 now points at {repo}. Run `baler lock` to update the lock, or \
                 revert baler.toml's repo for this package."
            );
        }

        let required = requested_req(name, spec)?;
        if !required.matches(&pin_ver(name, &version)?) {
            if relock {
                println!(
                    " [relock] '{name}' is pinned to {version}, but baler.toml now \
                     requires '{spec}', resolving it again"
                );
                roots.insert((*name).clone(), spec.clone());
                continue;
            }
            bail!(
                "baler.lock pins '{name}' to {version}, but baler.toml now \
                 requires '{spec}', so the lock is stale. Run `baler lock` to \
                 update it."
            );
        }

        if relock {
            roots.insert((*name).clone(), spec.clone());
        } else {
            fixed.insert((*name).clone(), version);
        }
    }

    let declared: HashMap<String, String> = wanted
        .iter()
        .map(|(name, pkg)| ((*name).clone(), pkg.repo.clone()))
        .collect();
    let universe = CranUniverse::new(sources, declared, fixed.keys().cloned().collect());

    let mut requested: Vec<(Node, VerReq)> = Vec::new();
    for (name, spec) in &roots {
        let node = Node::Cran(name.clone());
        if universe.current(&node)?.is_none() {
            let repo = &wanted[name].repo;
            bail!("'{name}' is not in the package index of {repo}");
        }
        requested.push((node, requested_req(name, spec)?));
    }
    for (name, version) in &fixed {
        let pinned = pin_ver(name, version)?;
        requested.push((Node::Cran(name.clone()), VerReq::exactly(&pinned)));
    }
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    let mut preferred = Preferred::new();
    for (name, (version, repo)) in &seed {
        if universe.repo_of(name)?.as_deref() == Some(repo.as_str()) {
            preferred.insert(Node::Cran(name.clone()), pin_ver(name, version)?);
        }
    }

    let solution = solve(&universe, &requested, &preferred)?;

    // The lock has no edges yet, so a fixed package is ordered by what the
    // current index says it depends on.
    let mut edges = solution.edges.clone();
    for name in fixed.keys() {
        let dependencies = universe
            .index_dependency_names(name)?
            .into_iter()
            .map(Node::Cran)
            .collect::<BTreeSet<_>>();
        edges.insert(Node::Cran(name.clone()), dependencies);
    }

    let mut planned = Vec::with_capacity(solution.versions.len());
    for node in dependency_order(solution.versions.keys(), &edges) {
        let Node::Cran(name) = &node else { continue };
        let (version, repo) = universe.release(name, &solution.versions[&node]).with_context(|| {
            format!(
                "baler.lock or the solver chose {name} {}, but that release is not available. \
                 Run `baler lock --update {name}`.",
                solution.versions[&node]
            )
        })?;
        planned.push(Planned { name: name.clone(), version, repo });
    }

    Ok(planned)
}
