//! The CRAN side of the solver's `Universe`: every repository a project
//! uses, each looked up lazily, plus each repository's Archive/ for older
//! releases.
//!
//! Which repository a package comes from is decided in one place, so the
//! same package never resolves differently depending on who asked for it:
//!
//! 1. A package the project declares directly comes from the repository it
//!    declares, always.
//! 2. Any other package (a transitive dependency) is searched for in the
//!    default CRAN first, then in the declared repositories in alphabetical
//!    order, and comes from the first one that has it. A private repository
//!    therefore cannot take over a CRAN package name for a dependency nobody
//!    declared. To use the private copy, declare it with `repo`.
//! 3. A dependency found in none of them is assumed to be installed already
//!    (a Bioconductor package, say), with a warning, as it always has been.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use anyhow::{bail, Context, Result};

use crate::baler_toml::DEFAULT_CRAN_MIRROR;
use crate::cran::packages::{
    fetch, fetch_archive_deps, fetch_archive_versions, PackageRecord, RVersion,
};
use crate::resolver::{Node, Universe};
use crate::ver::{Ver, VerReq};

pub type Index = HashMap<String, PackageRecord>;

/// Everything that needs the network, so tests can run without one.
pub struct Sources<'a> {
    /// The package index of a repository.
    pub index: &'a dyn Fn(&str) -> Result<Index>,
    /// The archived releases of a package: `(repo, package)`.
    pub archive_versions: &'a dyn Fn(&str, &str) -> Result<Vec<RVersion>>,
    /// The dependencies of one archived release: `(repo, package, release)`.
    pub archive_deps: &'a dyn Fn(&str, &str, &RVersion) -> Result<Vec<(String, String)>>,
}

impl Sources<'static> {
    pub fn network() -> Self {
        Self {
            index: &fetch,
            archive_versions: &fetch_archive_versions,
            archive_deps: &fetch_archive_deps,
        }
    }
}

pub struct CranUniverse<'a> {
    sources: &'a Sources<'a>,
    declared: HashMap<String, String>,
    search: Vec<String>,
    leaves: HashSet<String>,
    indexes: RefCell<HashMap<String, Rc<Index>>>,
    located: RefCell<HashMap<String, Option<String>>>,
    archived: RefCell<HashMap<(String, String), Vec<RVersion>>>,
    warned: RefCell<HashSet<String>>,
}

fn same_repo(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

impl<'a> CranUniverse<'a> {
    /// `declared` maps each directly requested package to its repository.
    /// `leaves` are packages fixed by a lock: the solver is told they have no
    /// dependencies, because the lock already lists everything they need.
    pub fn new(
        sources: &'a Sources<'a>,
        declared: HashMap<String, String>,
        leaves: HashSet<String>,
    ) -> Self {
        let mut others: Vec<String> = declared.values().cloned().collect();
        others.sort();
        let mut search = vec![DEFAULT_CRAN_MIRROR.to_owned()];
        for repo in others {
            if !search.iter().any(|known| same_repo(known, &repo)) {
                search.push(repo);
            }
        }
        Self {
            sources,
            declared,
            search,
            leaves,
            indexes: RefCell::default(),
            located: RefCell::default(),
            archived: RefCell::default(),
            warned: RefCell::default(),
        }
    }

    fn index_of(&self, repo: &str) -> Result<Rc<Index>> {
        if let Some(known) = self.indexes.borrow().get(repo) {
            return Ok(known.clone());
        }
        println!("Fetching package index from {repo}...");
        let fetched = Rc::new((self.sources.index)(repo)?);
        self.indexes.borrow_mut().insert(repo.to_owned(), fetched.clone());
        Ok(fetched)
    }

    /// The repository `name` comes from, or `None` when no repository has it.
    pub fn repo_of(&self, name: &str) -> Result<Option<String>> {
        if let Some(known) = self.located.borrow().get(name) {
            return Ok(known.clone());
        }
        let repo = match self.declared.get(name) {
            Some(declared) => Some(declared.clone()),
            None => {
                let mut found = None;
                for repo in &self.search {
                    if self.index_of(repo)?.contains_key(name) {
                        found = Some(repo.clone());
                        break;
                    }
                }
                found
            }
        };
        self.located.borrow_mut().insert(name.to_owned(), repo.clone());
        Ok(repo)
    }

    fn archived_releases(&self, repo: &str, name: &str) -> Result<Vec<RVersion>> {
        let key = (repo.to_owned(), name.to_owned());
        if let Some(known) = self.archived.borrow().get(&key) {
            return Ok(known.clone());
        }
        println!(" [checking] {name}: searching the archive of {repo}...");
        let listed = (self.sources.archive_versions)(repo, name)
            .with_context(|| format!("Failed to list the archived releases of '{name}'"))?;
        self.archived.borrow_mut().insert(key, listed.clone());
        Ok(listed)
    }

    /// The release of `name` with this version, spelled the way the
    /// repository publishes it (`0.3-1`, not `0.3.1`), and the repository it
    /// comes from. This is what turns a version read from a lock, which may
    /// have lost its original spelling, back into something that can be
    /// downloaded.
    pub fn release(&self, name: &str, version: &Ver) -> Result<(RVersion, String)> {
        let Some(repo) = self.repo_of(name)? else {
            bail!("'{name}' is not in any package index");
        };
        if let Some(record) = self.index_of(&repo)?.get(name) {
            if record.version.ver() == version {
                return Ok((record.version.clone(), repo));
            }
        }
        match self
            .archived_releases(&repo, name)?
            .into_iter()
            .find(|release| release.ver() == version)
        {
            Some(release) => Ok((release, repo)),
            None => bail!("{name} {version} is neither current nor in the archive of {repo}"),
        }
    }

    /// Names the newest release of `name` depends on, as the index lists them.
    pub fn index_dependency_names(&self, name: &str) -> Result<Vec<String>> {
        let Some(repo) = self.repo_of(name)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .index_of(&repo)?
            .get(name)
            .map(|record| record.deps.iter().map(|(dep, _)| dep.clone()).collect())
            .unwrap_or_default())
    }

    fn warn_unavailable(&self, package: &str, dependency: &str) {
        if self.warned.borrow_mut().insert(dependency.to_owned()) {
            eprintln!(
                "  [warn] '{dependency}' (needed by {package}) is in no package index, \
                 assuming it is already installed"
            );
        }
    }
}

fn cran_name(node: &Node) -> Result<&str> {
    match node {
        Node::Cran(name) => Ok(name),
        other => bail!("{other} is not a CRAN package"),
    }
}

impl Universe for CranUniverse<'_> {
    fn current(&self, node: &Node) -> Result<Option<Ver>> {
        let name = cran_name(node)?;
        let Some(repo) = self.repo_of(name)? else {
            return Ok(None);
        };
        Ok(self.index_of(&repo)?.get(name).map(|record| record.version.ver().clone()))
    }

    fn older(&self, node: &Node) -> Result<Vec<Ver>> {
        let name = cran_name(node)?;
        let Some(repo) = self.repo_of(name)? else {
            return Ok(Vec::new());
        };
        let index = self.index_of(&repo)?;
        let newest = index.get(name).map(|record| record.version.ver());
        Ok(self
            .archived_releases(&repo, name)?
            .iter()
            .map(|release| release.ver().clone())
            .filter(|ver| Some(ver) != newest)
            .collect())
    }

    fn dependencies(&self, node: &Node, version: &Ver) -> Result<Vec<(Node, VerReq)>> {
        let name = cran_name(node)?;
        if self.leaves.contains(name) {
            return Ok(Vec::new());
        }
        let Some(repo) = self.repo_of(name)? else {
            bail!("'{name}' is not in any package index");
        };
        let index = self.index_of(&repo)?;
        let Some(record) = index.get(name) else {
            bail!("'{name}' is not in the package index of {repo}");
        };

        let declared = if record.version.ver() == version {
            record.deps.clone()
        } else {
            let (release, _) = self.release(name, version)?;
            (self.sources.archive_deps)(&repo, name, &release)?
        };

        let mut found = Vec::with_capacity(declared.len());
        for (dependency, constraint) in declared {
            if self.repo_of(&dependency)?.is_none() {
                self.warn_unavailable(name, &dependency);
                continue;
            }
            let req = VerReq::parse(&constraint).with_context(|| {
                format!(
                    "{name} {version} declares the version constraint {constraint:?} for \
                     '{dependency}', which baler cannot read"
                )
            })?;
            found.push((Node::Cran(dependency), req));
        }
        Ok(found)
    }
}
