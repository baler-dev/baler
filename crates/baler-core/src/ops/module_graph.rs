use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};

use crate::baler_toml::{BalerToml, ModuleDep, ModuleSource, PackageDep};
use crate::ops::resolve::{resolve_packages_from_specs, ResolvedPlan};
use crate::version::VersionSpec;

/// Fetches a module's baler.toml given the source (`git` or `url`)
/// declared in its dependent's `[dependencies.baler]` entry.
/// resolve_transitive() only knows how to walk the dependency graph.
/// It never touches the network itself, so who implements this
/// decides that policy: a real fetch over GitHub in production, a
/// HashMap in a test.
pub trait ModuleFetcher {
    fn fetch(&self, source: &ModuleSource) -> Result<BalerToml>;
}

/// Walk the full module dependency graph, starting from `root`,
/// fetching each declared box-module dep's baler.toml through `fetcher`
/// and folding its own dependencies into the same queue.
///
/// Two invariants that must hold from the first version of this
/// function, not added later: no infinite loop on a dependency cycle,
/// and no double-fetch of a module two different dependents both need.
///
/// This is DFS with an explicit ancestor path, not BFS with a queue,
/// deliberately. A "seen before" set alone can't distinguish a real
/// cycle (A depends on B depends on A) from a legitimate diamond (X
/// and Y both depend on the same Z): both look identical as "this name
/// showed up twice." Only the ancestor chain, who is currently
/// resolving whom — tells them apart. A queue has no notion of
/// ancestry; a call stack does, which is why this walks the graph
/// recursively with `path` instead of draining a VecDeque.
pub fn resolve_transitive(
    root: &BalerToml,
    fetcher: &dyn ModuleFetcher,
) -> Result<ResolvedPlan> {
    resolve_dependencies(
        &root.dependencies.packages,
        root.dependencies.baler.as_ref(),
        fetcher,
        false,
    )
}

/// `resolve_transitive` for dependency tables that are not a whole manifest.
///
/// With `skip_sourceless`, a module dependency that declares no source is
/// skipped with a warning instead of being an error. There is nothing to
/// fetch for it, so its own dependencies cannot be resolved. `baler lock`
/// uses this so that an old manifest does not stop locking.
pub fn resolve_dependencies(
    packages: &BTreeMap<String, PackageDep>,
    modules: Option<&BTreeMap<String, ModuleDep>>,
    fetcher: &dyn ModuleFetcher,
    skip_sourceless: bool,
) -> Result<ResolvedPlan> {
    let mut pkg_specs: BTreeMap<String, Vec<VersionSpec>> = BTreeMap::new();
    let mut pkg_repos: BTreeMap<String, String> = BTreeMap::new();

    for (name, dep) in packages {
        record_package(name, dep, &mut pkg_specs, &mut pkg_repos)?;
    }

    // name => (resolved version, source it came from). A module already
    // in here is finished. A second request for it is checked for
    // consistency instead of being fetched again. This is what makes a
    // diamond dependency cheap: the second path to reach it is a lookup,
    // not a re-fetch.
    let mut resolved_modules: BTreeMap<String, (semver::Version, ModuleSource)> = BTreeMap::new();

    // The chain of module names currently being resolved, root to leaf.
    // If resolving some module's deps leads back to a name already on
    // this path, that's a genuine cycle. The graph has no valid order
    // to fetch these in.
    let mut path: Vec<String> = Vec::new();

    for (name, dep) in modules.unwrap_or(&BTreeMap::new()) {
        resolve_module(
            name,
            dep,
            fetcher,
            &mut resolved_modules,
            &mut path,
            &mut pkg_specs,
            &mut pkg_repos,
            skip_sourceless,
        )?;
    }

    let packages = resolve_packages_from_specs(pkg_specs, &pkg_repos)?;
    let modules = resolved_modules
        .into_iter()
        .map(|(name, (version, _source))| (name, version.to_string()))
        .collect();

    Ok(ResolvedPlan { packages, modules })
}

/// Resolve one box-module dep and recurse into its own dependencies.
/// Package deps discovered along the way are folded into `pkg_specs`/
/// `pkg_repos` for the caller to finalize once the whole graph is walked.
fn resolve_module(
    name: &str,
    dep: &ModuleDep,
    fetcher: &dyn ModuleFetcher,
    resolved_modules: &mut BTreeMap<String, (semver::Version, ModuleSource)>,
    path: &mut Vec<String>,
    pkg_specs: &mut BTreeMap<String, Vec<VersionSpec>>,
    pkg_repos: &mut BTreeMap<String, String>,
    skip_sourceless: bool,
) -> Result<()> {
    if skip_sourceless
        && dep
            .source()
            .with_context(|| format!("Invalid source for module '{name}'"))?
            .is_none()
    {
        eprintln!(
            "  [warn] module '{name}' declares no source, so its own dependencies are not resolved"
        );
        return Ok(());
    }

    if let Some((existing_version, existing_source)) = resolved_modules.get(name) {
        let source = declared_source(name, dep)?;
        if &source != existing_source {
            bail!(
                "'{name}' is required from two different sources: '{}' and '{}' — \
                 baler doesn't merge these.",
                existing_source, source
            );
        }
        let spec = VersionSpec::parse(dep.version())?;
        if !spec.matches(existing_version) {
            bail!(
                "'{name}' has conflicting version constraints: already resolved to {} \
                 but this dependent requires {}.",
                existing_version, spec
            );
        }
        return Ok(());
    }

    if path.iter().any(|ancestor| ancestor == name) {
        let mut chain = path.clone();
        chain.push(name.to_owned());
        bail!("Dependency cycle detected: {}", chain.join(" -> "));
    }

    let source = declared_source(name, dep)?;

    let fetched = fetcher
        .fetch(&source)
        .with_context(|| format!("Failed to fetch module '{name}' from '{source}'"))?;

    // Reuses ModuleMeta::semver() (the same check `from_dir` runs on a
    // local baler.toml) instead of parsing `version` a second way here,
    // with its own context layered on for which module/source it was.
    let fetched_version = fetched
        .project
        .semver()
        .with_context(|| format!("Module '{name}' at '{source}' has an invalid version"))?;

    let spec = VersionSpec::parse(dep.version())?;
    if !spec.matches(&fetched_version) {
        bail!(
            "'{name}' requires version {spec} but the source at '{source}' declares {fetched_version}."
        );
    }

    for (pkg_name, pkg_dep) in &fetched.dependencies.packages {
        record_package(pkg_name, pkg_dep, pkg_specs, pkg_repos)?;
    }

    path.push(name.to_owned());
    for (dep_name, dep_dep) in fetched.dependencies.baler.unwrap_or_default() {
        resolve_module(
            &dep_name,
            &dep_dep,
            fetcher,
            resolved_modules,
            path,
            pkg_specs,
            pkg_repos,
            skip_sourceless,
        )?;
    }
    path.pop();

    resolved_modules.insert(name.to_owned(), (fetched_version, source));
    Ok(())
}

/// Fold one package dependency into the accumulated specs. A package is
/// installed from exactly one repo, so two dependents that each name a
/// repository for it must name the same one: anything else is an error, never
/// a silent last-writer-wins that depends on the order the graph was walked
/// in. A dependent that names no repository does not care, and so never
/// clashes with one that does. Repo URLs are compared without a trailing
/// slash.
fn record_package(
    name: &str,
    dep: &PackageDep,
    pkg_specs: &mut BTreeMap<String, Vec<VersionSpec>>,
    pkg_repos: &mut BTreeMap<String, String>,
) -> Result<()> {
    if let Some(repo) = dep.explicit_repo() {
        if let Some(existing) = pkg_repos.get(name) {
            if existing.trim_end_matches('/') != repo.trim_end_matches('/') {
                bail!(
                    "Package '{name}' is requested from two repos: {existing} and {repo}. \
                     Make every dependent that names a repo use the same one."
                );
            }
        }
        pkg_repos.insert(name.to_owned(), repo.to_owned());
    }
    let spec = VersionSpec::parse(dep.version())?;
    pkg_specs.entry(name.to_owned()).or_default().push(spec);
    Ok(())
}

/// The declared source of a module dep, or an error saying what to
/// write. There is no default module registry, so a dep without a
/// `git` or `url` has nowhere to be fetched from.
fn declared_source(name: &str, dep: &ModuleDep) -> Result<ModuleSource> {
    dep.source()
        .with_context(|| format!("Invalid source for module '{name}'"))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "'{name}' has no source declared, and baler has no default module registry.\n\
                 Declare it as: {name} = {{ git = \"https://github.com/user/repo\", tag = \"...\" }}"
            )
        })
}

/// A module that is not installed yet, and where to install it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInstall {
    pub name: String,
    pub source: ModuleSource,
    pub version: semver::Version,
}

/// Which of `modules` have to be installed, and from where.
///
/// A module that is already installed is left alone and nothing is fetched
/// for it. For the others, the source is fetched and checked before anything
/// is installed: it must be the module the entry names, and the version it
/// declares must satisfy the entry's constraint. A module that is missing and
/// declares no source cannot be installed, so it is reported and skipped.
pub fn plan_module_installs(
    modules: &BTreeMap<String, ModuleDep>,
    fetcher: &dyn ModuleFetcher,
    is_installed: &dyn Fn(&str) -> Result<bool>,
) -> Result<Vec<ModuleInstall>> {
    let mut planned = Vec::new();
    for (name, dep) in modules {
        if is_installed(name)? {
            continue;
        }
        let Some(source) = dep
            .source()
            .with_context(|| format!("Invalid source for module '{name}'"))?
        else {
            eprintln!(
                "  [warn] module '{name}' is not installed and declares no source, so baler \
                 cannot install it. Declare it as: {name} = {{ git = \"https://github.com/user/repo\", tag = \"...\" }}"
            );
            continue;
        };

        let fetched = fetcher
            .fetch(&source)
            .with_context(|| format!("Failed to fetch module '{name}' from '{source}'"))?;
        if fetched.project.name != *name {
            bail!(
                "'{name}' is declared with the source '{source}', but that source is the \
                 module '{}'. The dependency key must be the module's own name.",
                fetched.project.name
            );
        }
        let version = fetched
            .project
            .semver()
            .with_context(|| format!("Module '{name}' at '{source}' has an invalid version"))?;
        let spec = VersionSpec::parse(dep.version())?;
        if !spec.matches(&version) {
            bail!("'{name}' requires version {spec} but the source at '{source}' declares {version}.");
        }
        planned.push(ModuleInstall { name: name.clone(), source, version });
    }
    Ok(planned)
}
