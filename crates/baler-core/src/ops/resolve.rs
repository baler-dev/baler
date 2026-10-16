use std::collections::BTreeMap;
use anyhow::{bail, Result};
use semver::Version;

use crate::baler_toml::{BalerToml, ModuleDep, PackageDep, DEFAULT_CRAN_MIRROR};
use crate::lockfile::{BalerLock, LockGroup};
use crate::version::VersionSpec;
use crate::paths::{resolve_install_dir, resolve_r_lib_dir};

/// A resolved package dep — version spec plus the repo it comes from.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub version_spec: String,
    pub repo: String,
}

#[derive(Debug)]
pub struct ResolvedPlan {
    /// package name => resolved package (spec + repo)
    pub packages: BTreeMap<String, ResolvedPackage>,
    /// module name => version spec
    pub modules: BTreeMap<String, String>,
}

/// Resolve only the root project's own package_deps/module_deps, one
/// level deep — no fetching, no transitive walk. Used where a full
/// dependency graph isn't needed (e.g. resolving the packages a
/// just-unpacked module itself declares). For the full graph, see
/// resolve_transitive().
pub fn resolve(
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    module_deps: &Option<BTreeMap<String, ModuleDep>>,
) -> Result<ResolvedPlan> {
    let mut pkg_specs: BTreeMap<String, Vec<VersionSpec>> = BTreeMap::new();
    let mut pkg_repos: BTreeMap<String, String> = BTreeMap::new();
    let mut mod_specs: BTreeMap<String, Vec<VersionSpec>> = BTreeMap::new();

    for (name, dep) in package_deps.as_ref().unwrap_or(&BTreeMap::new()) {
        let spec = VersionSpec::parse(dep.version())?;
        pkg_specs.entry(name.clone()).or_default().push(spec);
        // Last writer wins for repo
        // Fine since duplicate deps are unusual
        pkg_repos.insert(name.clone(), dep.repo().to_owned());
    }

    for (name, dep) in module_deps.as_ref().unwrap_or(&BTreeMap::new()) {
        let spec = VersionSpec::parse(dep.version())?;
        mod_specs.entry(name.clone()).or_default().push(spec);

        // TODO: dep.source() is parsed but not yet used. Once transitive
        // module resolution exists, fetch the source's baler.toml,
        // read its declared module.version, check it against `spec`,
        // and push its own deps onto the queue.
    }

    let packages = resolve_packages_from_specs(pkg_specs, &pkg_repos)?;

    let modules = mod_specs
        .into_keys()
        .map(|n| (n, "latest".to_owned()))
        .collect();

    Ok(ResolvedPlan { packages, modules })
}

/// Offline check of one dependency group in `baler.toml` against the lock.
/// Returns one line per problem, empty when the lock covers every declared
/// package in that group: each must be pinned, from the same repo, at a
/// version that satisfies the constraint written in `baler.toml`.
///
/// This is deliberately all it checks. It doesn't touch the network and it
/// can't tell whether the lock holds packages `baler.toml` no longer asks
/// for, because the lock doesn't record which package pulled in which.
/// Those are cleaned up the next time `baler lock` runs.
fn lock_problems(
    lock: &BalerLock,
    group: LockGroup,
    declared: &BTreeMap<String, PackageDep>,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();

    for (name, dep) in declared {
        let Some(locked) = lock
            .packages
            .iter()
            .find(|locked| &locked.name == name && locked.in_group(group))
        else {
            let which = if group == LockGroup::Runtime {
                String::new()
            } else {
                format!(" ({})", group.as_str())
            };
            problems.push(format!(
                "'{name}' is declared in baler.toml{which} but missing from baler.lock"
            ));
            continue;
        };

        if dep.repo() != locked.repo {
            problems.push(format!(
                "'{name}' repo changed in baler.toml ({} -> {}) but baler.lock \
                 still pins the old one",
                locked.repo,
                dep.repo()
            ));
            continue;
        }

        let required = VersionSpec::parse(dep.version())?;
        match Version::parse(&locked.version) {
            Ok(pinned) if required.matches(&pinned) => {}
            Ok(pinned) => problems.push(format!(
                "'{name}' is pinned to {pinned} in baler.lock, but baler.toml \
                 requires '{}'",
                dep.version()
            )),
            Err(_) => problems.push(format!(
                "baler.lock has an invalid version for '{name}': '{}'",
                locked.version
            )),
        }
    }

    Ok(problems)
}

/// `lock_problems` over every group `baler.toml` declares packages for:
/// runtime, `[compiled-code].build_deps`, and `[extras]`. Used before
/// bundling, where a lock that no longer matches the manifest would be
/// shipped to everyone who installs the bundle.
pub fn lock_problems_for(toml: &BalerToml, lock: &BalerLock) -> Result<Vec<String>> {
    let mut problems = lock_problems(lock, LockGroup::Runtime, &toml.dependencies.packages)?;
    if let Some(build) = toml.compiled_code.as_ref().and_then(|c| c.build_deps.as_ref()) {
        problems.extend(lock_problems(lock, LockGroup::Build, build)?);
    }
    if let Some(extras) = toml.extras.as_ref() {
        problems.extend(lock_problems(lock, LockGroup::Extras, &extras.packages)?);
    }
    Ok(problems)
}

/// Build a plan straight from an existing lock instead of walking
/// package_deps. A lock already holds the full transitive closure that
/// `baler lock` resolved — cli, lifecycle, magrittr, vctrs and
/// everything else a direct dep like purrr pulls in, not just the names
/// baler.toml declares directly. resolve()'s one-level walk can't see
/// past that first level, which is exactly what leaves those extra
/// packages unrequested and uninstalled when a lock is present.
///
/// Every name still declared in package_deps keeps its own constraint
/// string (so cran::client's own lock-vs-constraint check still catches
/// a genuinely stale lock). A name that's only in the lock — a
/// transitive dep never declared in baler.toml — gets "*", since
/// nothing in baler.toml constrains it and the lock is the only
/// authority on what it should resolve to.
///
/// An install never rewrites the lock. When the lock doesn't cover what
/// baler.toml declares, this fails and points at `baler lock`, which keeps
/// every pin that is still valid and only adds, drops or re-resolves what
/// changed. `baler lock --update` is for deliberately moving pins.
pub fn resolve_locked(
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    module_deps: &Option<BTreeMap<String, ModuleDep>>,
    lock: &BalerLock,
) -> Result<ResolvedPlan> {
    locked_plan(package_deps, module_deps, None, &None, lock)
}

/// `resolve_locked` that also installs the lock's `extras` group, and
/// checks `[extras]` against the lock the same way runtime packages are.
/// Backs `baler install --extras`.
pub fn resolve_locked_with_extras(
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    module_deps: &Option<BTreeMap<String, ModuleDep>>,
    extra_packages: &BTreeMap<String, PackageDep>,
    extra_modules: &Option<BTreeMap<String, ModuleDep>>,
    lock: &BalerLock,
) -> Result<ResolvedPlan> {
    locked_plan(package_deps, module_deps, Some(extra_packages), extra_modules, lock)
}

/// Shared body of `resolve_locked` and `resolve_locked_with_extras`.
/// `extra_packages` is `Some` only when the caller asked for extras.
fn locked_plan(
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    module_deps: &Option<BTreeMap<String, ModuleDep>>,
    extra_packages: Option<&BTreeMap<String, PackageDep>>,
    extra_modules: &Option<BTreeMap<String, ModuleDep>>,
    lock: &BalerLock,
) -> Result<ResolvedPlan> {
    let empty = BTreeMap::new();
    let declared = package_deps.as_ref().unwrap_or(&empty);

    let mut problems = lock_problems(lock, LockGroup::Runtime, declared)?;
    if let Some(extra) = extra_packages {
        problems.extend(lock_problems(lock, LockGroup::Extras, extra)?);
    }
    if !problems.is_empty() {
        bail!(
            "baler.lock is out of date with baler.toml:\n  - {}\n\
             The lock is stale. Run `baler lock` and commit the result.",
            problems.join("\n  - ")
        );
    }

    // Build pins stay in the lock but are not installed here, so an install
    // never pulls in compile-time packages. Extras pins are installed only
    // when the caller asked for them, so a normal install never pulls in
    // dev-only packages either.
    let with_extras = extra_packages.is_some();
    let packages = lock
        .packages
        .iter()
        .filter(|locked| locked.is_runtime() || (with_extras && locked.in_group(LockGroup::Extras)))
        .map(|locked| {
            let dep = declared
                .get(&locked.name)
                .or_else(|| extra_packages.and_then(|extra| extra.get(&locked.name)));
            let version_spec = match dep {
                Some(dep) => dep.version().to_owned(),
                None => "*".to_owned(),
            };
            let pinned = ResolvedPackage { version_spec, repo: locked.repo.clone() };
            (locked.name.clone(), pinned)
        })
        .collect();

    let mut mod_specs: BTreeMap<String, Vec<VersionSpec>> = BTreeMap::new();
    for (name, dep) in module_deps.iter().flatten().chain(extra_modules.iter().flatten()) {
        let spec = VersionSpec::parse(dep.version())?;
        mod_specs.entry(name.clone()).or_default().push(spec);
    }
    let modules = mod_specs
        .into_keys()
        .map(|name| (name, "latest".to_owned()))
        .collect();

    Ok(ResolvedPlan { packages, modules })
}

/// `resolve` over the root's runtime deps and its `[extras]` together.
/// The plan holds one version spec and one repo per name, so a name
/// declared in both groups must agree in both. A mismatch fails here
/// instead of one group silently overriding the other. Modules are
/// compared by version only, since a module's source isn't resolved yet.
pub fn resolve_with_extras(
    package_deps: &Option<BTreeMap<String, PackageDep>>,
    module_deps: &Option<BTreeMap<String, ModuleDep>>,
    extra_packages: &BTreeMap<String, PackageDep>,
    extra_modules: &Option<BTreeMap<String, ModuleDep>>,
) -> Result<ResolvedPlan> {
    let mut packages = package_deps.clone().unwrap_or_default();
    for (name, dep) in extra_packages {
        match packages.get(name) {
            Some(runtime) if runtime.version() == dep.version() && runtime.repo() == dep.repo() => {}
            Some(runtime) => bail!(
                "'{name}' is in both [dependencies] ('{}' from {}) and [extras] ('{}' from {}). \
                 Make them match.",
                runtime.version(),
                runtime.repo(),
                dep.version(),
                dep.repo()
            ),
            None => {
                packages.insert(name.clone(), dep.clone());
            }
        }
    }

    let mut modules = module_deps.clone().unwrap_or_default();
    for (name, dep) in extra_modules.iter().flatten() {
        match modules.get(name) {
            Some(runtime) if runtime.version() == dep.version() => {}
            Some(runtime) => bail!(
                "Module '{name}' is in both [dependencies.baler] ('{}') and [extras.baler] ('{}'). \
                 Make them match.",
                runtime.version(),
                dep.version()
            ),
            None => {
                modules.insert(name.clone(), dep.clone());
            }
        }
    }

    resolve(&Some(packages), &Some(modules))
}

/// Shared by resolve() and module_graph::resolve_transitive() — collapses
/// each package's accumulated version specs down to one, erroring on any
/// real conflict rather than silently picking one. pub(crate) so
/// module_graph.rs can call it without duplicating the logic.
pub(crate) fn resolve_packages_from_specs(
    pkg_specs: BTreeMap<String, Vec<VersionSpec>>,
    pkg_repos: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ResolvedPackage>> {
    let mut packages = BTreeMap::new();
    for (name, specs) in pkg_specs {
        let version_spec = match specs.as_slice() {
            [] => "*".to_owned(),
            [only] => format!("{only}"),
            multiple => {
                let constraints: Vec<String> = multiple.iter().map(|s| s.to_string()).collect();
                bail!(
                    "'{name}' has {} conflicting version constraints ({}) — \
                     baler doesn't merge these yet.",
                    multiple.len(),
                    constraints.join(", ")
                );
            }
        };
        let repo = pkg_repos
            .get(&name)
            .cloned()
            .unwrap_or_else(|| DEFAULT_CRAN_MIRROR.to_owned());
        packages.insert(name, ResolvedPackage { version_spec, repo });
    }
    Ok(packages)
}

/// Pretty-print the resolved plan to stdout.
pub fn print_plan(plan: &ResolvedPlan) {
    if plan.packages.is_empty() && plan.modules.is_empty() {
        println!("  No dependencies.");
        return;
    }
    if !plan.packages.is_empty() {
        println!("  R packages:");
        for (name, pkg) in &plan.packages {
            if pkg.repo == DEFAULT_CRAN_MIRROR {
                println!("    {} ({})", name, pkg.version_spec);
            } else {
                println!("    {} ({}) [{}]", name, pkg.version_spec, pkg.repo);
            }
        }
    }
    if !plan.modules.is_empty() {
        println!("  baler modules:");
        for (name, ver) in &plan.modules {
            println!("    {} ({})", name, ver);
        }
    }
}

pub fn already_installed_module(name: &str) -> Result<bool> {
    let install_dir = resolve_install_dir()?;
    Ok(install_dir.join(name).is_dir())
}

/// Resolve a plan's R packages to exact versions and repos without
/// installing anything — what `baler lock` calls. Module deps aren't
/// included: there's no automatic resolve+install path for them yet
/// (see the TODO in `resolve()` above), so there's nothing concrete to
/// pin for a module the same way there is for a package.
///
/// `lock` seeds the resolution rather than being enforced: pins that still
/// fit `baler.toml` are kept, along with the transitive packages they
/// reach, and anything that no longer fits is resolved again. See
/// `cran::client::resolve_packages`.
pub fn resolve_only(
    plan: &ResolvedPlan,
    lock: Option<&BalerLock>,
) -> Result<std::collections::HashMap<String, (semver::Version, String)>> {
    if plan.packages.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    crate::cran::client::resolve_packages(&plan.packages, lock)
}

/// Runs the plan. On a real (non-dry-run) install, returns everything
/// that was resolved for R packages (direct and transitive). `install`
/// discards it: the lock is only ever written by `baler lock`. A dry run
/// or a plan with no packages returns an empty map.
pub fn execute_plan(
    plan: &ResolvedPlan,
    dry_run: bool,
    lock: Option<&BalerLock>,
) -> Result<std::collections::HashMap<String, (semver::Version, String)>> {
    let mut resolved = std::collections::HashMap::new();

    if !plan.packages.is_empty() {
        if dry_run {
            println!("  Would install R packages (pass --install-deps to proceed):");
            for (name, pkg) in &plan.packages {
                println!("    {} ({})", name, pkg.version_spec);
            }
        } else {
            let r_lib = resolve_r_lib_dir()?;
            println!("  Installing R packages into {}...", r_lib.display());
            resolved = crate::cran::client::install_packages(&plan.packages, &r_lib, lock)?;
        }
    }

    for (name, _spec) in &plan.modules {
        if already_installed_module(name)? {
            println!("  [ok] {} (already installed)", name);
        } else {
            println!(
                "  [missing] {} — install with: baler install <path or gh:user/repo>",
                name
            );
        }
    }

    Ok(resolved)
}
