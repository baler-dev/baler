use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use baler_core::baler_toml::DEFAULT_CRAN_MIRROR;
use baler_core::cran::packages::{PackageRecord, RVersion};
use baler_core::cran::universe::{CranUniverse, Index, Sources};
use baler_core::resolver::{solve, Node, Preferred, Universe};
use baler_core::ver::{Ver, VerReq};

const CRAN: &str = DEFAULT_CRAN_MIRROR;
const CUSTOM: &str = "https://r-universe.example.org";

fn cran(name: &str) -> Node {
    Node::Cran(name.to_owned())
}

fn index_of(entries: &[(&str, &str, &[(&str, &str)])]) -> Index {
    entries
        .iter()
        .map(|(name, version, deps)| {
            let record = PackageRecord {
                version: RVersion::parse(version).unwrap(),
                deps: deps.iter().map(|(d, c)| (d.to_string(), c.to_string())).collect(),
            };
            (name.to_string(), record)
        })
        .collect()
}

fn releases(versions: &[&str]) -> Result<Vec<RVersion>> {
    versions.iter().map(|v| RVersion::parse(v)).collect()
}

fn declared(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries.iter().map(|(n, r)| (n.to_string(), r.to_string())).collect()
}

fn no_archive_versions(_: &str, _: &str) -> Result<Vec<RVersion>> {
    Ok(Vec::new())
}

fn no_archive_deps(_: &str, _: &str, _: &RVersion) -> Result<Vec<(String, String)>> {
    bail!("unexpected archive read")
}

#[test]
fn a_declared_package_comes_from_its_repo_and_the_rest_from_cran_first() {
    let repos: HashMap<String, Index> = HashMap::from([
        (CRAN.to_owned(), index_of(&[("cli", "3.6.2", &[])])),
        (
            CUSTOM.to_owned(),
            index_of(&[("mine", "1.0", &[("cli", "*")]), ("cli", "9.9.9", &[])]),
        ),
    ]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let universe =
        CranUniverse::new(&sources, declared(&[("mine", CUSTOM)]), HashSet::new());

    assert_eq!(universe.repo_of("mine").unwrap().as_deref(), Some(CUSTOM));
    assert_eq!(universe.repo_of("cli").unwrap().as_deref(), Some(CRAN));
    assert_eq!(
        universe.current(&cran("cli")).unwrap(),
        Some(Ver::parse("3.6.2").unwrap())
    );
}

#[test]
fn a_dependency_missing_from_cran_is_found_in_a_declared_repo() {
    let repos: HashMap<String, Index> = HashMap::from([
        (CRAN.to_owned(), index_of(&[])),
        (CUSTOM.to_owned(), index_of(&[("mine", "1.0", &[]), ("extra", "2.0", &[])])),
    ]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let universe =
        CranUniverse::new(&sources, declared(&[("mine", CUSTOM)]), HashSet::new());
    assert_eq!(universe.repo_of("extra").unwrap().as_deref(), Some(CUSTOM));
}

#[test]
fn a_dependency_in_no_repo_is_skipped_instead_of_failing() {
    let repos: HashMap<String, Index> = HashMap::from([(
        CRAN.to_owned(),
        index_of(&[("a", "1.0", &[("bioc-only", ">= 1.0"), ("cli", "*")]), ("cli", "3.0", &[])]),
    )]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let universe = CranUniverse::new(&sources, declared(&[("a", CRAN)]), HashSet::new());
    let found = universe
        .dependencies(&cran("a"), &Ver::parse("1.0").unwrap())
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, cran("cli"));
}

#[test]
fn each_index_is_fetched_once_and_only_when_needed() {
    let fetches = Cell::new(0);
    let repos: HashMap<String, Index> = HashMap::from([
        (CRAN.to_owned(), index_of(&[("cli", "3.6.2", &[])])),
        (CUSTOM.to_owned(), index_of(&[("mine", "1.0", &[])])),
    ]);
    let index = |repo: &str| {
        fetches.set(fetches.get() + 1);
        repos.get(repo).cloned().context("no such repo")
    };
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let universe =
        CranUniverse::new(&sources, declared(&[("mine", CUSTOM)]), HashSet::new());

    universe.current(&cran("mine")).unwrap();
    assert_eq!(fetches.get(), 1);
    universe.current(&cran("cli")).unwrap();
    universe.current(&cran("cli")).unwrap();
    universe.current(&cran("mine")).unwrap();
    assert_eq!(fetches.get(), 2);
}

#[test]
fn a_release_is_spelled_the_way_the_repository_publishes_it() {
    let repos: HashMap<String, Index> =
        HashMap::from([(CRAN.to_owned(), index_of(&[("pkg", "0.3-1", &[])]))]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let versions = |_: &str, _: &str| releases(&["0.2-9"]);
    let sources = Sources {
        index: &index,
        archive_versions: &versions,
        archive_deps: &no_archive_deps,
    };
    let universe = CranUniverse::new(&sources, declared(&[("pkg", CRAN)]), HashSet::new());

    let (current, _) = universe.release("pkg", &Ver::parse("0.3.1").unwrap()).unwrap();
    assert_eq!(current.as_str(), "0.3-1");
    let (older, _) = universe.release("pkg", &Ver::parse("0.2.9").unwrap()).unwrap();
    assert_eq!(older.as_str(), "0.2-9");
    assert!(universe.release("pkg", &Ver::parse("0.1").unwrap()).is_err());
}

#[test]
fn a_locked_package_has_no_dependencies_for_the_solver() {
    let repos: HashMap<String, Index> = HashMap::from([(
        CRAN.to_owned(),
        index_of(&[("a", "1.0", &[("cli", "*")]), ("cli", "3.0", &[])]),
    )]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let leaves = HashSet::from(["a".to_owned()]);
    let universe = CranUniverse::new(&sources, declared(&[("a", CRAN)]), leaves);
    let found = universe
        .dependencies(&cran("a"), &Ver::parse("1.0").unwrap())
        .unwrap();
    assert!(found.is_empty());
    assert_eq!(universe.index_dependency_names("a").unwrap(), ["cli"]);
}

#[test]
fn an_unreadable_constraint_names_the_release_and_the_dependency() {
    let repos: HashMap<String, Index> = HashMap::from([(
        CRAN.to_owned(),
        index_of(&[("a", "2.0", &[("cli", "nonsense")]), ("cli", "3.0", &[])]),
    )]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let sources = Sources {
        index: &index,
        archive_versions: &no_archive_versions,
        archive_deps: &no_archive_deps,
    };
    let universe = CranUniverse::new(&sources, declared(&[("a", CRAN)]), HashSet::new());
    let error = format!(
        "{:#}",
        universe.dependencies(&cran("a"), &Ver::parse("2.0").unwrap()).unwrap_err()
    );
    assert!(error.contains("a 2.0") && error.contains("'cli'"), "{error}");
}

#[test]
fn the_archive_is_listed_once_however_often_it_is_asked() {
    let repos: HashMap<String, Index> =
        HashMap::from([(CRAN.to_owned(), index_of(&[("cli", "3.6.2", &[])]))]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let listings = Cell::new(0);
    let versions = |_: &str, _: &str| {
        listings.set(listings.get() + 1);
        releases(&["2.0.0"])
    };
    let sources = Sources {
        index: &index,
        archive_versions: &versions,
        archive_deps: &no_archive_deps,
    };
    let universe = CranUniverse::new(&sources, declared(&[("cli", CRAN)]), HashSet::new());
    universe.older(&cran("cli")).unwrap();
    universe.older(&cran("cli")).unwrap();
    assert_eq!(listings.get(), 1);
}

#[test]
fn the_solver_backs_up_using_an_archived_releases_real_dependencies() {
    // The index only lists a 2.0, which needs cli >= 3. b needs cli < 3.
    // The archived a 1.0 has no dependencies, so the solver must go back
    // to it, and it can only know that by reading a 1.0's own description.
    let repos: HashMap<String, Index> = HashMap::from([(
        CRAN.to_owned(),
        index_of(&[
            ("a", "2.0", &[("cli", ">= 3.0")]),
            ("b", "1.0", &[("cli", "< 3.0")]),
            ("cli", "3.6.2", &[]),
        ]),
    )]);
    let index = |repo: &str| repos.get(repo).cloned().context("no such repo");
    let versions = |_: &str, name: &str| match name {
        "a" => releases(&["1.0"]),
        "cli" => releases(&["2.9.0"]),
        _ => releases(&[]),
    };
    let deps = |_: &str, name: &str, release: &RVersion| match (name, release.as_str()) {
        ("a", "1.0") | ("cli", "2.9.0") => Ok(Vec::new()),
        other => bail!("unexpected archive read: {other:?}"),
    };
    let sources = Sources { index: &index, archive_versions: &versions, archive_deps: &deps };
    let universe = CranUniverse::new(
        &sources,
        declared(&[("a", CRAN), ("b", CRAN)]),
        HashSet::new(),
    );
    let requested = [(cran("a"), VerReq::any()), (cran("b"), VerReq::any())];
    let solution = solve(&universe, &requested, &Preferred::new()).unwrap();
    assert_eq!(solution.versions[&cran("a")].as_str(), "1.0");
    assert_eq!(solution.versions[&cran("cli")].as_str(), "2.9.0");
}
