//! Dependency solver for modules and CRAN packages.
//!
//! This is a thin layer over the `pubgrub` crate (the algorithm uv and Dart's
//! pub use). It exists so that:
//!
//! - every candidate version is judged by its OWN dependencies, not by the
//!   newest release's,
//! - a choice that leads to a conflict is undone and the next candidate is
//!   tried (backtracking), instead of failing on the first fit,
//! - a conflict is explained as a chain of reasons ("because a 2.0 needs
//!   cli >= 3 and b needs cli < 3, ..."), and
//! - the same input always gives the same answer.
//!
//! The solver does no IO. Where versions and dependencies come from is the
//! job of a `Universe`, so the algorithm is tested without a network.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt;

use anyhow::{bail, Result};
use pubgrub::{
    DefaultStringReporter, Dependencies, DependencyProvider, PackageResolutionStatistics,
    PubGrubError, Ranges, Reporter,
};

use crate::ver::{Op, Ver, VerReq};

/// Something that can be installed and depended on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    /// The project being resolved. Never part of a `Solution`.
    Root,
    Cran(String),
    Module(String),
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Root => write!(f, "the project"),
            Node::Cran(name) => write!(f, "{name}"),
            Node::Module(name) => write!(f, "module {name}"),
        }
    }
}

/// Where versions and dependencies come from.
///
/// Versions are asked for in two steps because the second one can be slow.
/// `current` is the newest release, which a package index lists directly.
/// `older` is everything else (CRAN's Archive/), and the solver only asks
/// for it when the newest release does not fit.
pub trait Universe {
    /// The newest version of `node`, or `None` when the node does not exist.
    fn current(&self, node: &Node) -> Result<Option<Ver>>;

    /// Every other known version of `node`, in any order.
    fn older(&self, node: &Node) -> Result<Vec<Ver>>;

    /// What exactly this version of `node` depends on. An error when the
    /// version does not exist or its metadata cannot be read.
    fn dependencies(&self, node: &Node, version: &Ver) -> Result<Vec<(Node, VerReq)>>;
}

/// Versions to stay on when they still fit, for example the ones a lock file
/// pinned. A preference, not a requirement: a pin that conflicts with a new
/// constraint is dropped and that package is resolved again.
pub type Preferred = HashMap<Node, Ver>;

/// One consistent set of versions, and who depends on whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solution {
    pub versions: BTreeMap<Node, Ver>,
    pub edges: BTreeMap<Node, BTreeSet<Node>>,
}

impl Solution {
    /// Nodes with their dependencies first. See `dependency_order`.
    pub fn install_order(&self) -> Vec<Node> {
        dependency_order(self.versions.keys(), &self.edges)
    }
}

/// `nodes` with every node after the ones it depends on, alphabetical among
/// equals, so the install order is the same on every machine. Edges that
/// point outside `nodes` are ignored.
pub fn dependency_order<'a>(
    nodes: impl Iterator<Item = &'a Node>,
    edges: &BTreeMap<Node, BTreeSet<Node>>,
) -> Vec<Node> {
    fn visit<'a>(
        node: &'a Node,
        members: &BTreeSet<&'a Node>,
        edges: &'a BTreeMap<Node, BTreeSet<Node>>,
        done: &mut BTreeSet<&'a Node>,
        ordered: &mut Vec<Node>,
    ) {
        if !members.contains(node) || !done.insert(node) {
            return;
        }
        if let Some(dependencies) = edges.get(node) {
            for dependency in dependencies {
                visit(dependency, members, edges, done, ordered);
            }
        }
        ordered.push(node.clone());
    }

    let members: BTreeSet<&Node> = nodes.collect();
    let mut done = BTreeSet::new();
    let mut ordered = Vec::with_capacity(members.len());
    for node in &members {
        visit(node, &members, edges, &mut done, &mut ordered);
    }
    ordered
}

/// Solve `requested` (what the project depends on directly).
pub fn solve(
    universe: &dyn Universe,
    requested: &[(Node, VerReq)],
    preferred: &Preferred,
) -> Result<Solution> {
    for (node, _) in requested {
        if universe.current(node)?.is_none() {
            bail!("'{node}' was not found. Check its name and where it is looked up.");
        }
    }

    let provider = Provider {
        universe,
        preferred,
        requested: requested.to_vec(),
        current: RefCell::default(),
        older: RefCell::default(),
        dependencies: RefCell::default(),
    };

    let selected = match pubgrub::resolve(&provider, Node::Root, root_version()) {
        Ok(selected) => selected,
        Err(PubGrubError::NoSolution(tree)) => {
            bail!(
                "No set of versions satisfies every dependency:\n{}",
                DefaultStringReporter::report(&tree)
            )
        }
        Err(PubGrubError::ErrorRetrievingDependencies { package, version, source }) => {
            bail!("Could not read the dependencies of {package} {version}: {source}")
        }
        Err(PubGrubError::ErrorChoosingVersion { package, source }) => {
            bail!("Could not list the versions of {package}: {source}")
        }
        Err(PubGrubError::ErrorInShouldCancel(source)) => bail!("Resolution stopped: {source}"),
    };

    let versions: BTreeMap<Node, Ver> = selected
        .into_iter()
        .filter(|(node, _)| *node != Node::Root)
        .collect();

    let mut edges = BTreeMap::new();
    for (node, version) in &versions {
        let dependencies = provider
            .dependencies_of(node, version)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let targets = dependencies
            .into_iter()
            .map(|(target, _)| target)
            .filter(|target| versions.contains_key(target))
            .collect::<BTreeSet<Node>>();
        edges.insert(node.clone(), targets);
    }

    Ok(Solution { versions, edges })
}

fn root_version() -> Ver {
    Ver::from_parts(vec![0])
}

/// The versions a requirement allows, as a set `pubgrub` can intersect.
fn ranges_of(req: &VerReq) -> Ranges<Ver> {
    req.constraints().fold(Ranges::full(), |allowed, (op, ver)| {
        let part = match op {
            Op::Lt => Ranges::strictly_lower_than(ver.clone()),
            Op::Le => Ranges::lower_than(ver.clone()),
            Op::Eq => Ranges::singleton(ver.clone()),
            Op::Ge => Ranges::higher_than(ver.clone()),
            Op::Gt => Ranges::strictly_higher_than(ver.clone()),
        };
        allowed.intersection(&part)
    })
}

#[derive(Debug)]
struct ProviderError(String);

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for ProviderError {}

fn failed<T>(attempt: Result<T>) -> std::result::Result<T, ProviderError> {
    attempt.map_err(|error| ProviderError(format!("{error:#}")))
}

/// Remembers every answer from the universe, because the solver asks the
/// same question many times while it backtracks and some answers cost a
/// download.
struct Provider<'a> {
    universe: &'a dyn Universe,
    preferred: &'a Preferred,
    requested: Vec<(Node, VerReq)>,
    current: RefCell<HashMap<Node, Option<Ver>>>,
    older: RefCell<HashMap<Node, Vec<Ver>>>,
    dependencies: RefCell<HashMap<(Node, Ver), Vec<(Node, VerReq)>>>,
}

impl Provider<'_> {
    fn current_of(&self, node: &Node) -> std::result::Result<Option<Ver>, ProviderError> {
        if let Some(known) = self.current.borrow().get(node) {
            return Ok(known.clone());
        }
        let found = failed(self.universe.current(node))?;
        self.current.borrow_mut().insert(node.clone(), found.clone());
        Ok(found)
    }

    fn older_of(&self, node: &Node) -> std::result::Result<Vec<Ver>, ProviderError> {
        if let Some(known) = self.older.borrow().get(node) {
            return Ok(known.clone());
        }
        let found = failed(self.universe.older(node))?;
        self.older.borrow_mut().insert(node.clone(), found.clone());
        Ok(found)
    }

    fn dependencies_of(
        &self,
        node: &Node,
        version: &Ver,
    ) -> std::result::Result<Vec<(Node, VerReq)>, ProviderError> {
        if *node == Node::Root {
            return Ok(self.requested.clone());
        }
        let key = (node.clone(), version.clone());
        if let Some(known) = self.dependencies.borrow().get(&key) {
            return Ok(known.clone());
        }
        let found = failed(self.universe.dependencies(node, version))?;
        self.dependencies.borrow_mut().insert(key, found.clone());
        Ok(found)
    }
}

impl DependencyProvider for Provider<'_> {
    type P = Node;
    type V = Ver;
    type VS = Ranges<Ver>;
    type M = String;
    type Priority = (u32, std::cmp::Reverse<String>);
    type Err = ProviderError;

    /// Packages that have caused conflicts are decided first, so a bad
    /// combination is found early. Ties go to the alphabetically first name.
    fn prioritize(
        &self,
        package: &Node,
        _range: &Ranges<Ver>,
        statistics: &PackageResolutionStatistics,
    ) -> Self::Priority {
        (statistics.conflict_count(), std::cmp::Reverse(package.to_string()))
    }

    /// The preferred version if it still fits, else the newest that fits.
    /// The slow list of older versions is only fetched once the newest
    /// release does not fit.
    fn choose_version(
        &self,
        package: &Node,
        range: &Ranges<Ver>,
    ) -> std::result::Result<Option<Ver>, ProviderError> {
        if *package == Node::Root {
            let only = root_version();
            return Ok(range.contains(&only).then_some(only));
        }
        if let Some(pin) = self.preferred.get(package) {
            if range.contains(pin) {
                return Ok(Some(pin.clone()));
            }
        }
        if let Some(newest) = self.current_of(package)? {
            if range.contains(&newest) {
                return Ok(Some(newest));
            }
        }
        let older = self.older_of(package)?;
        Ok(older.into_iter().filter(|version| range.contains(version)).max())
    }

    fn get_dependencies(
        &self,
        package: &Node,
        version: &Ver,
    ) -> std::result::Result<Dependencies<Node, Ranges<Ver>, String>, ProviderError> {
        let mut merged: HashMap<Node, Ranges<Ver>> = HashMap::new();
        for (node, req) in self.dependencies_of(package, version)? {
            let allowed = ranges_of(&req);
            let combined = match merged.get(&node) {
                Some(already) => already.intersection(&allowed),
                None => allowed,
            };
            merged.insert(node, combined);
        }
        Ok(Dependencies::Available(merged.into_iter().collect()))
    }
}
